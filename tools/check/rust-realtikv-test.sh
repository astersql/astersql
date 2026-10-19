#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

set -euo pipefail

readonly pd_host="127.0.0.1"
readonly pd_port="12379"
readonly pd_addr="${pd_host}:${pd_port}"
readonly pd_health_url="http://${pd_addr}/pd/api/v1/version"
readonly task_tag="astersql-rust-realtikv-$$"
readonly tag_data_dir="${HOME}/.tiup/data/${task_tag}"
readonly log_dir="${TMPDIR:-/tmp}/astersql-rust-realtikv"
readonly playground_log="${log_dir}/${task_tag}.log"

playground_pid=""
test_pid=""
cleanup_failed=0

usage() {
  echo "usage: $0 <command> [args ...]" >&2
  exit 64
}

find_tiup() {
  if command -v tiup >/dev/null 2>&1; then
    command -v tiup
  elif [[ -x "${HOME}/.tiup/bin/tiup" ]]; then
    printf '%s\n' "${HOME}/.tiup/bin/tiup"
  else
    echo "error: tiup was not found in PATH or ${HOME}/.tiup/bin/tiup" >&2
    return 1
  fi
}

terminate_process() {
  local pid="$1"
  local name="$2"

  if ! kill -0 "${pid}" 2>/dev/null; then
    wait "${pid}" 2>/dev/null || true
    return
  fi

  kill "${pid}" 2>/dev/null || true
  for _ in {1..150}; do
    if ! kill -0 "${pid}" 2>/dev/null; then
      wait "${pid}" 2>/dev/null || true
      return
    fi
    sleep 0.1
  done

  echo "warning: ${name} did not stop after 15 seconds; killing owned process ${pid}" >&2
  kill -KILL "${pid}" 2>/dev/null || true
  wait "${pid}" 2>/dev/null || true
}

cleanup() {
  local command_status=$?
  trap - EXIT INT TERM
  set +e

  if [[ -n "${test_pid}" ]]; then
    terminate_process "${test_pid}" "test command"
  fi
  if [[ -n "${playground_pid}" ]]; then
    terminate_process "${playground_pid}" "TiUP playground"
  fi

  if [[ -n "${tiup_bin:-}" ]]; then
    "${tiup_bin}" clean "${task_tag}" >/dev/null 2>&1 || true
  fi
  case "${tag_data_dir}" in
    "${HOME}/.tiup/data/astersql-rust-realtikv-"*) rm -rf -- "${tag_data_dir}" ;;
    *)
      echo "error: refusing to remove unexpected TiUP data path ${tag_data_dir}" >&2
      cleanup_failed=1
      ;;
  esac
  if [[ -e "${tag_data_dir}" ]]; then
    echo "error: failed to remove TiUP data for owned tag ${task_tag}" >&2
    cleanup_failed=1
  fi

  if curl --silent --fail --max-time 2 "${pd_health_url}" >/dev/null 2>&1; then
    echo "error: owned PD endpoint is still reachable after cleanup: ${pd_health_url}" >&2
    cleanup_failed=1
  else
    echo "RealTiKV cleanup complete: tag=${task_tag}, PD endpoint is unreachable" >&2
  fi

  if [[ ${command_status} -ne 0 ]]; then
    exit "${command_status}"
  fi
  exit "${cleanup_failed}"
}

[[ $# -gt 0 ]] || usage

tiup_bin="$(find_tiup)"
command -v curl >/dev/null 2>&1 || {
  echo "error: curl is required for the PD health probe" >&2
  exit 69
}
command -v nc >/dev/null 2>&1 || {
  echo "error: nc is required for the PD port ownership check" >&2
  exit 69
}

if nc -z "${pd_host}" "${pd_port}" >/dev/null 2>&1; then
  echo "error: ${pd_addr} is already in use; refusing to adopt or clean another cluster" >&2
  exit 69
fi

mkdir -p "${log_dir}"
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "Starting owned TiUP playground: tag=${task_tag}, log=${playground_log}" >&2
"${tiup_bin}" playground --mode tikv-slim --tag "${task_tag}" --pd.port "${pd_port}" \
  >"${playground_log}" 2>&1 &
playground_pid=$!

health_deadline=$((SECONDS + 60))
until curl --silent --fail --max-time 2 "${pd_health_url}" >/dev/null 2>&1; do
  if ! kill -0 "${playground_pid}" 2>/dev/null; then
    echo "error: TiUP playground exited before PD became healthy; log=${playground_log}" >&2
    exit 70
  fi
  if ((SECONDS >= health_deadline)); then
    echo "error: PD did not become healthy within 60 seconds; log=${playground_log}" >&2
    exit 70
  fi
  sleep 1
done

export REAL_TIKV_PD="${pd_addr}"
export ASTERSQL_REALTIKV_TEST_PATH="tikv://${pd_addr}?disableGC=true"
echo "PD is healthy: ${pd_health_url}" >&2

"$@" &
test_pid=$!
set +e
wait "${test_pid}"
command_status=$?
set -e
test_pid=""
exit "${command_status}"
