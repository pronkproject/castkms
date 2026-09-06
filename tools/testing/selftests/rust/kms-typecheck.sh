#!/bin/bash
# SPDX-License-Identifier: GPL-2.0
# Compile against a built x86-64 kernel crate, not a userspace API imitation.
# Usage: RUSTUP_TOOLCHAIN=<kernel toolchain> ./kms-typecheck.sh <KBUILD_OUTPUT>
# Build rust/kernel.o first; this test never loads modules or opens a DRM node.
set -euo pipefail
shopt -s nullglob

echo "TAP version 13"

build=${1:-${KBUILD_OUTPUT:-}}
if [[ -z $build || ! -f $build/rust/libkernel.rmeta || ! -f $build/scripts/target.json || ! -f $build/.config ]] ||
   ! grep -qx 'CONFIG_X86_64=y' "$build/.config" ||
   ! grep -qx 'CONFIG_RUST=y' "$build/.config" ||
   ! grep -qx 'CONFIG_DRM=y' "$build/.config"; then
	echo "1..0 # SKIP supply an x86-64 CONFIG_RUST=y CONFIG_DRM=y build with rust/kernel.o"
	exit 4
fi
build=$(realpath "$build")
fixtures=$(dirname "$(realpath "$0")")/kms
cases=("$fixtures"/*.rs)
if ((${#cases[@]} == 0)); then
	echo "Bail out! No KMS type-check fixtures installed"
	exit 1
fi
results=$(mktemp -d "${TMPDIR:-/tmp}/rust-kms-typecheck.XXXXXX")
echo "# Compiler diagnostics: $results"

compile()
{
	RUSTC_BOOTSTRAP=1 "${RUSTC:-rustc}" --edition=2021 --crate-type=rlib \
		--emit=metadata --sysroot=/dev/null -Zunstable-options -Cpanic=abort \
		--target="$build/scripts/target.json" -L"$build/rust" \
		--extern kernel="$build/rust/libkernel.rmeta" "$@"
}

count=0
failed=0
for fixture in "${cases[@]}"; do
	name=$(basename "$fixture" .rs)
	count=$((count + 1))
	pattern=$(sed -n 's@^// error-pattern: @@p' "$fixture")
	if [[ -z $pattern ]]; then
		echo "not ok $count - $name lacks an expected diagnostic"
		failed=1
	elif ! compile "$fixture" -o "$results/$name.rmeta" >"$results/$name.positive.log" 2>&1; then
		echo "not ok $count - $name positive control failed"
		failed=1
	elif compile --cfg negative "$fixture" -o "$results/$name.negative.rmeta" >"$results/$name.negative.log" 2>&1; then
		echo "not ok $count - $name invalid use compiled"
		failed=1
	elif ! grep -Eq "$pattern" "$results/$name.negative.log"; then
		echo "not ok $count - $name failed for an unexpected reason"
		failed=1
	else
		echo "ok $count - $name"
	fi
done
echo "1..$count"
exit "$failed"
