#!/usr/bin/env bash
# Archive a native Linux build with the admin UI and license files.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <binary> <architecture>" >&2
    exit 2
fi

binary="$1"
arch="$2"
case "${arch}:$(uname -m)" in
    amd64:x86_64 | arm64:aarch64 | arm64:arm64) ;;
    *)
        echo "runner architecture $(uname -m) does not match requested architecture ${arch}" >&2
        exit 1
        ;;
esac

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
tag="${GITHUB_REF_NAME:-v${version}}"
if [[ "$tag" != "v${version}" ]]; then
    echo "release tag ${tag} does not match Cargo.toml version v${version}" >&2
    exit 1
fi

if [[ ! -x "$binary" ]]; then
    echo "release binary not found: ${binary}" >&2
    exit 1
fi

package="raahi-${tag}-linux-${arch}"
out="dist/${package}"
rm -rf "$out"
mkdir -p "$out/ui"

cp "$binary" "$out/raahi"
chmod 0755 "$out/raahi"
cp -R ui/build "$out/ui/build"
cp README.md LICENSE "$out/"

tar -czf "dist/${package}.tar.gz" -C dist "$package"
rm -rf "$out"

file "dist/${package}.tar.gz"
ls -lh "dist/${package}.tar.gz"
