# Supported APIs

This file is the human-readable companion to the generated route inventory. The
vendored OpenSearch REST spec is pinned to OpenSearch `3.6.0` under
`vendor/opensearch-rest-api-spec`, and `build.rs` generates the route inventory
from that spec at build time.

The generated inventory recognizes the recent OpenSearch route names and
methods so unsupported APIs can fail by their public API name instead of
collapsing to a generic unknown route. The deterministic local surface is still
smaller than the inventory.

## Deterministic Local Behavior

Access classes are coarse local roles, not OpenSearch Security plugin roles.
`read` requires any configured role, `write` requires `read_write` or `admin`,
and `admin` requires `admin`.

| API | Tier | Access | Notes |
| --- | --- | --- | --- |
| `info`, `ping` | implemented | read | Root info and HEAD/ping-style startup checks. |
| `indices.create`, `indices.delete` | implemented | write | Single-node local catalog mutations. |
| `indices.get`, `indices.exists` | implemented | read | Single-node local catalog lookup and `HEAD /{index}` existence checks. |
| `indices.put_index_template`, `indices.delete_index_template`, `indices.delete_template` | implemented | write | Composable templates are stored as readable JSON; legacy template delete returns an OpenSearch-shaped missing-template error unless a matching local template exists. |
| `indices.get_index_template`, `indices.exists_index_template` | implemented | read | Stored template lookup. |
| `cluster.put_component_template`, `cluster.get_component_template`, `cluster.exists_component_template`, `cluster.delete_component_template` | implemented | admin/read | Component templates are stored as readable registry JSON. |
| `ingest.put_pipeline`, `ingest.get_pipeline`, `ingest.delete_pipeline` | implemented | write/read | Registry storage only; processor execution is not modeled. |
| `search_pipeline.put`, `search_pipeline.get`, `search_pipeline.delete` | implemented | write/read | Registry storage only; pipeline execution is not modeled. |
| `put_script`, `get_script`, `delete_script` | implemented | write/read | Stored script definitions are readable JSON; arbitrary Painless execution remains unsupported. |
| `indices.put_alias`, `indices.delete_alias`, `indices.update_aliases` | implemented | write | `_aliases` and `_alias` support `add`, `remove`, and local `remove_index` action sequences. |
| `indices.get_alias`, `indices.exists_alias` | implemented | read | Alias misses return explicit not-found errors. |
| `indices.get_mapping`, `indices.get_field_mapping`, `indices.get_settings` | implemented | read | Stored as JSON catalog metadata and used by compatibility clients. |
| `indices.put_mapping`, `indices.put_settings` | implemented | write | Stored as JSON catalog metadata. |
| `field_caps` | implemented | read | `GET`/`POST /_field_caps` and `/{index}/_field_caps`; built from explicit mappings and observed local documents. |
| `indices.resolve_index` | implemented | read | `GET /_resolve/index/{name}` lists matching local indices and aliases for Dashboards index-pattern creation; dot-prefixed local system indices are hidden unless `expand_wildcards=all`/`hidden` is requested. |
| `indices.stats`, `cat.indices` | implemented/best-effort | read | Single-node document/store counters for local compatibility checks. |
| `cat.plugins`, `cat.templates` | implemented | read | Deterministic empty local metadata arrays for Dashboards compatibility. |
| `cluster.stats` | implemented | read | Stable single-node development metadata with cluster UUID, node, index, document, and store counters. |
| `index`, `delete`, `update`, `create` | implemented | write | `_create` conflicts on existing IDs; update scripts are unsupported. |
| `get`, `get_source`, `exists_source` | implemented | read | Source retrieval and existence checks. |
| `indices.refresh` | implemented | write | No-op visibility barrier; writes are already visible after commit in the local store. |
| `bulk` | implemented | write | `POST`/`PUT` only; malformed source lines and invalid metadata produce errors without mutation. |
| `search`, `count`, `mget`, `msearch` | implemented | read | In-memory search and read APIs, including read APIs that use `POST`; supports the documented Discover query, saved-object `_find` search fields, first visualization aggregation subset, scalar sort values, and `search_after` cursor paging for `_search` with deterministic tie-breaker sort values when needed. |
| `indices.validate_query`, `indices.analyze`, `explain` | implemented/scaffold | read | Development-scale query validation, simple text analysis, and local evaluator explanation. |
| `scroll`, `clear_scroll` | implemented | read | In-memory process-local scroll cursors for migration-style batched reads; cursors are not durable across restarts. |
| `create_pit`, `get_all_pits`, `delete_pit`, `delete_all_pits` | implemented | read | Process-local PIT contexts with bounded retained frozen database views. `_search` with `pit.id` reads the frozen view and can refresh `pit.keep_alive`; PIT searches include a deterministic `_shard_doc` sort tie-breaker. |
| `reindex`, `tasks.get` | implemented | write/read | Reindex executes synchronously against local data; `wait_for_completion=false` returns a synthetic completed task for polling clients. |
| `delete_by_query`, `update_by_query` | implemented/narrow | write | Query-matched local mutation. `update_by_query` only supports the saved-object namespace/workspace removal scripts used by Dashboards-style clients. |
| `snapshot.get_repository`, `snapshot.create_repository`, `snapshot.delete_repository`, `snapshot.verify_repository`, `snapshot.cleanup_repository`, `snapshot.create`, `snapshot.get`, `snapshot.delete`, `snapshot.restore` | implemented | admin | Local native repository catalog under `--data-dir/repositories` in durable mode; snapshot APIs fail closed under `--ephemeral`. `_all` and `all` are read/list selector tokens, not valid repository or snapshot names for create/delete targets; snapshot names starting with `_` are reserved for API operation tokens. Snapshot restore currently parses the narrow native-local request shape and rejects unsupported options before returning a fail-closed unsupported response; restore execution, clone, status, remote repository plugins, and distributed shard semantics remain unsupported. |

