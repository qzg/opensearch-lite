# OpenSearch YAML Parity Harness

`tests/opensearch_yaml_runner.rs` runs selected upstream OpenSearch REST YAML
fixtures against the in-process OpenSearch Lite router. The goal is to promote
upstream behavior into executable local parity checks without requiring a JVM
OpenSearch server for every test run.

## Runner Scope

The current runner supports the fixture features needed by the selected parity
set:

- Multi-document YAML fixtures with shared `setup` and `teardown` sections.
- `skip` steps for version ranges and feature declarations. The runner executes
  tests when required features are supported and skips tests with unsupported
  runner features.
- `do` requests for core local APIs: bulk, create, delete, get, get source,
  index, count, mget, search, refresh, stats, field mapping, field caps,
  aliases, cat plugins/templates, cluster stats, index templates, and index
  create/get/exists/delete.
- Assertions: `match`, `length`, `is_true`, and `is_false`.
- Common `catch` statuses: bad request, request, missing, conflict, param, and
  regex-style catches mapped to request errors.
- NDJSON bulk bodies expressed as either literal strings or YAML lists.

Unsupported fixture constructs should be added only when a selected upstream
fixture requires them. This keeps the runner small and makes each new runner
feature traceable to a parity case.

## Selected Fixtures

The current selected fixture set covers:

- `bulk/10_basic.yml`
- `bulk/50_refresh.yml`
- `create/10_with_id.yml`
- `delete/10_basic.yml`
- `get/10_basic.yml`
- `get_source/70_source_filtering.yml`
- `get_source/85_source_missing.yml`
- `indices.get_alias/20_empty.yml`
- `indices.get_field_mapping/10_basic.yml`
- `indices.get_field_mapping/50_field_wildcards.yml`
- `indices.get_index_template/10_basic.yml`
- `indices.get_index_template/20_get_missing.yml`
- `indices.exists/10_basic.yml`
- `indices.put_index_template/10_basic.yml`
- `indices.refresh/10_basic.yml`
- `indices.stats/10_index.yml`
- `indices.update_aliases/30_remove_index_and_replace_with_alias.yml`
- `mget/70_source_filtering.yml`
- `search/10_source_filtering.yml`
- `search/20_default_values.yml`
- `search.aggregation/20_terms.yml`
- `update/20_doc_upsert.yml`

The tranche also pins selected upstream fixtures that are useful source anchors
but not yet fully executable through the small runner:

- `field_caps/10_basic.yml`
- `cat.plugins/10_basic.yml`
- `cat.templates/10_basic.yml`
- `cluster.stats/10_basic.yml`

## Known Policy

OpenSearch Lite should match the selected fixture assertions unless a
documented local-only limitation is deliberately accepted. When a fixture
exposes a gap in deterministic core behavior, prefer fixing the server over
weakening the runner. When a fixture depends on production-only behavior,
security plugins, distributed semantics, Lucene scoring/analyzers, or features
outside the local development identity, keep it unselected and document the
reason when it becomes relevant.
