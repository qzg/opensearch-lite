# Kubernetes Security

For workgroup deployments, run OpenSearch Lite with in-process TLS and Basic
authentication even when an ingress, service mesh, or front proxy is also
present. The first security tranche does not include a trusted-proxy bypass.

## Secret Mounts

Create TLS and auth Secrets through your normal secret-management workflow. The
pod expects mounted files:

- `/run/opensearch-lite/tls/tls.crt`
- `/run/opensearch-lite/tls/tls.key`
- `/run/opensearch-lite/tls/ca.crt`
- `/run/opensearch-lite/auth/users.json`

Minimal deployment shape:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensearch-lite
spec:
  replicas: 1
  selector:
    matchLabels:
      app: opensearch-lite
  template:
    metadata:
      labels:
        app: opensearch-lite
    spec:
      containers:
        - name: opensearch-lite
          image: opensearch-lite:latest
          args:
            - --listen
            - 0.0.0.0:9200
            - --allow-nonlocal-listen
            - --tls-cert-file
            - /run/opensearch-lite/tls/tls.crt
            - --tls-key-file
            - /run/opensearch-lite/tls/tls.key
            - --tls-ca-file
            - /run/opensearch-lite/tls/ca.crt
            - --users-file
            - /run/opensearch-lite/auth/users.json
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
              mountPath: /run/opensearch-lite/tls
              readOnly: true
            - name: auth
              mountPath: /run/opensearch-lite/auth
              readOnly: true
      volumes:
        - name: tls
          secret:
            secretName: opensearch-lite-tls
        - name: auth
          secret:
            secretName: opensearch-lite-users
```

Use TCP probes for process reachability. Avoid HTTP probes with inline Basic
credentials in manifests. For deeper validation, run `--validate-config` through
`kubectl exec` so the command reads mounted files inside the pod.

Set `--memory-limit` below the container memory limit. The flag controls the
stored-data budget; the process still needs overhead for HTTP handling, query
evaluation, JSON parsing, TLS, and runtime fallback. On Linux containers,
OpenSearch Lite reads cgroup memory limits when available and fails fast if the
configured data budget or snapshot metadata cannot fit safely. The remediation
message is intended for humans and coding agents: increase pod/container
memory, reduce local data, lower `--memory-limit`, use a smaller data
directory, or move to full OpenSearch locally, server-hosted OpenSearch, or
cloud-hosted OpenSearch.

## Operator Workflow

Validate from inside the pod:

```sh
kubectl exec deploy/opensearch-lite -- \
  opensearch-lite \
    --listen 0.0.0.0:9200 \
    --allow-nonlocal-listen \
    --tls-cert-file /run/opensearch-lite/tls/tls.crt \
    --tls-key-file /run/opensearch-lite/tls/tls.key \
    --tls-ca-file /run/opensearch-lite/tls/ca.crt \
    --users-file /run/opensearch-lite/auth/users.json \
    --memory-limit 384MiB \
    --validate-config
```

Inspect mounted files without printing secrets:

```sh
kubectl exec deploy/opensearch-lite -- sh -lc '
  ls -l /run/opensearch-lite/tls /run/opensearch-lite/auth &&
  test -s /run/opensearch-lite/tls/tls.crt &&
  test -s /run/opensearch-lite/tls/tls.key &&
  test -s /run/opensearch-lite/auth/users.json
'
```

Rotate certificates or users by updating the Secret and restarting the pod. Hot
reload is intentionally deferred.
