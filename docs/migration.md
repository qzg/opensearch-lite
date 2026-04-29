# Migration To Real OpenSearch

OpenSearch Lite is intended for local development. Production and high-fidelity
local testing should use real OpenSearch.

## Before Switching

1. Run the application against OpenSearch Lite with `--strict-compatibility`.
2. Add only deliberate best-effort or fallback routes to `--strict-allowlist`.
3. Run the parity smoke script against real OpenSearch.
4. Review `docs/supported-apis.md` for local approximations.

## Known Local Differences

- Single-node metadata is approximated.
- Lucene analyzers, scoring, segments, shard allocation, and replica behavior
  are not emulated.
- Search uses bounded local scans for supported scalar queries.
- Runtime agent fallback can synthesize read responses that real OpenSearch will
  not synthesize.

Application code that sticks to implemented APIs should move by changing the
endpoint configuration to a real OpenSearch cluster.
