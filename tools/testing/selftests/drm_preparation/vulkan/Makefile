# SPDX-License-Identifier: GPL-2.0 OR MIT
# Opt-in hardware fixture; the parent directory's model tests need no GPU SDK.
OUTPUT ?= .
CC ?= cc
PKG_CONFIG ?= pkg-config
CFLAGS += -std=gnu11 -O2 -g -Wall -Wextra -Werror
CPPFLAGS += $(shell $(PKG_CONFIG) --cflags vulkan)
LDLIBS += $(shell $(PKG_CONFIG) --libs vulkan)

.PHONY: all
all: $(OUTPUT)/probe $(OUTPUT)/handoff $(OUTPUT)/context-test

$(OUTPUT)/probe: probe.c context.c context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) probe.c context.c $(LDLIBS) -o $@

$(OUTPUT)/handoff: handoff.c image.c image.h device.c device.h context.c context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) handoff.c image.c device.c context.c $(LDLIBS) -o $@

$(OUTPUT)/context-test: context_test.c context.c context.h
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) context_test.c context.c $(LDLIBS) \
		-Wl,--wrap=vkCreateInstance,--wrap=vkEnumeratePhysicalDevices,--wrap=vkDestroyInstance -o $@
