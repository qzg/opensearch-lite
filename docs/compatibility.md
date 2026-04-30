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

## Security Compatibility

TLS plus Basic authentication is supported for client connection compatibility.
The local users file provides coarse `admin`, `read_write`, and `read_only`
roles so development and workgroup clients can exercise normal secured
connection settings.

This is not OpenSearch Security plugin parity. Security management APIs,
tenants, document-level security, field-level security, index-pattern
permissions, audit-log management, SAML, OIDC, LDAP, and AWS SigV4 are not
implemented in this tranche. Requests under `_plugins/_security`,
`_opendistro/_security`, `_security`, snapshots, and task-control namespaces
fail closed instead of reaching runtime fallback.

Strict compatibility is evaluated after authentication and authorization.
Security does not make best-effort or fallback routes look implemented.

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
