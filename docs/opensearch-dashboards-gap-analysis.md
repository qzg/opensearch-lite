# OpenSearch Dashboards Compatibility Gap Analysis

Reference clone: `../OpenSearch-Dashboards`

- Upstream repository: `https://github.com/opensearch-project/OpenSearch-Dashboards.git`
- Branch: `main`
- Commit: `a30877d7b9c70c896247ca2e8f9e974cb672b1ed`
- Package version at clone time: `3.7.0`

## Purpose

OpenSearch Dashboards is a high-value guide for the next OpenSearch Lite API
tranches because it exercises the APIs that a real OpenSearch application uses
to boot, store saved objects, create data views, import sample data, and run
basic exploration workflows.

This is not a commitment to implement every Dashboards plugin endpoint. The
useful signal is which OpenSearch REST APIs must behave deterministically before
Dashboards can run against OpenSearch Lite without relying on runtime agent
fallback.

## Primary Source Paths

- `../OpenSearch-Dashboards/src/core/server/saved_objects/service/lib/repository_opensearch_client.ts`
- `../OpenSearch-Dashboards/src/core/server/saved_objects/migrations/core/migration_opensearch_client.ts`
- `../OpenSearch-Dashboards/src/core/server/saved_objects/migrations/core/opensearch_index.ts`
- `../OpenSearch-Dashboards/src/core/server/saved_objects/migrations/core/index_migrator.ts`
- `../OpenSearch-Dashboards/src/plugins/data/server/index_patterns/fetcher/lib/opensearch_api.ts`
- `../OpenSearch-Dashboards/src/plugins/data/server/search/routes/call_msearch.ts`
- `../OpenSearch-Dashboards/src/plugins/data_importer/server/routes/import_file.ts`
- `../OpenSearch-Dashboards/src/plugins/data_importer/server/routes/import_text.ts`
- `../OpenSearch-Dashboards/src/plugins/console/server/routes/api/console/proxy/create_handler.ts`
- `../OpenSearch-Dashboards/src/core/server/cross_compatibility/cross_compatibility_service.ts`
- `../OpenSearch-Dashboards/src/plugins/telemetry/server/telemetry_collection/get_cluster_stats.ts`

## APIs Already Mostly Covered

These Dashboards-used APIs already have deterministic local behavior or a close
enough approximation:

- `info`, `ping`
- `indices.create`, `indices.get`, `indices.delete`
- `indices.get_alias`, `indices.exists_alias`, `indices.update_aliases`
- `indices.get_mapping`, `indices.put_mapping`
- `indices.get_settings`, `indices.put_settings`
- `indices.refresh`
- document `create`, `index`, `get`, `delete`, `update`
- `bulk`
- `search`, `count`, `mget`, `msearch`
- `cat.indices`
- `cluster.get_settings`

There are still shape and edge-case gaps inside some of these APIs. In
particular, saved-object migration APIs remain outside the current deterministic
surface and are tracked below.

## Priority 1: Dashboards Boot And Data View Creation

These should be the next implementation tranche because they unblock the most
basic Dashboards connection and data-view workflows.

| API | Why Dashboards Uses It | Current Gap | Recommended Local Behavior |
| --- | --- | --- | --- |
| `indices.exists` | Data importer and setup checks call `HEAD /{index}`. | Implemented in the first fixture tranche. | Classified as `indices.exists`, returns empty `200/404`, and has tests plus selected YAML coverage. |
| `field_caps` | Data view creation asks `/{index}/_field_caps?fields=*` to infer fields. | Implemented in the first fixture tranche. | Builds field capabilities from stored mappings plus observed document values; returns OpenSearch-shaped `fields` by field name/type with `searchable` and `aggregatable`. |
| `cat.plugins` | Core cross-compatibility checks installed OpenSearch plugins. | Implemented in the first fixture tranche. | Returns deterministic empty local plugin metadata for JSON clients. |
| `cluster.stats` | Telemetry requests cluster UUID and cluster metadata. | Implemented in the first fixture tranche. | Returns single-node development metadata with stable `cluster_uuid`, node count, index/doc/store counters. |
| `cat.templates` | Saved-object migration cleanup can list legacy templates. | Implemented in the first fixture tranche. | Returns deterministic empty local template metadata unless legacy templates are later stored. |
| `indices.delete_template` | Saved-object migration cleanup can delete legacy templates. | Implemented in the first fixture tranche. | Returns OpenSearch-shaped missing-template errors for absent legacy templates without fallback. |

### Current Status

The first Dashboards fixture tranche implements deterministic local responses
for `indices.exists`, `field_caps`, `cat.plugins`, `cat.templates`,
`cluster.stats`, legacy template delete misses, and alias `remove_index`.
Coverage is source-traceable and fixture-level only:

- `tests/dashboards_metadata_surface.rs` covers data-view metadata and field
  capability states.
