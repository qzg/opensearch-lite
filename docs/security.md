# Security

mainstack-search has three operating postures:

- Loopback local development: HTTP and no authentication are allowed when the
  listener is bound to a loopback address.
- Secured workgroup: non-loopback listeners require TLS plus a users file.
- Explicit insecure non-loopback: cleartext/no-auth non-loopback serving is
  allowed only with `--allow-insecure-non-loopback` and should be limited to
  disposable local container workflows.

This is connection-posture compatibility for development and small workgroups.
It is not full OpenSearch Security plugin parity. Tenants, index-pattern
permissions, document-level security, SAML, OIDC, LDAP, audit logging, and
security management APIs remain unsupported.

## Server Flags

```sh
mainstack-search \
  --listen 0.0.0.0:9200 \
  --allow-nonlocal-listen \
  --tls-cert-file /run/mainstack-search/tls/tls.crt \
  --tls-key-file /run/mainstack-search/tls/tls.key \
  --tls-ca-file /run/mainstack-search/tls/ca.crt \
  --users-file /run/mainstack-search/auth/users.json
```

`--tls-cert-file` and `--tls-key-file` configure the REST server certificate.
`--tls-ca-file` records the CA bundle that clients should trust and is validated
at startup. `--client-cert-ca-file` plus `--require-client-cert` enables mTLS
transport hardening, but client certificates do not create users or roles in
this tranche.

`--memory-limit` is the local stored-data budget, not a hard process RSS cap.
When Linux cgroup memory limits are visible, startup and `--validate-config`
compare that budget with the detected container limit after reserving runtime
overhead. If the configured data budget or snapshot metadata cannot fit safely,
mainstack-search fails before loading the full data set and prints remediation:
increase local/container memory, reduce local data, lower `--memory-limit`, use
a smaller `--data-dir`, or move the workload to full OpenSearch locally,
server-hosted OpenSearch, or cloud-hosted OpenSearch.

Use `--validate-config` to check mounted files without serving traffic:

```sh
mainstack-search \
  --listen 0.0.0.0:9200 \
  --allow-nonlocal-listen \
  --tls-cert-file /run/mainstack-search/tls/tls.crt \
  --tls-key-file /run/mainstack-search/tls/tls.key \
  --tls-ca-file /run/mainstack-search/tls/ca.crt \
  --users-file /run/mainstack-search/auth/users.json \
  --validate-config
```

That command is designed for `docker exec`, `kubectl exec`, and coding-agent
repair loops. It reports missing, unreadable, or malformed mounted inputs
without printing secret file contents. It also prints resource diagnostics:
configured data memory budget, detected container memory limit when available,
reserved overhead, effective safe data budget, and snapshot index/document
metadata when `snapshot.meta.json` exists.

## Users File

The users file is JSON with PHC password hashes:

```json
{
  "users": [
    {
      "username": "alice",
      "password_hash": "$argon2id$v=19$...",
      "roles": ["admin"]
    }
  ]
}
```

Supported roles are:

| Role | Access |
| --- | --- |
| `admin` | All local APIs plus admin/control namespaces. |
| `read_write` | Read APIs and local data mutations. |
| `read_only` | Read APIs, including read APIs that use `POST` such as `_search`, `_count`, `_mget`, and `_msearch`. |

Users must have at least one role. Duplicate usernames, empty usernames,
malformed JSON, and invalid PHC hashes fail startup validation.

## Agent-Friendly Checks

From a running container:

```sh
docker exec mainstack-search \
  mainstack-search \
    --listen 0.0.0.0:9200 \
    --allow-nonlocal-listen \
    --tls-cert-file /run/mainstack-search/tls/tls.crt \
    --tls-key-file /run/mainstack-search/tls/tls.key \
    --tls-ca-file /run/mainstack-search/tls/ca.crt \
    --users-file /run/mainstack-search/auth/users.json \
    --validate-config
```

From Kubernetes:

```sh
kubectl exec deploy/mainstack-search -- \
  mainstack-search \
    --listen 0.0.0.0:9200 \
    --allow-nonlocal-listen \
    --tls-cert-file /run/mainstack-search/tls/tls.crt \
    --tls-key-file /run/mainstack-search/tls/tls.key \
    --tls-ca-file /run/mainstack-search/tls/ca.crt \
    --users-file /run/mainstack-search/auth/users.json \
    --validate-config
```

Use `curl --cacert <ca-file> -u <user>:<password> https://host:9200/` for a
manual connectivity check. Prefer reading the password from a shell variable or
mounted file rather than embedding it in scripts or manifests.
