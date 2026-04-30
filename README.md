# OpenSearch Lite

`opensearch-lite` is a local-only, Rust-based OpenSearch-compatible server for
development stacks. It targets recent OpenSearch 3.x client behavior while using
small, readable local storage and deterministic local implementations for core
index, document, bulk, count, multi-get, multi-search, field discovery,
Discover-style search, and first-tranche visualization aggregation workflows.

It is not a production OpenSearch replacement. Distributed cluster behavior,
Lucene scoring parity, security plugins, and production fault tolerance are out
of scope.

## Quick Start

```sh
cargo run -- --ephemeral
```

The server listens on `127.0.0.1:9200` by default and reports itself as
OpenSearch `3.6.0` compatible.

```sh
curl http://127.0.0.1:9200/
curl -X PUT http://127.0.0.1:9200/orders -H 'content-type: application/json' -d '{}'
curl -X PUT http://127.0.0.1:9200/orders/_doc/1 -H 'content-type: application/json' -d '{"status":"paid","total":42}'
curl -X POST http://127.0.0.1:9200/orders/_search -H 'content-type: application/json' -d '{"query":{"term":{"status":"paid"}}}'
curl -X POST http://127.0.0.1:9200/orders/_count -H 'content-type: application/json' -d '{"query":{"term":{"status":"paid"}}}'
```

## Local Safety

- Loopback binding is the default.
- Non-loopback binding requires `--allow-nonlocal-listen` and, by default, TLS
  plus `--users-file`.
- Cleartext/no-auth non-loopback serving requires the explicit
  `--allow-insecure-non-loopback` development exception.
- Durable mode writes readable JSON/JSONL files under `--data-dir`.
- `--ephemeral` keeps state in memory for disposable runs.
- Strict compatibility mode can fail best-effort and fallback responses during
  CI or migration checks.

The durable files are intentionally agent-readable for local development:
`mutations.jsonl` records committed mutation intent, and `snapshot.json`
contains materialized state when present. Treat them as local data files because
they can contain indexed document content.

## Secured Workgroup Start

```sh
opensearch-lite \
  --listen 0.0.0.0:9200 \
  --allow-nonlocal-listen \
  --tls-cert-file /run/opensearch-lite/tls/tls.crt \
  --tls-key-file /run/opensearch-lite/tls/tls.key \
  --tls-ca-file /run/opensearch-lite/tls/ca.crt \
  --users-file /run/opensearch-lite/auth/users.json
```

Run the same arguments with `--validate-config` from a shell, `docker exec`, or
`kubectl exec` to verify mounted TLS and users files without starting a
listener. See [docs/security.md](docs/security.md) and
[docs/kubernetes-security.md](docs/kubernetes-security.md).

## Verification

```sh
cargo test
cargo test --test python_client_smoke -- --ignored --nocapture
cargo test --test javascript_client_smoke -- --ignored --nocapture
cargo test --test java_client_smoke -- --ignored --nocapture
scripts/run-performance-gates.sh
OPENSEARCH_PARITY_DOCKER=1 scripts/run-opensearch-parity-smoke.sh
```

The ignored client smoke tests invoke the matching scripts under `scripts/`.
Those scripts can also be run directly when debugging a single client.
Set `OPENSEARCH_LITE_SECURE_SMOKE=1` on a smoke script to start a temporary
HTTPS/auth server and prove CA-trusted client connectivity.
Selected upstream REST YAML fixtures run through
`cargo test --test opensearch_yaml_runner`; see `docs/yaml-parity.md` for the
current fixture set and runner policy.
Dashboards-shaped fixture coverage for data-view setup, Discover search, simple
visualization aggregations, and direct durable-file inspection is covered by:

```sh
cargo test --test dashboards_workflow_surface
cargo test --test dashboards_metadata_surface
cargo test --test dashboards_aggregation_surface
cargo test --test durable_agent_read_surface
```

That is fixture-level compatibility, not a live OpenSearch Dashboards support
claim.

The parity smoke can also target an existing OpenSearch 3.x endpoint with
`OPENSEARCH_URL=http://127.0.0.1:9200`.

## Agent Fallback

Read-only runtime agent fallback is disabled unless `--agent-endpoint` is set.
Configured endpoints may receive local read context, including raw indexed
documents relevant to the request. See [docs/agent-fallback.md](docs/agent-fallback.md).

## Documentation

- [Compatibility](docs/compatibility.md)
- [Supported APIs](docs/supported-apis.md)
- [Security](docs/security.md)
- [Kubernetes security](docs/kubernetes-security.md)
- [Agent fallback](docs/agent-fallback.md)
- [Driver examples](docs/driver-examples.md)
- [Migration guidance](docs/migration.md)
- [OpenSearch Dashboards gap analysis](docs/opensearch-dashboards-gap-analysis.md)