- `tests/dashboards_workflow_surface.rs` covers a synthetic data-view,
  Discover, and visualization workflow without runtime fallback.
- `tests/dashboards_aggregation_surface.rs` covers the first visualization
  aggregation subset.
- `tests/durable_agent_read_surface.rs` proves coding-agent-readable
  `mutations.jsonl` and `snapshot.json` durable state.

This does not yet claim live OpenSearch Dashboards process compatibility.

## Priority 2: Saved Object Migration Compatibility

Dashboards saved-object migrations are the deepest compatibility driver. A fresh
OpenSearch Lite data directory may avoid some migration paths, but development
users will eventually restart with existing `.opensearch_dashboards*` indices or
import saved objects.

| API | Why Dashboards Uses It | Recommended Local Behavior |
| --- | --- | --- |
| `scroll` and `clear_scroll` | Migration code reads existing saved objects in batches. | Implement an in-memory scroll cursor with short TTL. Search with `scroll` returns `_scroll_id`; `scroll` returns subsequent batches; `clear_scroll` acknowledges cleanup. |
| `reindex` and `tasks.get` | Migration converts old concrete indices to aliases and polls async reindex tasks. | Execute reindex synchronously, return a synthetic task id when `wait_for_completion=false`, and keep completed task metadata for `tasks.get`. |
| `delete_by_query` | Migration can delete saved-object types configured for removal. | Support query-matched local deletes with `conflicts=proceed`, `refresh`, and OpenSearch-shaped counters. |
| `update_by_query` | Saved-object namespace/workspace deletion rewrites or deletes matching docs. | Support a narrow safe subset for Dashboards scripts that remove namespace/workspace entries and set `ctx.op = "delete"`. Reject unknown scripts with actionable errors. |
| `indices.update_aliases` `remove_index` | Migration can replace a concrete index with an alias. | Extend alias actions beyond `add`/`remove` to support `remove_index` atomically enough for local use. |

### Current Status

The first saved-object migration slice now has deterministic fixture coverage:

- `scroll` and `clear_scroll` use process-local in-memory cursors for
  migration-style batched reads.
- `reindex` executes synchronously against local data. When
  `wait_for_completion=false`, it returns a synthetic completed task ID that
  `tasks.get` can poll.
- `delete_by_query` shares the bounded query evaluator and commits matching
  local deletes.
- `update_by_query` supports the narrow Dashboards saved-object namespace and
  workspace removal scripts. Other scripts fail with a structured
  `script_exception` and do not mutate state.

This is still fixture-level compatibility. The next confidence step is a live
OpenSearch Dashboards smoke to expose any startup/migration shape gaps that the
source-traceable fixtures did not exercise.

## Priority 3: Saved Object Search DSL

The saved-object repository builds richer queries than the current scalar search
surface. To make Dashboards usable beyond startup, search/count/delete/update by
query should share a common local query evaluator that covers:

- `bool.must`, `bool.filter`, `bool.should`, `minimum_should_match`, `must_not`
- `term` and `terms`
- `exists`
- `match_all`
- `simple_query_string`
- `match_phrase_prefix`
- `range`
- `nested` enough for saved-object references
- `_source` filtering
- `sort`, `from`, `size`, and `track_total_hits`

This should still be an in-memory evaluator. The development-scale data target
does not require Lucene parity, but Dashboards needs the response shape and
basic semantics to be predictable.

## Priority 4: Visualization And Discover Workflows

Basic Discover can work with search hits and source filtering, but dashboards
and visualizations depend on aggregations emitted by the data plugin. The next
useful aggregation subset is:

- bucket aggregations: `terms`, `date_histogram`, `histogram`, `range`,
  `filters`, `missing`
- metric aggregations: `value_count`, `min`, `max`, `avg`, `sum`,
  `cardinality`, `stats`, `top_hits`

Keep this scoped to small in-memory datasets first. Accuracy should be good for
development, but production-grade distributed aggregation behavior is out of
scope.

## Lower Priority Or Plugin-Specific Signals

- Console proxy can send arbitrary OpenSearch paths through
  `client.transport.request`. This cannot define a finite implementation scope;
  it is a reason to keep unsupported errors and hints strong.
- Data source management routes call plugin APIs such as
  `ppl.getDataConnections`, `ppl.modifyDataConnection`, and
  `datasourcemanagement.runDirectQuery`. These are plugin-specific and should
  remain out of the core OpenSearch compatibility tranche unless the target
  bundle enables those Dashboards plugins.
- The archiver package uses snapshot APIs to avoid deleting indices that are
  part of snapshots. This is mostly test/dev tooling for Dashboards itself, not
  a first-pass requirement for running applications against OpenSearch Lite.

## Suggested Next Tranche

Run a live OpenSearch Dashboards smoke against OpenSearch Lite and capture the
remaining route/shape failures as source-traceable fixtures. Likely follow-ups
are migration edge-case response shapes, saved-object repository search
variants, and any plugin startup metadata that appears in real traffic.
