#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SERVER_BIN="${RUST_TIDB_SERVER_BIN:-${REPO_ROOT}/target/debug/astersql-cmd-tidb-server}"
PROTOC_BIN="${PROTOC:-/opt/homebrew/opt/protobuf@21/bin/protoc}"

if [[ "$#" -eq 0 ]]; then
    echo "Specify an integrationtest2 case, for example: -t br_integration" >&2
    exit 2
fi

for binary in pd-server tikv-server; do
    if [[ ! -x "${SCRIPT_DIR}/third_bin/${binary}" ]]; then
        echo "Missing ${SCRIPT_DIR}/third_bin/${binary}" >&2
        exit 1
    fi
done

(
    cd "${REPO_ROOT}"
    PROTOC="${PROTOC_BIN}" cargo build -p astersql-cmd-tidb-server --locked
)

runner_options=(-s "${SERVER_BIN}")
if [[ -x "${SCRIPT_DIR}/mysql_tester" ]]; then
    runner_options=(-b n "${runner_options[@]}")
fi
runner_options+=("$@")

cd "${SCRIPT_DIR}"
./run-tests.sh "${runner_options[@]}"
