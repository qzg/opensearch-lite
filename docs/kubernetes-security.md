# Kubernetes Security

For workgroup deployments, run mainstack-search with in-process TLS and Basic
authentication even when an ingress, service mesh, or front proxy is also
present. The first security tranche does not include a trusted-proxy bypass.

## Secret Mounts

Create TLS and auth Secrets through your normal secret-management workflow. The
pod expects mounted files:

- `/run/mainstack-search/tls/tls.crt`
- `/run/mainstack-search/tls/tls.key`
- `/run/mainstack-search/tls/ca.crt`
- `/run/mainstack-search/auth/users.json`

Minimal deployment shape:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mainstack-search
spec:
  replicas: 1
  selector:
    matchLabels:
      app: mainstack-search
  template:
    metadata:
      labels:
        app: mainstack-search
    spec:
      containers:
        - name: mainstack-search
          image: mainstack-search:latest
          args:
            - --listen
            - 0.0.0.0:9200
            - --allow-nonlocal-listen
            - --tls-cert-file
            - /run/mainstack-search/tls/tls.crt
            - --tls-key-file
            - /run/mainstack-search/tls/tls.key
            - --tls-ca-file
            - /run/mainstack-search/tls/ca.crt
            - --users-file
            - /run/mainstack-search/auth/users.json
            - --memory-limit
            - 384MiB
          resources:
            requests:
              memory: 512Mi
            limits:
              memory: 512Mi
          ports:
            - containerPort: 9200
          livenessProbe:
            tcpSocket:
              port: 9200
          readinessProbe:
            tcpSocket:
              port: 9200
          volumeMounts:
            - name: tls
              mountPath: /run/mainstack-search/tls
              readOnly: true
            - name: auth
              mountPath: /run/mainstack-search/auth
              readOnly: true
      volumes:
        - name: tls
          secret:
            secretName: mainstack-search-tls
        - name: auth
          secret:
            secretName: mainstack-search-users
```

Use TCP probes for process reachability. Avoid HTTP probes with inline Basic
credentials in manifests. For deeper validation, run `--validate-config` through
`kubectl exec` so the command reads mounted files inside the pod.

Set `--memory-limit` below the container memory limit. The flag controls the
stored-data budget; the process still needs overhead for HTTP handling, query
evaluation, JSON parsing, TLS, and runtime fallback. On Linux containers,
mainstack-search reads cgroup memory limits when available and fails fast if the
configured data budget or snapshot metadata cannot fit safely. The remediation
message is intended for humans and coding agents: increase pod/container
memory, reduce local data, lower `--memory-limit`, use a smaller data
directory, or move to full OpenSearch locally, server-hosted OpenSearch, or
cloud-hosted OpenSearch.

## Operator Workflow

Validate from inside the pod:

```sh
kubectl exec deploy/mainstack-search -- \
  mainstack-search \
    --listen 0.0.0.0:9200 \
    --allow-nonlocal-listen \
    --tls-cert-file /run/mainstack-search/tls/tls.crt \
    --tls-key-file /run/mainstack-search/tls/tls.key \
    --tls-ca-file /run/mainstack-search/tls/ca.crt \
    --users-file /run/mainstack-search/auth/users.json \
    --memory-limit 384MiB \
    --validate-config
```

Inspect mounted files without printing secrets:

```sh
kubectl exec deploy/mainstack-search -- sh -lc '
  ls -l /run/mainstack-search/tls /run/mainstack-search/auth &&
  test -s /run/mainstack-search/tls/tls.crt &&
  test -s /run/mainstack-search/tls/tls.key &&
  test -s /run/mainstack-search/auth/users.json
'
```

Rotate certificates or users by updating the Secret and restarting the pod. Hot
reload is intentionally deferred.
