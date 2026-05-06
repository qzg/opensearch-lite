# Migration To Real OpenSearch

mainstack-search is intended for local development. Production and high-fidelity
local testing should use real OpenSearch.

## Before Switching

1. Run the application against mainstack-search with `--strict-compatibility`.
2. Add only deliberate best-effort or fallback routes to `--strict-allowlist`.
3. Run the parity smoke script against real OpenSearch.
4. Review `docs/supported-apis.md` for local approximations.
5. Use standard HTTPS, Basic auth, and CA trust settings when exercising
   secured workgroup mode.

## Known Local Differences

- Single-node metadata is approximated.
- Lucene analyzers, scoring, segments, shard allocation, and replica behavior
  are not emulated.
- Search uses bounded local scans for supported scalar queries.
- Runtime agent fallback can synthesize read responses that real OpenSearch will
  not synthesize.
- Local roles are coarse development roles. Do not treat `admin`, `read_write`,
  and `read_only` as equivalent to production OpenSearch Security roles.
- OpenSearch Security plugin management APIs are not implemented locally.

Application code that sticks to implemented APIs should move by changing the
endpoint configuration to a real OpenSearch cluster. Application code that uses
standard HTTPS, Basic auth, and CA trust should not need code changes solely for
the connection posture.
