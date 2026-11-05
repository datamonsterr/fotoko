#!/usr/bin/env bash
# Cross-build fotoko-pi-agent (release) for aarch64-unknown-linux-gnu.
#
# Toolchain choice, first match wins:
#   1. cargo-zigbuild + zig  (target is pinned to glibc 2.36 = Debian bookworm, runs on trixie too)
#   2. aarch64-linux-gnu-gcc (apt: gcc-aarch64-linux-gnu) as the cargo linker
# ring (via rustls) compiles C, so a plain rustup target is not enough.
#
# Output: deploy/pi/dist/fotoko-pi-agent and fotoko-pi-agent.sha256
set -euo pipefail

TARGET=aarch64-unknown-linux-gnu
GLIBC=${FOTOKO_GLIBC:-2.36}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)
dist="$here/dist"

cd "$repo"
rustup target add "$TARGET" >/dev/null 2>&1 || true

if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
  echo "build: cargo zigbuild ($TARGET.$GLIBC)"
  cargo zigbuild --release --locked -p fotoko-pi-agent --target "$TARGET.$GLIBC"
elif command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
  echo "build: cargo + aarch64-linux-gnu-gcc"
  echo "warning: this links against the HOST distro's cross glibc, which may be newer than the Pi's (bookworm: 2.36)." >&2
  echo "         After the build check: objdump -T dist/fotoko-pi-agent | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1" >&2
  echo "         Prefer cargo-zigbuild, which pins glibc." >&2
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
  export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
  cargo build --release --locked -p fotoko-pi-agent --target "$TARGET"
else
  cat >&2 <<'MSG'
build.sh: no aarch64 C toolchain found. Install one of:
  pip install ziglang && cargo install --locked cargo-zigbuild   (preferred)
  sudo apt install gcc-aarch64-linux-gnu                          (Debian/Ubuntu host)
MSG
  exit 2
fi

target_dir=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
bin="$target_dir/$TARGET/release/fotoko-pi-agent"
[ -f "$bin" ] || { echo "build.sh: missing $bin" >&2; exit 1; }

mkdir -p "$dist"
cp "$bin" "$dist/fotoko-pi-agent"
(cd "$dist" && sha256sum fotoko-pi-agent > fotoko-pi-agent.sha256)
echo "built: $dist/fotoko-pi-agent"
cat "$dist/fotoko-pi-agent.sha256"
