#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${OPENSEARCH_LITE_JAVA_SMOKE_PORT:-19205}"
SECURE_SMOKE="${OPENSEARCH_LITE_SECURE_SMOKE:-0}"
if [[ -n "${OPENSEARCH_URL:-}" ]]; then
  URL="${OPENSEARCH_URL}"
elif [[ "${SECURE_SMOKE}" == "1" ]]; then
  URL="https://127.0.0.1:${PORT}"
else
  URL="http://127.0.0.1:${PORT}"
fi
SERVER_PID=""
SECURITY_DIR=""
USE_DOCKER=0

if ! command -v mvn >/dev/null 2>&1 && command -v docker >/dev/null 2>&1; then
  USE_DOCKER=1
fi

cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  if [[ -n "${SECURITY_DIR}" ]]; then
    rm -rf "${SECURITY_DIR}"
  fi
}
trap cleanup EXIT

if [[ -z "${OPENSEARCH_PASSWORD:-}" && -n "${OPENSEARCH_PASSWORD_FILE:-}" ]]; then
  OPENSEARCH_PASSWORD="$(tr -d '\r\n' <"${OPENSEARCH_PASSWORD_FILE}")"
  export OPENSEARCH_PASSWORD
fi

if [[ -z "${OPENSEARCH_URL:-}" && "${SECURE_SMOKE}" == "1" ]]; then
  if ! command -v openssl >/dev/null 2>&1; then
    echo "openssl is required for secure local smoke fixtures" >&2
    exit 2
  fi
  SECURITY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/opensearch-lite-java-security.XXXXXX")"
  openssl req -x509 -newkey rsa:2048 -sha256 -days 1 -nodes \
    -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,DNS:host.docker.internal,IP:127.0.0.1" \
    -keyout "${SECURITY_DIR}/key.pem" \
    -out "${SECURITY_DIR}/cert.pem" >/dev/null 2>&1
  cat >"${SECURITY_DIR}/users.json" <<'JSON'
{"users":[{"username":"smoke","password_hash":"$argon2id$v=19$m=19456,t=2,p=1$b3BlbnNlYXJjaC1saXRl$yb2+WOV4yTxfqlWaoWwrZM6fZfxVj0LwU8tbuI4UZNM","roles":["admin"]}]}
JSON
  export OPENSEARCH_CA_CERT="${SECURITY_DIR}/cert.pem"
  export OPENSEARCH_USERNAME="smoke"
  export OPENSEARCH_PASSWORD="opensearch-lite-smoke-password"
fi

if [[ -z "${OPENSEARCH_URL:-}" ]]; then
  LISTEN_ADDR="127.0.0.1:${PORT}"
  EXTRA_SERVER_ARGS=()
  if [[ "${USE_DOCKER}" == "1" ]]; then
    LISTEN_ADDR="0.0.0.0:${PORT}"
    EXTRA_SERVER_ARGS=(--allow-nonlocal-listen)
  fi
  if [[ "${SECURE_SMOKE}" == "1" ]]; then
    EXTRA_SERVER_ARGS+=(
      --tls-cert-file "${SECURITY_DIR}/cert.pem"
      --tls-key-file "${SECURITY_DIR}/key.pem"
      --tls-ca-file "${SECURITY_DIR}/cert.pem"
      --users-file "${SECURITY_DIR}/users.json"
    )
  fi
  cargo run --manifest-path "${ROOT_DIR}/Cargo.toml" -- \
    --listen "${LISTEN_ADDR}" \
    --ephemeral \
    "${EXTRA_SERVER_ARGS[@]}" >"${TMPDIR:-/tmp}/opensearch-lite-java-smoke.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 80); do
    CURL_ARGS=(-fsS)
    if [[ -n "${OPENSEARCH_CA_CERT:-}" ]]; then
      CURL_ARGS+=(--cacert "${OPENSEARCH_CA_CERT}")
    fi
    if [[ -n "${OPENSEARCH_USERNAME:-}" || -n "${OPENSEARCH_PASSWORD:-}" ]]; then
      CURL_ARGS+=(-u "${OPENSEARCH_USERNAME:-}:${OPENSEARCH_PASSWORD:-}")
    fi
    if curl "${CURL_ARGS[@]}" "${URL}/" >/dev/null 2>&1; then
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
  DOCKER_ARGS=(
    --rm
    --add-host=host.docker.internal:host-gateway
    -e "OPENSEARCH_URL=${DOCKER_URL}"
  )
  if [[ -n "${OPENSEARCH_USERNAME:-}" ]]; then
    DOCKER_ARGS+=(-e "OPENSEARCH_USERNAME=${OPENSEARCH_USERNAME}")
  fi
  if [[ -n "${OPENSEARCH_PASSWORD:-}" ]]; then
    DOCKER_ARGS+=(-e "OPENSEARCH_PASSWORD=${OPENSEARCH_PASSWORD}")
  fi
  if [[ -n "${OPENSEARCH_CA_CERT:-}" ]]; then
    DOCKER_ARGS+=(-e "OPENSEARCH_CA_CERT=/run/opensearch-lite/ca.pem" -v "${OPENSEARCH_CA_CERT}:/run/opensearch-lite/ca.pem:ro")
  fi
  if [[ -n "${OPENSEARCH_VERIFY_CERTS:-}" ]]; then
    DOCKER_ARGS+=(-e "OPENSEARCH_VERIFY_CERTS=${OPENSEARCH_VERIFY_CERTS}")
  fi
  docker build -q -t opensearch-lite-java-smoke "${ROOT_DIR}/docker/java-smoke" >/dev/null
  docker run "${DOCKER_ARGS[@]}" \
    ${OPENSEARCH_JAVA_CLIENT_VERSION:+-e "OPENSEARCH_JAVA_CLIENT_VERSION=${OPENSEARCH_JAVA_CLIENT_VERSION}"} \
    opensearch-lite-java-smoke
else
  echo "mvn or docker is required for the Java smoke" >&2
  exit 2
fi
