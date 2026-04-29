#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${OPENSEARCH_LITE_JAVA_SMOKE_PORT:-19205}"
URL="${OPENSEARCH_URL:-http://127.0.0.1:${PORT}}"
SERVER_PID=""
USE_DOCKER=0

if ! command -v mvn >/dev/null 2>&1 && command -v docker >/dev/null 2>&1; then
  USE_DOCKER=1
fi

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

if [[ -z "${OPENSEARCH_URL:-}" ]]; then
  LISTEN_ADDR="127.0.0.1:${PORT}"
  EXTRA_SERVER_ARGS=()
  if [[ "${USE_DOCKER}" == "1" ]]; then
    LISTEN_ADDR="0.0.0.0:${PORT}"
    EXTRA_SERVER_ARGS=(--allow-nonlocal-listen)
  fi
  cargo run --manifest-path "${ROOT_DIR}/Cargo.toml" -- \
    --listen "${LISTEN_ADDR}" \
    --ephemeral \
    "${EXTRA_SERVER_ARGS[@]}" >"${TMPDIR:-/tmp}/opensearch-lite-java-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 80); do
    if curl -fsS "${URL}/" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
fi

if command -v mvn >/dev/null 2>&1; then
  (
    cd "${ROOT_DIR}/docker/java-smoke"
    OPENSEARCH_URL="${URL}" mvn -q ${OPENSEARCH_JAVA_CLIENT_VERSION:+-Dopensearch-java.version="${OPENSEARCH_JAVA_CLIENT_VERSION}"} compile exec:java
  )
elif command -v docker >/dev/null 2>&1; then
  DOCKER_URL="${URL/127.0.0.1/host.docker.internal}"
  DOCKER_URL="${DOCKER_URL/localhost/host.docker.internal}"
  docker build -q -t opensearch-lite-java-smoke "${ROOT_DIR}/docker/java-smoke" >/dev/null
  docker run --rm \
    --add-host=host.docker.internal:host-gateway \
    -e "OPENSEARCH_URL=${DOCKER_URL}" \
    ${OPENSEARCH_JAVA_CLIENT_VERSION:+-e "OPENSEARCH_JAVA_CLIENT_VERSION=${OPENSEARCH_JAVA_CLIENT_VERSION}"} \
    opensearch-lite-java-smoke
else
  echo "mvn or docker is required for the Java smoke" >&2
  exit 2
fi
