// SPDX-License-Identifier: GPL-2.0-only
/* Exercise the monitor-scoped transport through the native CEC device. */
#include <fcntl.h>
#include <linux/cec.h>
#include <poll.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "fixture.h"

#include "../../../../include/uapi/drm/castkms_drm.h"

_Static_assert(sizeof(struct drm_castkms_cec_set_transport) == 8,
	       "CEC transport layout");
_Static_assert(sizeof(struct drm_castkms_cec_transaction) == 40,
	       "CEC transaction layout");
_Static_assert(sizeof(struct drm_castkms_cec_complete) == 16,
	       "CEC completion layout");
_Static_assert(sizeof(struct drm_castkms_cec_receive) == 32,
	       "CEC receive layout");
_Static_assert(sizeof(struct drm_castkms_cec_state) == 96,
	       "CEC state layout");

static void fill_cec_edid(unsigned char edid[256])
{
	static const unsigned char header[] = {
		0, 255, 255, 255, 255, 255, 255, 0
	};
	static const unsigned char display[] = {
		1, 3, 0x80, 52, 29, 120, 0x0a
	};
	/* CTA extension containing an HDMI VSDB with physical address 1.0.0.0. */
	static const unsigned char cta[] = {
		2, 3, 10, 0, 0x65, 0x03, 0x0c, 0x00, 0x10, 0x00
	};

	memset(edid, 0, 256);
	memcpy(edid, header, sizeof(header));
	edid[8] = 0x31;
	edid[9] = 0xd8;
	edid[10] = 42;
	memcpy(edid + 18, display, sizeof(display));
	memset(edid + 38, 1, 16);
	edid[57] = 0xfc;
	memcpy(edid + 59, "CastKMS CEC", 11);
	edid[126] = 1;
	memcpy(edid + 128, cta, sizeof(cta));
	for (unsigned int block = 0; block < 256; block += 128) {
		unsigned char sum = 0;

		for (unsigned int i = 0; i < 127; i++)
			sum += edid[block + i];
		edid[block + 127] = -sum;
	}
}

static int open_cec(uint32_t connector_id)
{
	for (unsigned int index = 0; index < 64; index++) {
		struct cec_connector_info connector = {0};
		struct cec_caps caps = {0};
		char path[32];
		int fd;

		snprintf(path, sizeof(path), "/dev/cec%u", index);
		fd = open(path, O_RDWR | O_CLOEXEC | O_NONBLOCK);
		if (fd < 0)
			continue;
		if (!ioctl(fd, CEC_ADAP_G_CAPS, &caps) &&
		    (caps.capabilities & CEC_CAP_TRANSMIT) &&
		    (caps.capabilities & CEC_CAP_CONNECTOR_INFO) &&
		    !ioctl(fd, CEC_ADAP_G_CONNECTOR_INFO, &connector) &&
		    connector.type == CEC_CONNECTOR_TYPE_DRM &&
		    connector.drm.connector_id == connector_id)
			return fd;
		CHECK(close(fd) == 0);
	}
	return -1;
}

static void configure_cec(int fd)
{
	struct cec_log_addrs addresses = {
		.cec_version = CEC_OP_CEC_VERSION_2_0,
		.num_log_addrs = 1,
		.vendor_id = CEC_VENDOR_ID_NONE,
	};
	uint32_t mode = CEC_MODE_INITIATOR | CEC_MODE_FOLLOWER;
	int flags = fcntl(fd, F_GETFL);

	addresses.primary_device_type[0] = CEC_OP_PRIM_DEVTYPE_SWITCH;
	addresses.log_addr_type[0] = CEC_LOG_ADDR_TYPE_UNREGISTERED;
	addresses.all_device_types[0] = CEC_OP_ALL_DEVTYPE_SWITCH;
	memcpy(addresses.osd_name, "CastKMS test", sizeof("CastKMS test"));
	CHECK(ioctl(fd, CEC_S_MODE, &mode) == 0);
	CHECK(flags >= 0);
	CHECK(fcntl(fd, F_SETFL, flags & ~O_NONBLOCK) == 0);
	CHECK(ioctl(fd, CEC_ADAP_S_LOG_ADDRS, &addresses) == 0);
	CHECK(fcntl(fd, F_SETFL, flags) == 0);
	CHECK(addresses.num_log_addrs == 1);
	CHECK(addresses.log_addr[0] == CEC_LOG_ADDR_UNREGISTERED);
}

