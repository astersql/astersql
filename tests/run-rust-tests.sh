#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

usage() {
    cat <<'EOF'
Usage: ./tests/run-rust-tests.sh <suite> [suite arguments]

Suites:
  local                 Run globalkill, graceshutdown, readonly, llm, and
                        the RealTiKV root parity crate (default).
  globalkill            Run tests/globalkilltest Rust tests.
  graceshutdown         Run tests/graceshutdown Rust tests.
  readonly              Run tests/readonlytest Rust tests.
  llm                   Run tests/llmtest and its Rust subcrates.
  realtikv [root|NAME]  Run the RealTiKV root parity crate or one sub-suite.
  realtikv all          Run every Rust crate under tests/realtikvtest.
  integration ARGS      Run mysql-tester against Rust tidb-server and real TiKV.
  list                  List valid suites and RealTiKV sub-suite names.

Extra arguments after a Cargo suite are forwarded to `cargo test`.
Integration arguments are forwarded to integrationtest/run-rust-tests.sh.
EOF
}

cargo_test() {
    (cd "${REPO_ROOT}" && cargo test --locked "$@")
}

realtikv_package() {
    local name="$1"
    local manifest="${SCRIPT_DIR}/realtikvtest/${name}/Cargo.toml"
    if [[ ! -f "${manifest}" ]]; then
        echo "unknown RealTiKV Rust suite: ${name}" >&2
        list_realtikv >&2
        return 2
    fi
    awk -F'"' '/^name = / { print $2; exit }' "${manifest}"
}

list_realtikv() {
    local manifest
    echo "RealTiKV Rust suites:"
    echo "  root"
    for manifest in "${SCRIPT_DIR}"/realtikvtest/*/Cargo.toml; do
        basename "$(dirname "${manifest}")" | sed 's/^/  /'
    done
}

run_local() {
    cargo_test \
        -p astersql-tests-globalkilltest \
        -p astersql-tests-graceshutdown \
        -p astersql-tests-readonlytest \
        -p astersql-tests-llmtest \
        -p astersql-tests-realtikvtest \
        "$@"
}

suite="${1:-local}"
if [[ "$#" -gt 0 ]]; then
    shift
fi

case "${suite}" in
    -h|--help|help)
        usage
        ;;
    list)
        usage
        echo
        list_realtikv
        ;;
    local)
        run_local "$@"
        ;;
    globalkill)
        cargo_test -p astersql-tests-globalkilltest "$@"
        ;;
    graceshutdown)
        cargo_test -p astersql-tests-graceshutdown "$@"
        ;;
    readonly)
        cargo_test -p astersql-tests-readonlytest "$@"
        ;;
    llm)
        cargo_test \
            -p astersql-tests-llmtest \
            -p astersql-tests-llmtest-generator \
            -p astersql-tests-llmtest-logger \
            -p astersql-tests-llmtest-testcase \
            "$@"
        ;;
    realtikv)
        realtikv_suite="${1:-root}"
        if [[ "$#" -gt 0 ]]; then
            shift
        fi
        case "${realtikv_suite}" in
            root)
                cargo_test -p astersql-tests-realtikvtest "$@"
                ;;
            all)
                realtikv_args=(-p astersql-tests-realtikvtest)
                for manifest in "${SCRIPT_DIR}"/realtikvtest/*/Cargo.toml; do
                    package="$(awk -F'"' '/^name = / { print $2; exit }' "${manifest}")"
                    realtikv_args+=(-p "${package}")
                done
                cargo_test "${realtikv_args[@]}" "$@"
                ;;
            *)
                package="$(realtikv_package "${realtikv_suite}")"
                cargo_test -p "${package}" "$@"
                ;;
        esac
        ;;
    integration)
        exec "${SCRIPT_DIR}/integrationtest/run-rust-tests.sh" "$@"
        ;;
    *)
        echo "unknown Rust test suite: ${suite}" >&2
        usage >&2
        exit 2
        ;;
esac
