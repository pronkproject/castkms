# SPDX-License-Identifier: GPL-2.0-only
CFLAGS += $(shell pkg-config --cflags libdrm) -Wall -Wextra
LDLIBS += $(shell pkg-config --libs libdrm)
TEST_GEN_PROGS := modeset master-lifetime execution capture-grant
TEST_PROGS := takeover-model.py handback-model.py
TEST_FILES := standalone.config
LOCAL_HDRS := fixture.h
include ../lib.mk

$(OUTPUT)/modeset: fixture.c import.c
$(OUTPUT)/master-lifetime: fixture.c
$(OUTPUT)/capture-grant: fixture.c
