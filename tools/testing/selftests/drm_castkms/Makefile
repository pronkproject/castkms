# SPDX-License-Identifier: GPL-2.0-only
CFLAGS += $(shell pkg-config --cflags libdrm) -Wall -Wextra
LDLIBS += $(shell pkg-config --libs libdrm)
TEST_GEN_PROGS := modeset master-lifetime execution monitor-control renderer-control capture-grant capture-stream capture-destination capture-output
TEST_PROGS := takeover-model.py handback-model.py
TEST_FILES := standalone.config
TEST_GEN_PROGS += audio
LOCAL_HDRS := fixture.h audio-fixture.h
include ../lib.mk

$(OUTPUT)/modeset: fixture.c import.c
$(OUTPUT)/master-lifetime: fixture.c
$(OUTPUT)/monitor-control: fixture.c
$(OUTPUT)/renderer-control: fixture.c
$(OUTPUT)/capture-grant: fixture.c
$(OUTPUT)/capture-stream: fixture.c
$(OUTPUT)/capture-destination: fixture.c
$(OUTPUT)/capture-output: fixture.c
$(OUTPUT)/audio: fixture.c audio-fixture.c
