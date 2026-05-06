# Compatibility

mainstack-search targets recent OpenSearch 3.x HTTP JSON/NDJSON APIs. The initial
pinned API reference is OpenSearch `3.6.0`, vendored under
`vendor/opensearch-rest-api-spec`.

## Tiers

- `implemented`: deterministic local behavior backed by the local catalog,
  document store, mutation log, or search evaluator.
- `best_effort`: safe local metadata or status response that approximates
  single-node development behavior.
- `mocked`: recognized API whose production behavior is immaterial in the
  local single-node runtime, answered as a benign positive no-op with an
  explanatory `mainstack_search` body field.
- `agent_fallback_eligible`: read-style request that may be answered by the
  configured runtime agent fallback.
- `agent_write_fallback_eligible`: write-style compatibility route that can
  only run when write-enabled agent fallback is explicitly configured and the
  caller is authorized for the route.
- `unsupported`: recognized or unknown behavior that should fail rather than
  fake success.
- `outside_product_identity`: behavior that conflicts with the local-only
  development identity.

Best-effort and fallback responses keep normal OpenSearch-shaped JSON bodies.
They add out-of-body compatibility signals such as:

- `x-mainstack-search-api`
- `x-mainstack-search-tier`

Use `--strict-compatibility` to make best-effort, mocked, and fallback responses
fail unless the route appears in `--strict-allowlist`.

## Security Compatibility

TLS plus Basic authentication is supported for client connection compatibility.
The local users file provides coarse `admin`, `read_write`, and `read_only`
roles so development and workgroup clients can exercise normal secured
connection settings.

This is not OpenSearch Security plugin parity. Security management APIs,
tenants, document-level security, field-level security, index-pattern
permissions, audit-log management, SAML, OIDC, LDAP, and AWS SigV4 are not
implemented in this tranche. The exact Dashboards account probe
`GET /_plugins/_security/api/account` returns mocked local principal metadata;
other requests under `_plugins/_security`, `_opendistro/_security`,
`_security`, unsupported snapshot subpaths, and task-control namespaces fail
closed instead of reaching runtime fallback. The implemented snapshot repository
management subset is admin-only.

Strict compatibility is evaluated after authentication and authorization.
Security does not make best-effort or fallback routes look implemented.

## Current Local Surface

- Root info: `GET /`, `HEAD /`
- Cluster health metadata: `GET /_cluster/health`
- Cluster stats metadata: `GET /_cluster/stats`
- Node info/stats metadata: `GET /_nodes`, `GET /_nodes/stats`
- Selected cat metadata: `GET /_cat/indices`, `GET /_cat/health`,
  `GET /_cat/plugins`, `GET /_cat/templates`
- Index create/get/exists/delete
- Index resolution for Dashboards data-view creation: `GET /_resolve/index/{name}`
- Composable index templates plus legacy template delete miss behavior
- Aliases, including `_aliases`/`_alias` `add`, `remove`, and `remove_index`
- Document index/create/get/head/update/delete
- Bulk index/create/update/delete
- Field capabilities from mappings and observed documents
- Search/count/msearch with `match_all`, `bool`, `term`, `terms`, `range`,
  `exists`, `ids`, simple `match`, `match_phrase_prefix`,
  `simple_query_string`, limited `nested`, scalar sort values, and `_search`
  `search_after` cursor paging with deterministic tie-breakers for non-unique
  sort values
- First-tranche visualization aggregations: `terms`, `date_histogram`,
  `histogram`, `range`, `filters`, `missing`, `value_count`, `min`, `max`,
  `avg`, `sum`, `cardinality`, `stats`, and `top_hits`
- Process-local scroll and clear-scroll cursors for migration-style batched
  saved-object reads
- Process-local PIT create/list/delete lifecycle APIs with retained frozen
  database views; `_search` with `pit.id` reads the frozen view and refreshes
  `pit.keep_alive` when supplied, including `search_after` over deterministic
  sort values
