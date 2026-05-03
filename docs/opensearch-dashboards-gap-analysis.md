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

This does not yet claim full live OpenSearch Dashboards process compatibility,
but the first Docker-based smoke has now booted Dashboards against
OpenSearch Lite.

## Live Docker Smoke: 2026-04-30

Command shape:

```sh
docker run --rm \
  -p 5601:5601 \
  -e OPENSEARCH_HOSTS='["http://host.docker.internal:9201"]' \
  -e OPENSEARCH_IGNOREVERSIONMISMATCH=true \
  -e DISABLE_SECURITY_DASHBOARDS_PLUGIN=true \
  -e SERVER_HOST=0.0.0.0 \
  opensearchproject/opensearch-dashboards:3.6.0
```

`9201` was a local logging proxy to OpenSearch Lite on `127.0.0.1:9200`.

Result: OpenSearch Dashboards `3.6.0` reached green status with security
disabled. The smoke also passed data-view field discovery, a saved-object
index-pattern create, and a Discover-style `_msearch` through Dashboards'
internal routes.

The first live blocker was `nodes.info`: Dashboards calls
`GET /_nodes?filter_path=nodes.*.version,nodes.*.http.publish_address,nodes.*.ip`
before saved-object migrations. Returning an empty `nodes` map made Dashboards
report `Unable to retrieve version information from OpenSearch nodes.` The
best-effort `nodes.info` response now includes a single local node with
`version`, `ip`, and `http.publish_address`.

Observed OpenSearch API traffic during the successful smoke:

- `GET /_nodes?filter_path=nodes.*.version,nodes.*.http.publish_address,nodes.*.ip`
- `GET /.kibana`
- `GET /_cat/templates/opensearch_dashboards_index_template*?format=json`
- `PUT /.kibana_1`
- `GET /_alias/.kibana`
- `POST /_aliases`
- `GET /.kibana_1/_refresh`
- `GET /_cat/plugins?format=JSON`
- `POST /orders/_field_caps?fields=*&ignore_unavailable=true&allow_no_indices=false`
- `GET /.kibana/_doc/config%3A3.6.0`
- `POST /.kibana/_search?size=1000&from=0&rest_total_hits_as_int=true`
- `PUT /.kibana/_create/config%3A3.6.0?refresh=wait_for`
- `PUT /.kibana/_create/index-pattern%3Aorders?refresh=wait_for`
- `POST /_msearch?ignore_throttled=true&ignore_unavailable=true`

## Live Saved-Object Workflow Smoke: 2026-05-01

The next Docker smoke reused OpenSearch Dashboards `3.6.0`, a local
loopback-rewriting proxy, and the patched OpenSearch Lite binary. Dashboards
again reached green status with saved-object migrations complete.

The smoke then seeded an `orders` index, created saved objects through
Dashboards' HTTP API, exported the resulting data view, saved search,
visualization, and dashboard with `includeReferencesDeep=true`, deleted the
saved-object documents, and imported the export file back through
`POST /api/saved_objects/_import?overwrite=true`.

Result: export returned `exportedCount: 4`, `missingRefCount: 0`, and no
`missingReferences`. Import returned `success: true` and `successCount: 4`.
The live gap found during the first workflow attempt was encoded saved-object
IDs: Dashboards writes path IDs such as `index-pattern%3Aorders`, while
reference lookups use raw IDs such as `index-pattern:orders`. OpenSearch Lite
now percent-decodes document ID path parameters for document, source, and
explain APIs so saved-object reference lookups resolve. Durable startup also
repairs legacy `.kibana*` and `.opensearch_dashboards*` documents that were
stored before this fix with literal encoded IDs.

Observed additional OpenSearch API traffic during this workflow:

- `PUT /.kibana/_create/search%3Apaid-orders-search?refresh=wait_for`
- `PUT /.kibana/_create/visualization%3Aorders-status-vis?refresh=wait_for`
- `PUT /.kibana/_create/dashboard%3Aorders-dashboard?refresh=wait_for`
- `POST /.kibana/_search?size=10000&from=0&rest_total_hits_as_int=true`
- `POST /_mget`
- `POST /_bulk?refresh=wait_for`

The same saved-object import was then run against a durable OpenSearch Lite
data directory. After a clean Lite restart, OpenSearch Dashboards again reached
green status, saved-object migrations completed, and
`GET /api/saved_objects/dashboard/orders-dashboard` returned the imported
dashboard. Restart traffic against existing `.kibana` state added:

- `GET /.kibana`
- `POST /.kibana/_count`
- `GET /.kibana/_doc/dashboard%3Aorders-dashboard`

Checked-in fixtures now preserve the OpenSearch traffic contracts for
overwrite-false import conflicts, `createNewCopies`-style import writes, deep
references after durable replay, and an older `.opensearch_dashboards*`
reindex/alias migration restart.

## Live Import Conflict And Older Migration Smoke: 2026-05-01

