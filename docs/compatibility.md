# Compatibility

OpenSearch Lite targets recent OpenSearch 3.x HTTP JSON/NDJSON APIs. The initial
pinned API reference is OpenSearch `3.6.0`, vendored under
`vendor/opensearch-rest-api-spec`.

## Tiers

- `implemented`: deterministic local behavior backed by the local catalog,
  document store, mutation log, or search evaluator.
- `best_effort`: safe local metadata or status response that approximates
  single-node development behavior.
- `agent_fallback_eligible`: read-style request that may be answered by the
  configured runtime agent fallback.
- `unsupported`: recognized or unknown behavior that should fail rather than
  fake success.
- `outside_product_identity`: behavior that conflicts with the local-only
  development identity.

Best-effort and fallback responses keep normal OpenSearch-shaped JSON bodies.
They add out-of-body compatibility signals such as:

- `x-opensearch-lite-api`
- `x-opensearch-lite-tier`

Use `--strict-compatibility` to make best-effort and fallback responses fail
unless the route appears in `--strict-allowlist`.

## Current Implemented Surface

- Root info: `GET /`, `HEAD /`
- Cluster health metadata: `GET /_cluster/health`
- Selected cat metadata: `GET /_cat/indices`, `GET /_cat/health`
- Index create/get/head/delete
- Index templates
- Aliases
- Document index/create/get/head/update/delete
- Bulk index/create/update/delete
- Scalar search with `match_all`, `term`, `terms`, `range`, `exists`, `ids`,
  simple `match`, and simple `bool`

Unsupported mutating APIs are never routed to runtime fallback.
