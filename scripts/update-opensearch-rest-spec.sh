#!/usr/bin/env bash
set -euo pipefail

OPENSEARCH_REF="${OPENSEARCH_REF:-3.6.0}"
OPENSEARCH_REPO="${OPENSEARCH_REPO:-../OpenSearch}"
DEST="vendor/opensearch-rest-api-spec"

rm -rf "$DEST"
mkdir -p "$DEST"
git -C "$OPENSEARCH_REPO" archive "$OPENSEARCH_REF" rest-api-spec/src/main/resources \
  | tar -x -C "$DEST" --strip-components=4

echo "Vendored OpenSearch REST spec from $OPENSEARCH_REPO at $OPENSEARCH_REF into $DEST"
