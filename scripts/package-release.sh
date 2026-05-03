#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BIN_NAME="opensearch-lite"
VERSION="${OPENSEARCH_LITE_VERSION:-$(awk -F' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)}"

case "$(uname -s)" in
  Darwin) OS="darwin" ;;
  Linux) OS="linux" ;;
  MINGW*|MSYS*|CYGWIN*) OS="windows" ;;
  *) OS="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
esac

case "$(uname -m)" in
  arm64|aarch64) ARCH="arm64" ;;
  x86_64|amd64) ARCH="x86_64" ;;
  *) ARCH="$(uname -m)" ;;
esac

EXE_SUFFIX=""
if [ "$OS" = "windows" ]; then
  EXE_SUFFIX=".exe"
fi

cargo build --release --bin "$BIN_NAME"

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BINARY_PATH="$TARGET_DIR/release/$BIN_NAME$EXE_SUFFIX"
PACKAGE_NAME="$BIN_NAME-$VERSION-$OS-$ARCH"
DIST_DIR="dist"
PACKAGE_DIR="$DIST_DIR/$PACKAGE_NAME"
ARCHIVE_PATH="$DIST_DIR/$PACKAGE_NAME.zip"
CHECKSUM_PATH="$ARCHIVE_PATH.sha256"

rm -rf "$PACKAGE_DIR" "$ARCHIVE_PATH" "$CHECKSUM_PATH"
mkdir -p "$PACKAGE_DIR"

cp "$BINARY_PATH" "$PACKAGE_DIR/$BIN_NAME$EXE_SUFFIX"
cp README.md "$PACKAGE_DIR/README.md"
cp Cargo.toml "$PACKAGE_DIR/Cargo.toml"
if [ -f LICENSE ]; then
  cp LICENSE "$PACKAGE_DIR/LICENSE"
fi

(
  cd "$DIST_DIR"
  zip -qr "$PACKAGE_NAME.zip" "$PACKAGE_NAME"
)

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$ARCHIVE_PATH" > "$CHECKSUM_PATH"
else
  shasum -a 256 "$ARCHIVE_PATH" > "$CHECKSUM_PATH"
fi

echo "$ARCHIVE_PATH"
echo "$CHECKSUM_PATH"
