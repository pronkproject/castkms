# SPDX-License-Identifier: GPL-2.0-only
CFLAGS += $(shell pkg-config --cflags libdrm) -Wall -Wextra
LDLIBS += $(shell pkg-config --libs libdrm)
TEST_GEN_PROGS := modeset
TEST_FILES := standalone.config
include ../lib.mk
