# SPDX-License-Identifier: GPL-2.0-only
CFLAGS += $(shell pkg-config --cflags libdrm) -Wall -Wextra
LDLIBS += $(shell pkg-config --libs libdrm)
TEST_GEN_PROGS := modeset
TEST_FILES := standalone.config
LOCAL_HDRS := fixture.h
include ../lib.mk

$(OUTPUT)/modeset: fixture.c
