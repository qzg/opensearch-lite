#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LITE_ADDR="${LITE_ADDR:-127.0.0.1:19202}"
LITE_URL="http://${LITE_ADDR}"
REAL_URL="${OPENSEARCH_URL:-}"
DOCKER_CONTAINER=""
LITE_PID=""

cleanup() {
  if [[ -n "${LITE_PID}" ]]; then
    kill "${LITE_PID}" >/dev/null 2>&1 || true
    wait "${LITE_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${DOCKER_CONTAINER}" ]]; then
    docker rm -f "${DOCKER_CONTAINER}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [[ -z "${REAL_URL}" ]]; then
  if [[ "${OPENSEARCH_PARITY_DOCKER:-0}" != "1" ]]; then
    cat >&2 <<'MSG'
Set OPENSEARCH_URL to a real OpenSearch 3.x endpoint, or set
OPENSEARCH_PARITY_DOCKER=1 to start opensearchproject/opensearch:3.6.0 locally.
MSG
    exit 2
  fi
  if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required when OPENSEARCH_PARITY_DOCKER=1" >&2
    exit 2
  fi
  REAL_PORT="${REAL_PORT:-19203}"
  DOCKER_CONTAINER="mainstack-search-parity-$$"
  docker run -d --name "${DOCKER_CONTAINER}" \
    -p "${REAL_PORT}:9200" \
    -e discovery.type=single-node \
    -e plugins.security.disabled=true \
    -e OPENSEARCH_INITIAL_ADMIN_PASSWORD='MainstackSearch1!' \
    opensearchproject/opensearch:3.6.0 >/dev/null
  REAL_URL="http://127.0.0.1:${REAL_PORT}"
fi

cargo build --quiet --manifest-path "${ROOT_DIR}/Cargo.toml"
"${ROOT_DIR}/target/debug/mainstack-search" \
  --ephemeral \
  --listen "${LITE_ADDR}" \
  --max-body-size 32MiB \
  --max-documents 10000 >/tmp/mainstack-search-parity.log 2>&1 &
LITE_PID=$!

python3 - "$LITE_URL" "$REAL_URL" <<'PY'
import json
import sys
import time
import urllib.error
import urllib.request

lite_url, real_url = [arg.rstrip("/") for arg in sys.argv[1:3]]


def request(base, method, path, body=None, content_type="application/json"):
    data = None
    headers = {}
    if body is not None:
        if isinstance(body, str):
            data = body.encode()
            headers["Content-Type"] = content_type
        else:
            data = json.dumps(body).encode()
            headers["Content-Type"] = content_type
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            payload = json.loads(raw) if raw else None
        except json.JSONDecodeError:
            payload = raw.decode(errors="replace")
        return error.code, payload


def wait_for(base):
    deadline = time.time() + 90
    last = None
    while time.time() < deadline:
        try:
            status, _ = request(base, "GET", "/")
            if status == 200:
                return
            last = status
        except Exception as exc:
            last = exc
        time.sleep(1)
    raise AssertionError(f"{base} did not become ready: {last}")


def cleanup(base):
    request(base, "DELETE", "/mainstack-parity")
    request(base, "DELETE", "/mainstack-parity-extra")


