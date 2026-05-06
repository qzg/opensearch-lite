#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${MAINSTACK_SEARCH_JS_SMOKE_PORT:-19204}"
SECURE_SMOKE="${MAINSTACK_SEARCH_SECURE_SMOKE:-0}"
if [[ -n "${OPENSEARCH_URL:-}" ]]; then
  URL="${OPENSEARCH_URL}"
elif [[ "${SECURE_SMOKE}" == "1" ]]; then
  URL="https://127.0.0.1:${PORT}"
else
  URL="http://127.0.0.1:${PORT}"
fi
SERVER_PID=""
WORK_DIR="${TMPDIR:-/tmp}/mainstack-search-js-smoke"
SECURITY_DIR=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${WORK_DIR}"
  if [[ -n "${SECURITY_DIR}" ]]; then
    rm -rf "${SECURITY_DIR}"
  fi
}
trap cleanup EXIT

if [[ -z "${OPENSEARCH_PASSWORD:-}" && -n "${OPENSEARCH_PASSWORD_FILE:-}" ]]; then
  OPENSEARCH_PASSWORD="$(tr -d '\r\n' <"${OPENSEARCH_PASSWORD_FILE}")"
  export OPENSEARCH_PASSWORD
fi

if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "node and npm are required for the JavaScript smoke" >&2
  exit 2
fi

if [[ -z "${OPENSEARCH_URL:-}" && "${SECURE_SMOKE}" == "1" ]]; then
  if ! command -v openssl >/dev/null 2>&1; then
    echo "openssl is required for secure local smoke fixtures" >&2
    exit 2
  fi
  SECURITY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mainstack-search-js-security.XXXXXX")"
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
    "${SERVER_ARGS[@]}" >"${TMPDIR:-/tmp}/mainstack-search-js-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 80); do
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

mkdir -p "${WORK_DIR}"
cd "${WORK_DIR}"
npm init -y >/dev/null
npm install --silent @opensearch-project/opensearch@${OPENSEARCH_JS_CLIENT_VERSION:-3}

OPENSEARCH_URL="${URL}" node --input-type=module <<'JS'
import assert from "node:assert/strict";
import fs from "node:fs";
import { Client } from "@opensearch-project/opensearch";

const config = { node: process.env.OPENSEARCH_URL };
if (process.env.OPENSEARCH_USERNAME || process.env.OPENSEARCH_PASSWORD) {
  config.auth = {
    username: process.env.OPENSEARCH_USERNAME ?? "",
    password: process.env.OPENSEARCH_PASSWORD ?? "",
  };
}
if (process.env.OPENSEARCH_URL.startsWith("https://")) {
  const rejectUnauthorized = !["0", "false", "no"].includes(
    (process.env.OPENSEARCH_VERIFY_CERTS ?? "true").toLowerCase(),
  );
  config.ssl = { rejectUnauthorized };
  if (process.env.OPENSEARCH_CA_CERT) {
    config.ssl.ca = fs.readFileSync(process.env.OPENSEARCH_CA_CERT);
  }
}
const client = new Client(config);
const bodyOf = (response) => response.body ?? response;

assert.equal(bodyOf(await client.ping()), true);
await client.indices.create({ index: "js-smoke", body: {} });

const created = bodyOf(await client.index({
  index: "js-smoke",
  id: "1",
  body: { customer_id: "c1", status: "paid", total: 42.5 },
  refresh: true,
}));
assert.ok(["created", "updated"].includes(created.result));

const doc = bodyOf(await client.get({ index: "js-smoke", id: "1" }));
assert.equal(doc._source.customer_id, "c1");

const count = bodyOf(await client.count({
  index: "js-smoke",
  body: { query: { term: { customer_id: "c1" } } },
}));
assert.equal(count.count, 1);

const mget = bodyOf(await client.mget({
  index: "js-smoke",
  body: { ids: ["1"], _source: ["status"] },
}));
assert.deepEqual(mget.docs[0]._source, { status: "paid" });

const search = bodyOf(await client.search({
  index: "js-smoke",
  body: {
    query: { match_all: {} },
    aggs: { by_status: { terms: { field: "status" } } },
  },
}));
assert.equal(search.hits.total.value, 1);
assert.equal(search.aggregations.by_status.buckets[0].key, "paid");

console.log("JavaScript OpenSearch client smoke passed");
JS