static void wait_readable_at(int fd, unsigned int line)
{
	struct pollfd event = { .fd = fd, .events = POLLIN };

	if (poll(&event, 1, 3000) != 1 || !(event.revents & POLLIN)) {
		fprintf(stderr, "cec.c:%u: timed out waiting for readable fd\n", line);
		exit(1);
	}
}

#define wait_readable(fd) wait_readable_at(fd, __LINE__)

static void expect_error_at(int fd, unsigned long command, void *request,
			    int error, unsigned int line)
{
	errno = 0;
	if (ioctl(fd, command, request) >= 0 || errno != error) {
		fprintf(stderr, "cec.c:%u: expected ioctl error %d, got %d\n",
			line, error, errno);
		exit(1);
	}
}

#define expect_error(fd, command, request, error) \
	expect_error_at(fd, command, request, error, __LINE__)

int main(int argc, char **argv)
{
	struct drm_castkms_monitor_files files = { -1, -1 };
	struct drm_castkms_create_monitor_control create = {
		.flags = DRM_CASTKMS_MONITOR_CREATE_ADMIN,
		.files = (uintptr_t)&files,
	};
	struct drm_castkms_monitor_query query = {0};
	struct drm_castkms_monitor_attach attach = {0};
	struct drm_castkms_cec_set_transport transport = {
		.flags = DRM_CASTKMS_CEC_TRANSPORT_ONLINE,
	};
	struct drm_castkms_cec_transaction transaction = {0};
	struct drm_castkms_cec_complete complete = {0};
	struct drm_castkms_cec_receive receive = {0};
	struct drm_castkms_cec_state state = {0};
	struct cec_msg message = {0}, result = {0};
	unsigned char edid[256];
	drmModeConnector *connector;
	drmModeRes *resources;
	uint32_t connector_id;
	int cec_fd, fd, helper;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(drmSetMaster(fd) == 0);
	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_connectors > 0);
	connector_id = resources->connectors[0];
	drmModeFreeResources(resources);

	create.connector_id = connector_id;
	CHECK(ioctl(helper, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &create) == 0);
	CHECK(drmIsMaster(fd) && !drmIsMaster(helper));
	CHECK(close(helper) == 0);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_MONITOR_CONTROL_VERSION);
	CHECK(query.flags == DRM_CASTKMS_MONITOR_CAP_CEC);
	transport.flags = 2;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_SET_TRANSPORT,
		     &transport, EINVAL);
	transport.flags = 0;
	transport.reserved = 1;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_SET_TRANSPORT,
		     &transport, EINVAL);
	transport.reserved = 0;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		     &transaction, EAGAIN);

	fill_cec_edid(edid);
	attach.edid_size = sizeof(edid);
	attach.edid_ptr = (uintptr_t)edid;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->connection == DRM_MODE_CONNECTED);
	drmModeFreeConnector(connector);
	transport.flags = DRM_CASTKMS_CEC_TRANSPORT_ONLINE;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_SET_TRANSPORT,
		    &transport) == 0);

	cec_fd = open_cec(connector_id);
	CHECK(cec_fd >= 0);
	configure_cec(cec_fd);
	/* Logical-address setup announces the adapter's physical address. */
	wait_readable(files.control_fd);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		    &transaction) == 0);
	CHECK(transaction.length == 5);
	CHECK(transaction.msg[1] == CEC_MSG_REPORT_PHYSICAL_ADDR);
	complete.cookie = transaction.cookie;
	complete.status = CEC_TX_STATUS_OK;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		    &complete) == 0);
	memset(&transaction, 0, sizeof(transaction));
	memset(&complete, 0, sizeof(complete));

	message.len = 2;
	message.msg[0] = 0xff;
	message.msg[1] = CEC_MSG_REQUEST_ACTIVE_SOURCE;
	message.timeout = 0;
	CHECK(ioctl(cec_fd, CEC_TRANSMIT, &message) == 0);
	CHECK(message.sequence != 0);
	wait_readable(files.control_fd);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_GET_STATE,
		    &state) == 0);
	complete.cookie = state.pending_cookie;
	complete.status = CEC_TX_STATUS_OK;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		     &complete, EAGAIN);
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		     (void *)1, EFAULT);
	wait_readable(files.control_fd);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		    &transaction) == 0);
	CHECK(transaction.cookie != 0);
	CHECK(transaction.length == 2);
	CHECK(transaction.msg[0] == 0xff);
	CHECK(transaction.msg[1] == CEC_MSG_REQUEST_ACTIVE_SOURCE);
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		     &transaction, EAGAIN);
	complete.cookie = transaction.cookie + 1;
	complete.status = CEC_TX_STATUS_OK;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		     &complete, ESTALE);
	complete.cookie = transaction.cookie;
	complete.status = 0;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		     &complete, EINVAL);
	complete.status = CEC_TX_STATUS_OK;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		    &complete) == 0);
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_COMPLETE_TX,
		     &complete, ENOENT);
	wait_readable(cec_fd);
	CHECK(ioctl(cec_fd, CEC_RECEIVE, &result) == 0);
	CHECK(result.sequence == message.sequence);
	CHECK(result.tx_status & CEC_TX_STATUS_OK);

	receive.length = 2;
	receive.msg[0] = 0x0f;
	receive.msg[1] = CEC_MSG_REQUEST_ACTIVE_SOURCE;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_RECEIVE,
		    &receive) == 0);
	wait_readable(cec_fd);
	memset(&result, 0, sizeof(result));
	CHECK(ioctl(cec_fd, CEC_RECEIVE, &result) == 0);
	CHECK(result.sequence == 0 && result.len == receive.length);
	CHECK(!memcmp(result.msg, receive.msg, receive.length));

	receive.msg[0] = 0xff;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_RECEIVE,
		     &receive, EINVAL);
	receive.msg[0] = 0x0f;
	receive.length = 0;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_RECEIVE,
		     &receive, EINVAL);
	receive.length = 2;

	/* An abandoned transport transaction must terminate the native request. */
	memset(&message, 0, sizeof(message));
	message.len = 2;
	message.msg[0] = 0xff;
	message.msg[1] = CEC_MSG_REQUEST_ACTIVE_SOURCE;
	message.timeout = 0;
	CHECK(ioctl(cec_fd, CEC_TRANSMIT, &message) == 0);
	wait_readable(files.control_fd);
	memset(&transaction, 0, sizeof(transaction));
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		    &transaction) == 0);
	wait_readable(cec_fd);
	memset(&result, 0, sizeof(result));
	CHECK(ioctl(cec_fd, CEC_RECEIVE, &result) == 0);
	CHECK(result.sequence == message.sequence);
	CHECK(result.tx_status & CEC_TX_STATUS_ERROR);

	/* Replacing an attached sink preserves online state but aborts its work. */
	memset(&message, 0, sizeof(message));
	message.len = 2;
	message.msg[0] = 0xff;
	message.msg[1] = CEC_MSG_REQUEST_ACTIVE_SOURCE;
	message.timeout = 0;
	CHECK(ioctl(cec_fd, CEC_TRANSMIT, &message) == 0);
	wait_readable(files.control_fd);
	memset(&transaction, 0, sizeof(transaction));
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_ACQUIRE_TX,
		    &transaction) == 0);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	wait_readable(cec_fd);
	memset(&result, 0, sizeof(result));
	CHECK(ioctl(cec_fd, CEC_RECEIVE, &result) == 0);
	CHECK(result.sequence == message.sequence);
	CHECK(result.tx_status & CEC_TX_STATUS_ERROR);

	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_GET_STATE,
		    &state) == 0);
	CHECK((state.flags & (DRM_CASTKMS_CEC_STATE_ONLINE |
			      DRM_CASTKMS_CEC_STATE_MONITOR_ATTACHED |
			      DRM_CASTKMS_CEC_STATE_ADAPTER_ENABLED)) ==
	      (DRM_CASTKMS_CEC_STATE_ONLINE |
	       DRM_CASTKMS_CEC_STATE_MONITOR_ATTACHED |
	       DRM_CASTKMS_CEC_STATE_ADAPTER_ENABLED));
	CHECK(state.physical_address == 0x1000);
	CHECK(state.logical_address_mask == (1U << CEC_LOG_ADDR_UNREGISTERED));
	CHECK(state.stats_tx_submitted == 4 && state.stats_tx_completed == 2);
	CHECK(state.stats_tx_error == 2 && state.stats_tx_timeout == 1);
	CHECK(state.stats_rx == 1 && state.stats_invalid == 2);

	transport.flags = 0;
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_SET_TRANSPORT,
		    &transport) == 0);
	receive.msg[0] = 0x0f;
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_RECEIVE,
		     &receive, ENONET);
	CHECK(close(files.revoke_fd) == 0);
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_CEC_GET_STATE,
		     &state, ECANCELED);
	{
		struct pollfd event = { .fd = files.control_fd, .events = POLLIN };

		CHECK(poll(&event, 1, 0) == 1);
		CHECK((event.revents & (POLLHUP | POLLERR)) ==
		      (POLLHUP | POLLERR));
	}
	CHECK(close(cec_fd) == 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: monitor-scoped CastKMS CEC transport");
	return 0;
}
