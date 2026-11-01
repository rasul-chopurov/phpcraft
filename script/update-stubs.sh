#!/bin/sh
# Refreshes crates/php-index/stubs.bin.gz from JetBrains phpstorm-stubs (Apache-2.0): the
# signatures and docs of PHP's built-in classes and functions.
#   ./script/update-stubs.sh [git ref]
set -e
cd "$(dirname "$0")/.."
REF=${1:-master}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
git clone -q --depth 1 --branch "$REF" https://github.com/JetBrains/phpstorm-stubs.git "$TMP/stubs"
VERSION=$(git -C "$TMP/stubs" rev-parse --short HEAD)
cargo run -q -p php-index --example pack_stubs -- "$TMP/stubs" crates/php-index/stubs.bin.gz "$VERSION"
cp "$TMP/stubs/LICENSE" crates/php-index/STUBS-LICENSE
echo "phpstorm-stubs $VERSION packed"
