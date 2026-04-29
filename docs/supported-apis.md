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

| API | Tier | Notes |
| --- | --- | --- |
| `info`, `ping` | implemented | Root info and HEAD/ping-style startup checks. |
| `indices.create`, `indices.get`, `indices.delete` | implemented | Single-node local catalog behavior. |
| `indices.put_index_template`, `indices.get_index_template`, `indices.delete_index_template` | implemented | Stored as readable JSON in the local catalog. |
| `indices.put_alias`, `indices.get_alias`, `indices.delete_alias`, `_aliases` actions | implemented | Alias misses return explicit not-found errors; `_aliases` supports basic `add` and `remove`. |
| `indices.get_mapping`, `indices.put_mapping`, `indices.get_field_mapping`, `indices.get_settings`, `indices.put_settings` | implemented | Stored as JSON catalog metadata and used by compatibility clients; field mapping supports exact and wildcard field lookup plus basic defaults. |
| `indices.stats`, `cat.indices` | implemented/best-effort | Single-node document/store counters for local compatibility checks; `_stats` filters the locally supported metric groups and rejects unknown metric names. |
| `index`, `get`, `get_source`, `exists_source`, `delete`, `update`, `create` | implemented | `_create` conflicts on existing IDs; update supports `doc`, `doc_as_upsert`, explicit `upsert`, and source-filtered update responses; update scripts are unsupported. |
| `indices.refresh` | implemented | No-op visibility barrier; writes are already visible after commit in the local store. |
| `bulk` | implemented | `POST`/`PUT` only; accepts refresh query parameters as no-ops; malformed source lines, invalid action metadata, and missing index metadata produce errors without mutation. |
| `search`, `count`, `mget`, `msearch` | implemented | Scalar in-memory search for `match_all`, `term`, `terms`, `range`, `exists`, `ids`, simple `match`, `match_phrase`, `match_phrase_prefix`, `prefix`, `wildcard`, and `bool` with basic `minimum_should_match`. Search supports basic `terms`, `min`, `max`, `sum`, `avg`, `value_count`, and `stats` aggregations. `_mget` supports request-level and item-level `_source` filtering. |

## Best-Effort Metadata

Best-effort responses are safe single-node approximations and include
compatibility headers:

- `cluster.health`
- `cluster.get_settings`
- `nodes.info`
- `nodes.stats`
- `cat.*`

## Runtime Fallback Eligibility

Only explicitly read-oriented OpenSearch APIs are eligible for runtime agent
fallback. Unknown `GET` requests may still use fallback when configured, but
their context is metadata-only; unknown `POST` requests fail closed. Mutating
APIs such as `_delete_by_query`, `_update_by_query`, `_reindex`, scripts,
snapshots, pipelines, task cancellation, and other write/control routes are
never routed to fallback.

Unsupported routes return structured OpenSearch-shaped errors with recovery
hints for agent callers.
