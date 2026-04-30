# Agent Fallback

Runtime agent fallback is a configured, read-only compatibility mechanism for
unsupported read requests. It is disabled unless `--agent-endpoint` is set.
Fallback runs after authentication and authorization. It cannot answer
unauthenticated or unauthorized requests, and it is denied for security/control
namespaces such as `_plugins/_security`, `_opendistro/_security`, `_security`,
snapshots, and task-control APIs.

## Configuration

```sh
opensearch-lite \
  --agent-endpoint https://example.test/v1/chat/completions \
  --agent-model model-name \
  --agent-token-env OPENAI_API_KEY
```

The endpoint must be OpenAI chat-completions compatible. Local endpoints may use
plain HTTP on loopback addresses. Non-loopback HTTP endpoints are rejected unless
`--agent-allow-insecure-endpoint` is explicitly set.

Bearer tokens must be loaded from an environment variable or a secret file. The
server must not log token material in startup logs, request logs, debug output,
or errors.

## Data Exposure

Once configured, the endpoint is trusted. Fallback context may include:

- the incoming request method/path/query/body
- route tier and compatibility metadata
- index templates, aliases, mappings, and settings
- bounded raw local documents from the target index scope

For known OpenSearch read APIs, the server scopes fallback context to indices
named in the request path or query parameters. Unknown fallback routes receive
metadata only even if the caller supplies `index` or `indices` query parameters.
If a request does not identify a validated target index, fallback receives
metadata only plus omission counts. Cloud-hosted endpoints may still receive
local indexed data for targeted requests. Do not enable fallback for private
data unless that trust boundary is acceptable.

Authorization headers and secret-like query or body fields are redacted before
fallback context is constructed. If a coding agent receives an authn/authz
failure, it should adjust credentials or choose a permitted read API rather
than expecting fallback to complete the request.

## Response Contract

The model must return only a JSON wrapper:

```json
{
  "status": 200,
  "headers": {},
  "body": {},
  "confidence": 90,
  "failure_reason": null,
  "read_only": true
}
```

`body` is the OpenSearch-shaped JSON response returned to the caller after
validation. The server rejects malformed wrappers, low confidence, oversized
responses, write intent, and invalid status values.

Raw documents are serialized as quoted data with stable delimiters and are
treated as untrusted. Document text must not override system or developer
instructions.
