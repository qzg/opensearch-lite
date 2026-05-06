#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${MAINSTACK_SEARCH_SMOKE_PORT:-19200}"
SECURE_SMOKE="${MAINSTACK_SEARCH_SECURE_SMOKE:-0}"
if [[ -n "${OPENSEARCH_URL:-}" ]]; then
  URL="${OPENSEARCH_URL}"
elif [[ "${SECURE_SMOKE}" == "1" ]]; then
  URL="https://127.0.0.1:${PORT}"
else
  URL="http://127.0.0.1:${PORT}"
fi
SERVER_PID=""
VENV_DIR="${TMPDIR:-/tmp}/mainstack-search-python-smoke-venv"
SECURITY_DIR=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  if [[ -n "${SECURITY_DIR}" ]]; then
    rm -rf "${SECURITY_DIR}"
  fi
}
trap cleanup EXIT

if [[ -z "${OPENSEARCH_PASSWORD:-}" && -n "${OPENSEARCH_PASSWORD_FILE:-}" ]]; then
  OPENSEARCH_PASSWORD="$(tr -d '\r\n' <"${OPENSEARCH_PASSWORD_FILE}")"
  export OPENSEARCH_PASSWORD
fi

if [[ -z "${OPENSEARCH_URL:-}" && "${SECURE_SMOKE}" == "1" ]]; then
  if ! command -v openssl >/dev/null 2>&1; then
    echo "openssl is required for secure local smoke fixtures" >&2
    exit 2
  fi
  SECURITY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mainstack-search-python-security.XXXXXX")"
  openssl req -x509 -newkey rsa:2048 -sha256 -days 1 -nodes \
    -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
    -keyout "${SECURITY_DIR}/key.pem" \
    -out "${SECURITY_DIR}/cert.pem" >/dev/null 2>&1
  cat >"${SECURITY_DIR}/users.json" <<'JSON'
{"users":[{"username":"smoke","password_hash":"$argon2id$v=19$m=19456,t=2,p=1$bWFpbnN0YWNrLXNlYXJjaA$qQUjOHa/zfhUvKG9++ip/V1R8o3/1mvUcgb2W8lwRIU","roles":["admin"]}]}
JSON
  export OPENSEARCH_CA_CERT="${SECURITY_DIR}/cert.pem"
  export OPENSEARCH_USERNAME="smoke"
  export OPENSEARCH_PASSWORD="mainstack-search-smoke-password"
fi

if [[ -z "${OPENSEARCH_URL:-}" ]]; then
  SERVER_ARGS=(
    --listen "127.0.0.1:${PORT}" \
    --ephemeral
  )
  if [[ "${SECURE_SMOKE}" == "1" ]]; then
    SERVER_ARGS+=(
      --tls-cert-file "${SECURITY_DIR}/cert.pem"
      --tls-key-file "${SECURITY_DIR}/key.pem"
      --tls-ca-file "${SECURITY_DIR}/cert.pem"
      --users-file "${SECURITY_DIR}/users.json"
    )
  fi
  cargo run --manifest-path "${ROOT_DIR}/Cargo.toml" -- \
    "${SERVER_ARGS[@]}" >"${TMPDIR:-/tmp}/mainstack-search-python-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 50); do
    CURL_ARGS=(-fsS)
    if [[ -n "${OPENSEARCH_CA_CERT:-}" ]]; then
      CURL_ARGS+=(--cacert "${OPENSEARCH_CA_CERT}")
    fi
    if [[ -n "${OPENSEARCH_USERNAME:-}" || -n "${OPENSEARCH_PASSWORD:-}" ]]; then
      CURL_ARGS+=(-u "${OPENSEARCH_USERNAME:-}:${OPENSEARCH_PASSWORD:-}")
    fi
    if curl "${CURL_ARGS[@]}" "${URL}/" >/dev/null 2>&1; then
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

url = os.environ["OPENSEARCH_URL"]
ca_cert = os.environ.get("OPENSEARCH_CA_CERT")
username = os.environ.get("OPENSEARCH_USERNAME")
password = os.environ.get("OPENSEARCH_PASSWORD")
verify_certs = os.environ.get("OPENSEARCH_VERIFY_CERTS")
if verify_certs is None:
    verify_certs = "true" if url.startswith("https://") else "false"

client = OpenSearch(
    hosts=[url],
    http_auth=(username, password) if username or password else None,
    use_ssl=url.startswith("https://"),
    verify_certs=verify_certs.lower() not in ("0", "false", "no"),
    ca_certs=ca_cert,
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