- Native local snapshot repository management: repository create/get/delete,
  verify/cleanup, and snapshot create/get/delete under `--data-dir/repositories`
  in durable mode; snapshot APIs fail closed under `--ephemeral`; restore is
  intentionally deferred and remains a fail-closed admin route with no state
  mutation
- Reindex with synthetic completed task metadata for `tasks.get`
- Delete by query and narrow saved-object namespace/workspace update by query

Unsupported mutating APIs are never routed to runtime fallback.
Mocked local no-op APIs return 200-series OpenSearch-shaped responses because
the operation has no meaningful single-node effect. Security/control,
unsupported snapshot restore/clone/status APIs, dangling-index, and destructive
filesystem-like APIs still fail closed. PIT lifecycle is implemented as a
read-class search context operation. `_msearch` requests that combine PIT or
`search_after` with newline-delimited sub-searches remain unsupported and fail
closed per item.

## Dashboards Compatibility

The Dashboards claim is deliberately narrow: mainstack-search has a
source-traceable fixture suite for data-view setup, Discover-style searches,
simple visualization aggregations, and first saved-object migration primitives
based on the pinned OpenSearch Dashboards 3.7.0 source signals recorded in
`docs/opensearch-dashboards-gap-analysis.md`.

A first Docker smoke with OpenSearch Dashboards 3.6.0 now reaches green status
with security disabled and has passed synthetic data-view field discovery,
saved-object index-pattern create, and Discover-style `_msearch` route probes.
A follow-up Docker smoke also created saved objects through Dashboards' HTTP API
and exported/imported a data view, saved search, visualization, and dashboard
with deep references intact. A durable restart smoke then replayed that
saved-object state, let Dashboards complete migrations, and read the imported
dashboard through Dashboards' saved-object API. Checked-in fixtures now cover
the corresponding OpenSearch traffic for overwrite-false import conflicts,
create-new-copy saved-object imports, and an older
`.opensearch_dashboards*` reindex/alias migration restart. A later API-level
Docker smoke then exercised those import-conflict and older-index migration
paths through OpenSearch Dashboards itself; it found and fixed URL-encoded
task/scroll path IDs plus exhausted scroll paging after a one-page migration
read. A browser-driven Dashboards smoke now creates a data view through the UI,
loads Discover results, saves a Data Table visualization, and lists those saved
objects in management. This is still not a full live Dashboards support claim;
broader migration and application edge cases remain the next compatibility
boundary.

## Query Guardrails

Search-shaped APIs validate bounded local requests before scanning:

- body bytes: the stricter of `--max-body-size` and 10 MiB
- result window: `--max-result-window`
- query depth: 32
- query clauses: 1024
- `terms` values: 4096
- aggregation depth: 8
- total requested buckets: 10000

Unsupported or over-limit query and aggregation shapes return structured
OpenSearch-shaped errors with hints so an agent caller can adjust the request.

## Durable File Compatibility

Durable mode writes `mutations.jsonl`, `snapshot.json`, and
`snapshot.meta.json` under `--data-dir`. The mutation log records transaction
`begin`/`commit` entries with readable mutation `kind`, index, document ID, and
selected source fields. The snapshot is readable JSON materialized state, and
the metadata file exposes generation, estimated stored bytes, index/document
counts, registry object count, and log high-water information without parsing
all document bodies. Snapshots are dirty-threshold based rather than rewritten
after every write.

OpenSearch-shaped API snapshots are separate local repository artifacts under
`--data-dir/repositories/<repository>/`. Repository generations use
`index.latest` plus readable `index-000001.json`-style manifest files and
content-addressed database blobs. These files are for local development
archives; snapshot APIs are disabled in `--ephemeral` mode and snapshot
restore/status are still unsupported.

PIT contexts are runtime-only and intentionally disappear on process restart.
They retain bounded in-memory database views and are not written to durable
state.

On startup, durable mode also repairs legacy Dashboards saved-object IDs that
were previously stored in encoded path form, such as
`index-pattern%3Aorders`, by renaming them to the decoded OpenSearch document
ID form used by Dashboards reference lookups.

Treat these files as local development artifacts. They may contain document
content; do not mount or expose them across trust boundaries unless that data
exposure is acceptable.