def run_flow(base):
    cleanup(base)
    status, _ = request(
        base,
        "PUT",
        "/mainstack-parity",
        {
            "settings": {"number_of_shards": 1, "number_of_replicas": 0},
            "mappings": {
                "properties": {
                    "status": {"type": "keyword"},
                    "total": {"type": "double"},
                    "name": {"type": "text"},
                }
            },
        },
    )
    assert status in (200, 201), (base, "create index", status)

    status, _ = request(
        base,
        "PUT",
        "/mainstack-parity/_mapping",
        {"properties": {"customer_id": {"type": "keyword"}}},
    )
    assert status == 200, (base, "put mapping", status)

    status, _ = request(
        base,
        "POST",
        "/_aliases",
        {"actions": [{"add": {"index": "mainstack-parity", "alias": "mainstack-parity-read"}}]},
    )
    assert status == 200, (base, "add alias", status)

    for doc_id, source in {
        "1": {"status": "paid", "total": 42.5, "name": "Northwind espresso", "customer_id": "c1"},
        "2": {"status": "refunded", "total": 12.0, "name": "Contoso filter", "customer_id": "c2"},
    }.items():
        status, _ = request(base, "PUT", f"/mainstack-parity/_doc/{doc_id}", source)
        assert status in (200, 201), (base, "put doc", doc_id, status)

    status, _ = request(base, "PUT", "/mainstack-parity/_create/1", {"status": "duplicate"})
    assert status == 409, (base, "duplicate create", status)

    bulk = "\n".join(
        [
            '{"index":{"_index":"mainstack-parity","_id":"3"}}',
            '{"status":"paid","total":99.0,"name":"Northwind tamper","customer_id":"c3"}',
            '{"update":{"_index":"mainstack-parity","_id":"2"}}',
            '{"doc":{"status":"paid"}}',
            "",
        ]
    )
    status, body = request(base, "POST", "/_bulk", bulk, "application/x-ndjson")
    assert status == 200 and body["errors"] is False, (base, "bulk", status, body)

    refresh_status, _ = request(base, "POST", "/mainstack-parity/_refresh")
    assert refresh_status == 200, (base, "refresh", refresh_status)

    status, body = request(base, "POST", "/mainstack-parity/_mget", {"ids": ["1", "2", "missing"]})
    assert status == 200, (base, "mget", status)
    assert [doc["found"] for doc in body["docs"]] == [True, True, False], body

    status, body = request(base, "POST", "/mainstack-parity/_mget?_source=status", {"ids": ["1"]})
    assert status == 200, (base, "mget source filter", status)
    assert body["docs"][0]["_source"] == {"status": "paid"}, body

    status, body = request(base, "GET", "/mainstack-parity/_source/1?_source=status")
    assert status == 200, (base, "get source", status)
    assert body == {"status": "paid"}, body

    status, body = request(base, "GET", "/mainstack-parity/_mapping/field/status?include_defaults=true")
    assert status == 200, (base, "field mapping", status)
    assert "mainstack-parity" in body and "mappings" in body["mainstack-parity"], body

    status, body = request(base, "GET", "/mainstack-parity/_stats")
    assert status == 200, (base, "stats", status)
    assert "mainstack-parity" in body["indices"], body
    assert body["indices"]["mainstack-parity"]["primaries"]["docs"]["count"] >= 3, body

    status, body = request(
        base,
        "POST",
        "/mainstack-parity/_update/upserted?_source=status",
        {"doc": {"status": "ignored"}, "upsert": {"status": "from-upsert"}},
    )
    assert status == 201, (base, "update upsert", status)
    assert body["result"] == "created", body
    assert body["get"]["_source"] == {"status": "from-upsert"}, body

    status, body = request(
        base,
        "POST",
        "/mainstack-parity/_count",
        {"query": {"term": {"status": "paid"}}},
    )
    assert status == 200 and body["count"] == 3, (base, "count", status, body)

    status, body = request(
        base,
        "POST",
        "/mainstack-parity-read/_search",
        {"query": {"range": {"total": {"gte": 40}}}, "sort": [{"total": {"order": "asc"}}]},
    )
    assert status == 200, (base, "search", status)
    assert body["hits"]["total"]["value"] == 2, body

    status, body = request(
        base,
        "POST",
        "/mainstack-parity/_search",
        {
            "size": 0,
            "aggs": {
                "by_status": {"terms": {"field": "status"}},
                "total_stats": {"stats": {"field": "total"}},
            },
        },
    )
    assert status == 200, (base, "aggregations", status)
    assert body["aggregations"]["by_status"]["buckets"][0]["key"] == "paid", body
    assert body["aggregations"]["by_status"]["buckets"][0]["doc_count"] == 3, body
    assert body["aggregations"]["total_stats"]["count"] == 3, body

    msearch = "\n".join(
        [
            '{"index":"mainstack-parity"}',
            '{"query":{"term":{"status":"paid"}}}',
            '{"index":"mainstack-parity"}',
            '{"query":{"ids":{"values":["1"]}}}',
            "",
        ]
    )
    status, body = request(base, "POST", "/_msearch", msearch, "application/x-ndjson")
    assert status == 200, (base, "msearch", status)
    assert body["responses"][0]["hits"]["total"]["value"] == 3, body
    assert body["responses"][1]["hits"]["total"]["value"] == 1, body

    status, _ = request(base, "POST", "/missing-parity/_search", {"query": {"match_all": {}}})
    assert status == 404, (base, "missing search", status)

    cleanup(base)


for endpoint in (lite_url, real_url):
    wait_for(endpoint)
    run_flow(endpoint)

print("OpenSearch parity smoke passed")
PY
