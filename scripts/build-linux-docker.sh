#!/usr/bin/env bash
#
# Build the Linux installers inside a throwaway container.
#
# Tauri links against system webkit2gtk/GTK and its bundler shells out to
# dpkg-deb and linuxdeploy, so Linux artifacts can only be produced on Linux.
# Running that in a container keeps the toolchain - Rust, GTK/WebKit dev
# headers, several hundred megabytes of it - off the host, which may well be a
# machine kept for something else entirely.
#
# Run it from the repository root on any machine with Docker:
#
#     docker run --rm \
#       -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
#       -v "$PWD:/src" -v "$PWD/out:/out" \
#       ubuntu:24.04 bash /src/scripts/build-linux-docker.sh
#
# Artifacts land in ./out.
set -euo pipefail

# Host-side Node, used only to run the build scripts. The Node that gets bundled
# into the installer is pinned separately in scripts/fetch-node.mjs.
NODE_VERSION=v22.23.2

echo "=== system packages ==="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends \
  ca-certificates curl git file xz-utils build-essential pkg-config \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev \
  patchelf desktop-file-utils libfuse2t64 >/dev/null

echo "=== node ${NODE_VERSION} ==="
curl -fsSL "https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz" -o /tmp/node.tar.xz
tar -xf /tmp/node.tar.xz -C /usr/local --strip-components=1
node --version

echo "=== rust ==="
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
  sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path >/dev/null
# shellcheck disable=SC1091
. "$HOME/.cargo/env"
rustc --version

cd /src
echo "=== npm ci ==="
npm ci --no-audit --no-fund

# linuxdeploy and appimagetool are themselves AppImages and want FUSE, which a
# container does not have. This tells them to unpack and run instead.
export APPIMAGE_EXTRACT_AND_RUN=1

echo "=== tauri build ==="
# BUNDLES limits the formats (e.g. BUNDLES=appimage) and VERBOSE=1 lets the
# bundler's own output through - tauri swallows linuxdeploy's errors otherwise,
# leaving only "failed to run linuxdeploy" with no reason attached.
args=()
[ -n "${BUNDLES:-}" ] && args+=(--bundles "$BUNDLES")
[ -n "${VERBOSE:-}" ] && args+=(--verbose)

# Collect whatever was produced even when a later bundler fails: the targets are
# built in sequence, so one broken format must not hide the finished ones.
build_status=0
npx tauri build "${args[@]}" || build_status=$?

echo "=== artifacts ==="
mkdir -p /out
find src-tauri/target/release/bundle -maxdepth 2 -type f \
  \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -exec cp -v {} /out/ \;

# The build ran as root; hand everything back so the host user can use it.
if [ -n "${HOST_UID:-}" ]; then
  chown -R "${HOST_UID}:${HOST_GID:-$HOST_UID}" /out /src 2>/dev/null || true
fi

ls -lh /out

# Report the build's own failure, after the artifacts are safely out.
exit "$build_status"
