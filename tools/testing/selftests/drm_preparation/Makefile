# SPDX-License-Identifier: GPL-2.0
TEST_PROGS := preparation-model.py
TEST_PROGS += output-model.py
TEST_PROGS += pipeline-model.py
TEST_PROGS += reservation-model.py
CFLAGS += $(shell pkg-config --cflags libdrm) -Wall -Wextra
LDLIBS += $(shell pkg-config --libs libdrm)
TEST_GEN_PROGS := atomic-ticket
TEST_GEN_PROGS += atomic-input
TEST_GEN_PROGS += atomic-signaling
TEST_GEN_PROGS += legacy-cursor
include ../lib.mk
