# SPDX-License-Identifier: GPL-2.0 OR MIT
# Opt-in hardware fixture; the parent directory's model tests need no GPU SDK.
OUTPUT ?= .
CC ?= cc
PKG_CONFIG ?= pkg-config
CFLAGS += -std=gnu11 -O2 -g -Wall -Wextra -Werror
CPPFLAGS += $(shell $(PKG_CONFIG) --cflags vulkan)
LDLIBS += $(shell $(PKG_CONFIG) --libs vulkan)

.PHONY: all
all: $(OUTPUT)/probe $(OUTPUT)/handoff $(OUTPUT)/reuse $(OUTPUT)/reuse-test $(OUTPUT)/sync-file-test $(OUTPUT)/context-test

$(OUTPUT)/reuse-main.o: reuse.c recycle.h image.h device.h context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) -Dmain=gpu_reuse_main -c reuse.c -o $@

$(OUTPUT)/reuse-test: reuse_test.c recycle.h image.h device.h context.h $(OUTPUT)/reuse-main.o
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) reuse_test.c $(OUTPUT)/reuse-main.o -o $@

$(OUTPUT)/reuse: reuse.c recycle.c recycle.h image.c image.h device.c device.h context.c context.h sync_file.c sync_file.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) reuse.c recycle.c image.c device.c context.c sync_file.c -o $@ $(LDLIBS)

$(OUTPUT)/probe: probe.c context.c context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) probe.c context.c $(LDLIBS) -o $@

$(OUTPUT)/handoff: handoff.c image.c image.h device.c device.h context.c context.h sync_file.c sync_file.h timing.c timing.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) handoff.c image.c device.c context.c sync_file.c timing.c $(LDLIBS) -o $@

$(OUTPUT)/sync-file-test: sync_file_test.c sync_file.c sync_file.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) sync_file_test.c sync_file.c -o $@

$(OUTPUT)/context-test: context_test.c context.c context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) context_test.c context.c $(LDLIBS) \
		-Wl,--wrap=vkCreateInstance,--wrap=vkEnumeratePhysicalDevices,--wrap=vkDestroyInstance -o $@
