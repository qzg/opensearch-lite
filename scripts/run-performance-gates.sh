#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cargo bench --manifest-path "${ROOT_DIR}/Cargo.toml" --bench search_scan
