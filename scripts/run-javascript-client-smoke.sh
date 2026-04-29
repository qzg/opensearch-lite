#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${OPENSEARCH_LITE_JS_SMOKE_PORT:-19204}"
URL="${OPENSEARCH_URL:-http://127.0.0.1:${PORT}}"
SERVER_PID=""
WORK_DIR="${TMPDIR:-/tmp}/opensearch-lite-js-smoke"

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "node and npm are required for the JavaScript smoke" >&2
  exit 2
fi

if [[ -z "${OPENSEARCH_URL:-}" ]]; then
  cargo run --manifest-path "${ROOT_DIR}/Cargo.toml" -- \
    --listen "127.0.0.1:${PORT}" \
    --ephemeral >"${TMPDIR:-/tmp}/opensearch-lite-js-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 80); do
    if curl -fsS "${URL}/" >/dev/null 2>&1; then
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
import { Client } from "@opensearch-project/opensearch";

const client = new Client({ node: process.env.OPENSEARCH_URL });
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