Snapshot restore routes are implemented/admin for authorization and routing
purposes, but execution is intentionally deferred. They parse supported local
request options, reject unsupported restore options explicitly, return
fail-closed unsupported responses, and do not route to runtime fallback or
mutate local state.

The first Dashboards-shaped fixture tranches cover data-view metadata,
Discover-style search, simple visualization aggregations, and saved-object
migration primitives without runtime agent fallback. A first Docker-hosted
OpenSearch Dashboards 3.6.0 startup smoke has also reached green status with
security disabled. Follow-up live smokes created, exported, imported, and
durably replayed saved objects with deep references intact. Checked-in
fixtures cover the OpenSearch traffic for overwrite-false import conflicts,
create-new-copy saved-object imports, and an older
`.opensearch_dashboards*` durable migration restart. A later API-level Docker
smoke also exercised those import-conflict and older-index migration paths
through OpenSearch Dashboards itself, including URL-encoded task and scroll IDs
and exhausted scroll paging. A browser-driven OpenSearch Dashboards 3.6.0 smoke
now covers data-view creation, Discover results, a saved Data Table
visualization, and Saved Objects listing against migrated durable local state.
Full live Dashboards support still requires broader migration and application
edge-case coverage.

### Search And Aggregation Guardrails

Search-shaped requests are bounded before scanning local documents:

- request body: the stricter of `--max-body-size` and the 10 MiB query-body
  default
- result window: `from + size <= --max-result-window`
- query depth: 32
- query clause count: 1024
- `terms` values: 4096
- aggregation depth: 8
- total requested bucket count: 10000

Supported first-tranche query clauses include `match_all`, `bool`, `term`,
`terms`, `exists`, `ids`, simple `match`, `match_phrase_prefix`, `range`,
`simple_query_string`, and limited `nested` object/array traversal. Supported
first-tranche aggregations include `terms`, `date_histogram`, `histogram`,
`range`, `filters`, `missing`, `value_count`, `min`, `max`, `avg`, `sum`,
`cardinality`, `stats`, and `top_hits`.

## Best-Effort Metadata

Best-effort responses are safe single-node approximations and include
compatibility headers:

- `cluster.health`
- `cluster.get_settings`
- `nodes.info`
- `nodes.stats`
- generic `cat.*` routes outside the implemented `cat.indices`, `cat.health`,
  `cat.plugins`, and `cat.templates` subset

## Mocked Local No-Ops

Mocked responses are positive compatibility no-ops for APIs whose distributed
cluster side effects do not exist in mainstack-search. They include compatibility
headers and, where response-shape compatibility allows it, an
`mainstack_search` body field explaining the local behavior and the path to full
OpenSearch when the behavior matters.

Initial mocked families:

- `cluster.allocation_explain`
- `cluster.put_settings`
- `cluster.reroute`
- cluster voting/decommission/weighted-routing control toggles
- `security.account` (`GET /_plugins/_security/api/account`) for local
  Dashboards account metadata only
- `query.datasources` (`GET /_plugins/_query/_datasources`) as an empty
  direct-query data-source list
- `indices.clear_cache`
- `indices.flush`
- `indices.forcemerge`
- `indices.open`
- `indices.upgrade`
- `delete_by_query_rethrottle`
- `reindex_rethrottle`
- `update_by_query_rethrottle`

Strict compatibility mode rejects mocked routes unless they are included in
`--strict-allowlist`.

## Runtime Fallback Eligibility

Only explicitly read-oriented OpenSearch APIs are eligible for runtime agent
fallback. Unknown `GET` requests may still use fallback when configured, but
their context is metadata-only; unknown `POST` requests fail closed. Mutating
APIs outside the deterministic local surface, scripts outside the narrow
saved-object update subset, restore execution and unsupported snapshot
operations such as clone, pipeline execution, task cancellation, and other
write/control routes are never routed to fallback.

Legacy template writes (`indices.put_template`) are identified as
`agent_write_fallback_eligible` so the configured fallback model can translate
legacy request shape into the local registry through the `commit_mutations`
tool. It fails unless write-enabled fallback is explicitly configured and the
caller has sufficient write authorization.

Unsupported routes return structured OpenSearch-shaped errors with recovery
hints for agent callers.