The next Docker smoke drove the saved-object conflict paths through Dashboards'
HTTP API instead of only replaying equivalent OpenSearch traffic. It exported a
data view, visualization, and dashboard, then re-imported that NDJSON with
`overwrite=false` and `createNewCopies=true`.

Result: the conflict import returned `success: false` with three conflict
entries, and `createNewCopies=true` returned `success: true` with three
generated destination IDs. The original dashboard remained readable, and the
copied dashboard references were rewritten to the copied visualization and data
view IDs.

The same tranche also started Dashboards with
`opensearchDashboards.index=.opensearch_dashboards` against a durable Lite data
directory seeded with a supported older saved-object index. That live migration
reindexed `.opensearch_dashboards` to `.opensearch_dashboards_1`, migrated the
documents to `.opensearch_dashboards_2`, switched the alias, and reached green
status. `GET /api/saved_objects/index-pattern/orders` returned the migrated
data view with migration version `7.6.0`.

Live migration found two response-shape gaps that were promoted to fixtures:

- Dashboards URL-encodes synthetic task IDs, polling
  `GET /_tasks/opensearch-lite-task%3A1`. `tasks.get` now decodes the path
  parameter before looking up the completed local task.
- Dashboards also URL-encodes path-form scroll IDs and may request one more
  scroll page after the first page already returned all hits. Path-form scroll
  IDs are now decoded, and exhausted scroll contexts survive for one empty page
  instead of returning `search_context_missing_exception`.

Observed additional OpenSearch API traffic during the older-index migration:

- `POST /_reindex?refresh=true&wait_for_completion=false`
- `GET /_tasks/opensearch-lite-task%3A1`
- `POST /.opensearch_dashboards_1/_search?scroll=15m`
- `POST /_bulk`
- `GET /_search/scroll/opensearch-lite-scroll%3A2?scroll=15m`
- `DELETE /_search/scroll/opensearch-lite-scroll%3A2`
- `POST /_aliases`

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

This started as fixture-level compatibility, and Docker smokes have now covered
boot, synthetic data-view and Discover-style route probes, saved-object
export/import conflict modes, durable replay, and an older
`.opensearch_dashboards*` migration. Browser-driven smoke coverage now also
exercises UI-level data-view creation, Discover, visualization, and saved-object
management flows.

## Live Browser Workflow Smoke: 2026-05-01

The browser-driven smoke reused OpenSearch Dashboards `3.6.0`, the local
loopback-rewriting proxy, and durable OpenSearch Lite state under
`opensearchDashboards.index=.opensearch_dashboards`.

Result: Dashboards loaded Home without unhandled startup rejections, created an
`orders` data view through Management, displayed all four seeded `orders`
documents in Discover, created and saved a Data Table visualization showing a
count of `4`, and listed Advanced Settings, the `orders` data view, and the
saved visualization in Saved Objects management.

Live browser traffic found three route/shape gaps that were promoted to local
implementation and tests:

- Dashboards still probes `GET /_plugins/_security/api/account` even with the
  Dashboards security plugin disabled. OpenSearch Lite now returns narrow
  mocked local principal metadata for that exact route while keeping the rest
  of the security namespace closed.
- Dashboards probes `GET /_plugins/_query/_datasources` during startup and
  management page loads. OpenSearch Lite now returns a mocked empty
  direct-query data-source list for that exact read route.
- Index-pattern creation calls `GET /_resolve/index/*` before field caps.
  OpenSearch Lite now returns matching local indices and aliases with
  dot-prefixed local system indices hidden unless `expand_wildcards=all` or
  `hidden` is requested.

Observed additional OpenSearch API traffic during this browser workflow:

- `GET /_plugins/_security/api/account`
- `GET /_plugins/_query/_datasources`
- `GET /_resolve/index/*`
- `POST /orders/_field_caps?fields=*&ignore_unavailable=true&allow_no_indices=false`
- `PUT /.opensearch_dashboards/_create/index-pattern%3A<uuid>?refresh=wait_for`
- `POST /.opensearch_dashboards/_update/config%3A3.6.0?refresh=wait_for&_source_includes=...`
- `POST /orders/_search?ignore_unavailable=true&track_total_hits=true&timeout=30000ms&preference=...`
- `PUT /.opensearch_dashboards/_create/visualization%3A<uuid>?refresh=wait_for`
- `POST /.opensearch_dashboards/_search?size=50&from=0&rest_total_hits_as_int=true`

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

The next saved-object management slice has started with the query forms emitted
by Dashboards `_find`: `simple_query_string` now honors OR vs AND
`default_operator`, wildcard all-field searches, boosted fields such as
`dashboard.title^3`, and `.raw`/`.keyword` multifield fallbacks against the
stored source.

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

Broaden the browser-driven smoke around migration edge cases, dashboard editing,
saved-object relationship/delete/export actions, and plugin-specific
application pages. Capture remaining route/shape failures as source-traceable
fixtures. Likely follow-ups are migration edge-case response shapes,
saved-object repository search variants, and plugin startup metadata that
appears in real traffic.
