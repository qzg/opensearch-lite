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
| `indices.get` | implemented | read | Single-node local catalog lookup. |
| `indices.put_index_template`, `indices.delete_index_template` | implemented | write | Stored as readable JSON in the local catalog. |
| `indices.get_index_template`, `indices.exists_index_template` | implemented | read | Stored template lookup. |
| `indices.put_alias`, `indices.delete_alias`, `_aliases` actions | implemented | write | `_aliases` supports basic `add` and `remove`. |
| `indices.get_alias`, `indices.exists_alias` | implemented | read | Alias misses return explicit not-found errors. |
| `indices.get_mapping`, `indices.get_field_mapping`, `indices.get_settings` | implemented | read | Stored as JSON catalog metadata and used by compatibility clients. |
| `indices.put_mapping`, `indices.put_settings` | implemented | write | Stored as JSON catalog metadata. |
| `indices.stats`, `cat.indices` | implemented/best-effort | read | Single-node document/store counters for local compatibility checks. |
| `index`, `delete`, `update`, `create` | implemented | write | `_create` conflicts on existing IDs; update scripts are unsupported. |
| `get`, `get_source`, `exists_source` | implemented | read | Source retrieval and existence checks. |
| `indices.refresh` | implemented | write | No-op visibility barrier; writes are already visible after commit in the local store. |
| `bulk` | implemented | write | `POST`/`PUT` only; malformed source lines and invalid metadata produce errors without mutation. |
| `search`, `count`, `mget`, `msearch` | implemented | read | Scalar in-memory search and read APIs, including read APIs that use `POST`. |

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
