#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
PD_ADDRESS="${TIKV_PATH:-127.0.0.1:2379}"
PD_HTTP_URL="${PD_HTTP_URL:-http://${PD_ADDRESS}}"
SERVER_BIN="${RUST_TIDB_SERVER_BIN:-${REPO_ROOT}/target/debug/astersql-cmd-tidb-server}"
PROTOC_BIN="${PROTOC:-/opt/homebrew/opt/protobuf@21/bin/protoc}"

if [[ "$#" -eq 0 ]]; then
    echo "Specify an existing integration case, for example: -t select" >&2
    exit 2
fi

if ! curl --silent --show-error --fail "${PD_HTTP_URL}/health" >/dev/null; then
    echo "PD is unavailable at ${PD_HTTP_URL}" >&2
    echo "Start PD and TiKV before running this test." >&2
    exit 1
fi

stores="$(curl --silent --show-error --fail "${PD_HTTP_URL}/pd/api/v1/stores")"
if [[ "${stores}" != *'"state_name":"Up"'* && "${stores}" != *'"state_name": "Up"'* ]]; then
    echo "PD does not report a TiKV store in Up state" >&2
    exit 1
fi

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
TIDB_TEST_STORE_NAME=tikv \
TIKV_PATH="${PD_ADDRESS}" \
./run-tests.sh "${runner_options[@]}"
