// SPDX-License-Identifier: GPL-2.0-only
/* Requires a disposable one-output device, PipeWire, WirePlumber, pw-play, wpctl and jq. */
#include "audio-fixture.h"
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <unistd.h>

static pid_t player = -1;

static void stop_player(void)
{
	if (player > 0) {
		pid_t ret;

		do {
			ret = waitpid(player, NULL, WNOHANG);
		} while (ret < 0 && errno == EINTR);
		if (ret == 0) {
			kill(player, SIGTERM);
			while (waitpid(player, NULL, 0) < 0 && errno == EINTR)
				;
		}
		player = -1;
	}
}

static void start_player(void)
{
	char command[512], target[128];
	unsigned short samples[9600];
	FILE *signal;
	unsigned int node;
	int card = audio_card(0), found = 0;

	CHECK(card >= 0);
	/* Restrict the stream to the selected CastKMS card, never the desktop default. */
	snprintf(command, sizeof(command),
		 "pw-dump | jq -r '.[] | select(.info.props[\"media.class\"] == \"Audio/Sink\" and "
		 "(.info.props[\"api.alsa.pcm.card\"] | tostring) == \"%d\") | .id, .info.props[\"node.name\"]'",
		 card);
	for (unsigned int attempt = 0; attempt < 100; attempt++) {
		FILE *nodes = popen(command, "r");

		CHECK(nodes);
		found = fscanf(nodes, "%u\n", &node) == 1 && fgets(target, sizeof(target), nodes) != NULL;
		CHECK(pclose(nodes) == 0);
		if (found)
			break;
		usleep(100000);
	}
	CHECK(found);
	target[strcspn(target, "\n")] = 0;
	printf("PipeWire target: %s\n", target);
	snprintf(command, sizeof(command), "wpctl set-volume %u 1.0 && wpctl set-mute %u 0", node, node);
	CHECK(system(command) == 0);
	signal = tmpfile();
	CHECK(signal);
	CHECK(fcntl(fileno(signal), F_SETFD, FD_CLOEXEC) == 0);
	for (unsigned int i = 0; i < 9600; i += 2) {
		samples[i] = 0x1234;
		samples[i + 1] = 0x5678;
	}
	for (unsigned int i = 0; i < 200; i++)
		CHECK(fwrite(samples, sizeof(samples), 1, signal) == 1);
	CHECK(fflush(signal) == 0);
	rewind(signal);
	player = fork();
	CHECK(player >= 0);
	if (player == 0) {
		CHECK(dup2(fileno(signal), STDIN_FILENO) == STDIN_FILENO);
		execlp("pw-play", "pw-play", "--raw", "--format=s16", "--rate=48000",
		       "--channels=2", "--target", target, "--properties",
		       "{ node.dont-fallback = true node.dont-move = true node.dont-reconnect = false }",
		       "-", NULL);
		_exit(127);
	}
	fclose(signal);
}

static void expect_signal(int fd)
{
	unsigned char samples[1920];

	for (unsigned int attempt = 0; attempt < 500; attempt++) {
		struct pollfd event = { .fd = fd, .events = POLLIN };
		ssize_t count;
		int status;

		pid_t ret = waitpid(player, &status, WNOHANG);

		if (ret == player)
			player = -1;
		CHECK(ret == 0);
		CHECK(poll(&event, 1, 100) >= 0);
		CHECK(!(event.revents & (POLLHUP | POLLERR)));
		count = read(fd, samples, sizeof(samples));
		if (count < 0 && errno == EAGAIN)
			continue;
		CHECK(count > 0 && count % 4 == 0);
		for (ssize_t i = 0; i < count; i += 4)
			if (!memcmp(samples + i, "\x34\x12\x78\x56", 4))
				return;
	}
	CHECK(system("pw-dump -N") == 0);
	CHECK(!"PipeWire samples did not arrive through the audio UAPI");
}

int main(int argc, char **argv)
{
	struct drm_castkms_monitor_files monitor_files;
	struct drm_castkms_create_monitor_control monitor = { .files = (uintptr_t)&monitor_files };
	struct drm_castkms_monitor_attach attach = {0};
	struct drm_castkms_monitor_detach detach = {0};
	struct drm_castkms_audio_files audio, next;
	struct buffer buffer;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo mode;
	drmVersion *version;
	unsigned char edid[256];
	uint32_t crtc, connector_id;
	int fd;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable one-output CastKMS node with PipeWire running\n");
		return 4;
	}
	if (system("command -v pw-dump >/dev/null 2>&1") != 0 ||
	    system("command -v wpctl >/dev/null 2>&1") != 0 ||
	    system("command -v pw-play >/dev/null 2>&1") != 0 ||
	    system("command -v jq >/dev/null 2>&1") != 0 ||
	    system("pw-dump >/dev/null 2>&1") != 0) {
		fprintf(stderr, "SKIP: active PipeWire and pw-play, wpctl, jq commands required\n");
		return 4;
	}
	CHECK(atexit(stop_player) == 0);
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	CHECK(drmSetMaster(fd) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	crtc = resources->crtcs[0];
	connector_id = resources->connectors[0];
	drmModeFreeResources(resources);
	monitor.connector_id = connector_id;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &monitor) == 0);
	audio_edid(edid);
	attach.edid_ptr = (uintptr_t)edid;
	attach.edid_size = sizeof(edid);
	CHECK(ioctl(monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->count_modes > 0);
	mode = connector->modes[0];
	drmModeFreeConnector(connector);
	buffer = create_buffer(fd, mode.hdisplay, mode.vdisplay, 0);
	CHECK(drmModeSetCrtc(fd, crtc, buffer.fb, 0, 0, &connector_id, 1, &mode) == 0);
	audio = audio_capture(fd, crtc, connector_id);
	start_player();
	expect_signal(audio.audio_fd);
	CHECK(drmModeSetCrtc(fd, crtc, 0, 0, 0, NULL, 0, NULL) == 0);
	{
		struct pollfd event = { .fd = audio.audio_fd, .events = POLLIN };

		CHECK(poll(&event, 1, 100) == 0);
	}
	CHECK(drmModeSetCrtc(fd, crtc, buffer.fb, 0, 0, &connector_id, 1, &mode) == 0);
	expect_signal(audio.audio_fd);
	stop_player();
	CHECK(ioctl(monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_DETACH, &detach) == 0);
	audio_terminal(audio.audio_fd);
	usleep(500000);
	CHECK(ioctl(monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	next = audio_capture(fd, crtc, connector_id);
	start_player();
	expect_signal(next.audio_fd);
	stop_player();
	audio_terminal(audio.audio_fd);
	CHECK(close(next.revoke_fd) == 0);
	audio_terminal(next.audio_fd);
	CHECK(close(next.audio_fd) == 0);
	CHECK(close(audio.audio_fd) == 0 && close(audio.revoke_fd) == 0);
	CHECK(drmModeSetCrtc(fd, crtc, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(fd, &buffer);
	CHECK(close(monitor_files.control_fd) == 0 && close(monitor_files.revoke_fd) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: PipeWire discovery, sample delivery, modeset recovery and reattachment");
	return 0;
}
