#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only

set -euo pipefail

target_release=${1:?missing target kernel release}
toolchain_stamp=/var/lib/castkms-vm/toolchain-v7
toolchain_manifest=/var/lib/castkms-vm/toolchain-packages.txt
kernel_ready=1
kernel_packages=()
toolchain_packages=(
	alsa-lib-devel
	alsa-utils
	bc
	bison
	curl
	drm_info
	drm-utils
	elfutils-libelf-devel
	flex
	gcc
	kmod
	libdrm-devel
	make
	openssl-devel
	perl-interpreter
	pipewire
	pipewire-devel
	pipewire-utils
	psmisc
	python3
	pkgconf
	rsync
	wireplumber
)

if test ! -e "$toolchain_stamp"; then
	sudo dnf -y -q --setopt=install_weak_deps=False install \
		"${toolchain_packages[@]}"
	sudo mkdir -p "$(dirname -- "$toolchain_stamp")"
	sudo touch "$toolchain_stamp"
fi

rpm -q --whatprovides \
	--qf '%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n' \
	"${toolchain_packages[@]}" | LC_ALL=C sort -u | \
	sudo tee "$toolchain_manifest" >/dev/null

for package in kernel kernel-core kernel-modules-core kernel-modules kernel-modules-internal kernel-devel; do
	kernel_packages+=("$package-$target_release")
	if ! rpm -q "$package-$target_release" >/dev/null 2>&1; then
		kernel_ready=0
	fi
done

if test "$kernel_ready" -eq 0; then
	# Install the exact kernel NEVRAs through DNF so Fedora's repository
	# metadata and package signatures remain part of the trust chain.
	sudo dnf -y -q install "${kernel_packages[@]}"
fi

test -f "/boot/vmlinuz-$target_release"
sudo grubby --set-default "/boot/vmlinuz-$target_release"

printf 'installed_kernel=%s\n' "$target_release"
printf 'running_kernel=%s\n' "$(uname -r)"
printf 'default_kernel=%s\n' "$(sudo grubby --default-kernel)"
printf 'toolchain_manifest=%s\n' "$toolchain_manifest"
