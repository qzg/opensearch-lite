#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BIN_NAME="mainstack-search"
VERSION="${MAINSTACK_SEARCH_VERSION:-$(awk -F' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)}"
if [[ -z "$VERSION" || ! "$VERSION" =~ ^[0-9A-Za-z][0-9A-Za-z._+-]*$ || "$VERSION" == *..* ]]; then
  echo "invalid release version [$VERSION]" >&2
  exit 1
fi

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

cargo build --locked --release --bin "$BIN_NAME"

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BINARY_PATH="$TARGET_DIR/release/$BIN_NAME$EXE_SUFFIX"
PACKAGE_NAME="$BIN_NAME-$VERSION-$OS-$ARCH"
DIST_DIR="dist"
PACKAGE_DIR="$DIST_DIR/$PACKAGE_NAME"
ARCHIVE_PATH="$DIST_DIR/$PACKAGE_NAME.zip"
CHECKSUM_PATH="$ARCHIVE_PATH.sha256"
case "$PACKAGE_DIR:$ARCHIVE_PATH:$CHECKSUM_PATH" in
  "$DIST_DIR"/*:"$DIST_DIR"/*:"$DIST_DIR"/*) ;;
  *)
    echo "release paths must stay under $DIST_DIR" >&2
    exit 1
    ;;
esac

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
if [[ ! "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]]; then
  echo "SOURCE_DATE_EPOCH must be a Unix timestamp" >&2
  exit 1
fi
if NORMALIZED_STAMP="$(date -u -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S 2>/dev/null)"; then
  :
elif NORMALIZED_STAMP="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S 2>/dev/null)"; then
  :
else
  echo "could not convert SOURCE_DATE_EPOCH [$SOURCE_DATE_EPOCH]" >&2
  exit 1
fi

rm -rf "$PACKAGE_DIR" "$ARCHIVE_PATH" "$CHECKSUM_PATH"
mkdir -p "$PACKAGE_DIR"

cp "$BINARY_PATH" "$PACKAGE_DIR/$BIN_NAME$EXE_SUFFIX"
cp README.md "$PACKAGE_DIR/README.md"
cp Cargo.toml "$PACKAGE_DIR/Cargo.toml"
mkdir -p "$PACKAGE_DIR/docs"
cp docs/supported-apis.md "$PACKAGE_DIR/docs/supported-apis.md"
cp docs/compatibility.md "$PACKAGE_DIR/docs/compatibility.md"
cp docs/agent-fallback.md "$PACKAGE_DIR/docs/agent-fallback.md"
cp docs/security.md "$PACKAGE_DIR/docs/security.md"
cp docs/kubernetes-security.md "$PACKAGE_DIR/docs/kubernetes-security.md"
if [ -f LICENSE ]; then
  cp LICENSE "$PACKAGE_DIR/LICENSE"
fi
chmod 755 "$PACKAGE_DIR" "$PACKAGE_DIR/$BIN_NAME$EXE_SUFFIX"
find "$PACKAGE_DIR" -type f ! -name "$BIN_NAME$EXE_SUFFIX" -exec chmod 644 {} +
find "$PACKAGE_DIR" -exec touch -t "$NORMALIZED_STAMP" {} +

(
  cd "$DIST_DIR"
  find "$PACKAGE_NAME" -type f | LC_ALL=C sort | zip -X -q "$PACKAGE_NAME.zip" -@
)

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$ARCHIVE_PATH" > "$CHECKSUM_PATH"
else
  shasum -a 256 "$ARCHIVE_PATH" > "$CHECKSUM_PATH"
fi

echo "$ARCHIVE_PATH"
echo "$CHECKSUM_PATH"
