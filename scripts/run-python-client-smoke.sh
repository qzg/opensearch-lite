#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${OPENSEARCH_LITE_SMOKE_PORT:-19200}"
URL="${OPENSEARCH_URL:-http://127.0.0.1:${PORT}}"
SERVER_PID=""
VENV_DIR="${TMPDIR:-/tmp}/opensearch-lite-python-smoke-venv"

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

if [[ -z "${OPENSEARCH_URL:-}" ]]; then
  cargo run --manifest-path "${ROOT_DIR}/Cargo.toml" -- \
    --listen "127.0.0.1:${PORT}" \
    --ephemeral >"${TMPDIR:-/tmp}/opensearch-lite-python-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 50); do
    if curl -fsS "${URL}/" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
fi

python3 -m venv "${VENV_DIR}"
"${VENV_DIR}/bin/python" -m pip install --quiet --upgrade pip
"${VENV_DIR}/bin/python" -m pip install --quiet 'opensearch-py>=2,<3'

OPENSEARCH_URL="${URL}" "${VENV_DIR}/bin/python" <<'PY'
import os
from opensearchpy import OpenSearch

client = OpenSearch(
    hosts=[os.environ["OPENSEARCH_URL"]],
    use_ssl=False,
    verify_certs=False,
)

assert client.ping()
client.indices.create(index="python-smoke", body={})

created = client.index(
    index="python-smoke",
    id="1",
    body={"customer_id": "c1", "status": "paid", "total": 42.5},
    refresh=True,
)
assert created["result"] in ("created", "updated")

doc = client.get(index="python-smoke", id="1")
assert doc["_source"]["customer_id"] == "c1"

results = client.search(
    index="python-smoke",
    body={"query": {"term": {"customer_id": "c1"}}},
)
assert results["hits"]["total"]["value"] == 1

try:
    client.create(index="python-smoke", id="1", body={"customer_id": "c2"})
except Exception as exc:
    status = getattr(exc, "status_code", None)
    assert status == 409, exc
else:
    raise AssertionError("duplicate create should conflict")

print("Python OpenSearch client smoke passed")
PY
