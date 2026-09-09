#!/bin/sh
# SPDX-License-Identifier: GPL-2.0 OR MIT
# Reject malformed size options without needing a graphics device.
set -eu

if [ "$#" -ne 1 ]; then
	echo "Usage: $0 HANDOFF_BINARY" >&2
	exit 1
fi
binary=$1
scratch=$(mktemp -d)
trap 'rm -f "$scratch/output"; rmdir "$scratch"' EXIT
trap 'exit 1' HUP INT TERM
count=0

expect_usage()
{
	status=0
	"$binary" /dev/null "$@" > "$scratch/output" 2>&1 || status=$?
	if [ "$status" -ne 1 ] || ! grep -q '^Usage:' "$scratch/output"; then
		echo "not ok - rejected arguments: $*" >&2
		cat "$scratch/output" >&2
		exit 1
	fi
	count=$((count + 1))
	echo "ok $count - rejected arguments: $*"
}

expect_usage --size
expect_usage --size 0x0
expect_usage --size 1920x1080junk
expect_usage --size 3840x
expect_usage --size -1x2160
expect_usage --size 999999999999999999999x2160
expect_usage --size 256x256 --size 256x256
expect_usage --size 1920x1080 --size 3840x2160
expect_usage --timing --timing
echo "1..$count"
