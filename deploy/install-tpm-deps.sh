#!/usr/bin/env bash
set -euo pipefail

build=0
case "${1:-}" in
    '') ;;
    --build) build=1 ;;
    *) printf 'Usage: %s [--build]\n' "$0" >&2; exit 2 ;;
esac

# Select packages by distribution family, never by CPU/TPM manufacturer.
if [[ -r /etc/debian_version ]]; then
    packages=(tpm2-tools)
    if (( build )); then packages+=(build-essential pkg-config libssl-dev libtss2-dev); fi
    sudo apt-get update
    sudo apt-get install -y "${packages[@]}"
elif [[ -r /etc/arch-release ]]; then
    packages=(tpm2-tss tpm2-tools openssl)
    if (( build )); then packages+=(base-devel pkgconf); fi
    sudo pacman -S --needed "${packages[@]}"
elif command -v dnf >/dev/null 2>&1; then
    packages=(tpm2-tss tpm2-tools openssl)
    if (( build )); then packages+=(gcc pkgconf-pkg-config openssl-devel tpm2-tss-devel); fi
    sudo dnf install -y "${packages[@]}"
elif command -v zypper >/dev/null 2>&1; then
    packages=(libtss2-esys0 tpm2.0-tools openssl)
    if (( build )); then packages+=(gcc pkg-config libopenssl-devel tpm2-0-tss-devel); fi
    sudo zypper --non-interactive install "${packages[@]}"
else
    printf 'Install tpm2-tools and TPM2 TSS runtime; source builds also need a C compiler, pkg-config, OpenSSL and TPM2 TSS development headers.\n' >&2
    exit 1
fi

if [[ ! -e /dev/tpmrm0 ]]; then
    printf 'No TPM 2.0 resource manager found at /dev/tpmrm0; enable TPM in firmware.\n' >&2
    exit 1
fi
if [[ ! -r /dev/tpmrm0 || ! -w /dev/tpmrm0 ]]; then
    device_group=$(stat -c %G /dev/tpmrm0)
    device_mode=$(stat -c %A /dev/tpmrm0)
    if [[ ( $device_group == tss || $device_group == tpm ) && $device_mode == ????rw* ]] && getent group "$device_group" >/dev/null; then
        sudo usermod -aG "$device_group" "$(id -un)"
        printf 'TPM access group %s enabled; sign out and back in before enrollment.\n' "$device_group"
        exit 1
    else
        printf 'Configure access to /dev/tpmrm0 according to your distribution.\n' >&2
        exit 1
    fi
fi
printf 'TPM runtime installed. A verifiable manufacturer EK certificate is also required.\n'
