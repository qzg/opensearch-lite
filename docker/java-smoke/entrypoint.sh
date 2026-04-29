#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${OPENSEARCH_JAVA_CLIENT_VERSION:-}" ]]; then
  VERSION_ARG="-Dopensearch-java.version=${OPENSEARCH_JAVA_CLIENT_VERSION}"
else
  VERSION_ARG=""
fi

mvn -q ${VERSION_ARG} compile exec:java
