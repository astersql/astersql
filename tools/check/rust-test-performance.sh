#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

set -uo pipefail

usage() {
  cat <<'EOF'
Usage:
  rust-test-performance.sh [--max-seconds N] [--runs N] [--output FILE] -- NEXTTEST_ARGS...
  rust-test-performance.sh --import-nextest-log FILE [--output FILE]

Runs a serial, warmed cargo-nextest latency audit and writes a machine-readable
TSV report. The command fails for zero selected tests, test failures/timeouts,
or any completed test whose duration exceeds --max-seconds.
EOF
}

die() {
  printf 'rust-test-performance: %s\n' "$*" >&2
  exit 2
}

repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || die 'run from a Git checkout'
cd "$repo_root"

max_seconds=1
runs=3
output=''
import_log=''
while (($# > 0)); do
  case "$1" in
    --max-seconds)
      (($# >= 2)) || die '--max-seconds requires a value'
      max_seconds=$2
      shift 2
      ;;
    --runs)
      (($# >= 2)) || die '--runs requires a value'
      runs=$2
      shift 2
      ;;
    --output)
      (($# >= 2)) || die '--output requires a value'
      output=$2
      shift 2
      ;;
    --import-nextest-log)
      (($# >= 2)) || die '--import-nextest-log requires a value'
      import_log=$2
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

[[ $max_seconds =~ ^([0-9]+([.][0-9]*)?|[.][0-9]+)$ ]] || die '--max-seconds must be a positive number'
awk -v value="$max_seconds" 'BEGIN { exit !(value > 0) }' || die '--max-seconds must be greater than zero'
[[ $runs =~ ^[1-9][0-9]*$ ]] || die '--runs must be a positive integer'

report_dir="$repo_root/target/rust-performance"
mkdir -p "$report_dir"
if [[ -z $output ]]; then
  if [[ -n $import_log ]]; then
    output="$report_dir/source-baseline.tsv"
  else
    output="$report_dir/audit-$(date -u +%Y%m%dT%H%M%SZ)-$$.tsv"
  fi
elif [[ $output != /* ]]; then
  output="$repo_root/$output"
fi
mkdir -p "$(dirname "$output")"

classify_script=$(mktemp "$report_dir/.classify.XXXXXX")
cleanup() {
  rm -f "$classify_script" "${run_log:-}" "${run_tsv:-}" "${run_meta:-}"
}
trap cleanup EXIT HUP INT TERM

cat >"$classify_script" <<'PY'
import json
import re
import sys


def split_binary(binary_id):
    if "::" in binary_id:
        return binary_id.split("::", 1)
    return binary_id, "lib"


def category(package, test):
    value = f"{package} {test}".lower()
    if any(word in value for word in ("realtikv", "ldap", "etcd")):
        return "external-service"
    if any(word in value for word in ("backoff", "retry", "ticker", "timeout", "speed_recorder", "progress")):
        return "fixed-wait"
    if any(word in value for word in ("combination", "concurrency", "matrix", "random", "100010", "allcombinations")):
        return "combination-scale"
    if any(word in value for word in ("parsergen", "manifest", "memory_leak", "concurrent_create", "tracker")):
        return "algorithm"
    return "initialization"


def import_log(path, output):
    status_re = re.compile(r"^\s*(SLOW|TIMEOUT|PASS) \[\s*[>]?[ ]*([0-9.]+)s\].*?\) (\S+) (\S+)\s*$")
    rows = {}
    with open(path, encoding="utf-8", errors="replace") as source:
        for line in source:
            match = status_re.match(line)
            if not match:
                continue
            status, duration, binary_id, test = match.groups()
            key = (binary_id, test)
            if status == "SLOW":
                rows.setdefault(key, [duration, "slow"])
            elif status == "TIMEOUT":
                rows[key] = [duration, "timeout"]
            elif key in rows and rows[key][1] == "slow":
                rows[key][0] = duration
    with open(output, "w", encoding="utf-8") as target:
        target.write("kind\trun\tcrate\tbinary\ttest\tduration_seconds\tstatus\ttest_count\tmax_seconds\tcategory\n")
        for (binary_id, test), (duration, status) in sorted(rows.items()):
            package, binary = split_binary(binary_id)
            target.write(f"source\t0\t{package}\t{binary}\t{test}\t{duration}\t{status}\t\t\t{category(package, test)}\n")
        target.write(f"summary\t0\t\t\t__source_summary__\t\timported\t{len(rows)}\t\t\n")
    print(len(rows))


def elapsed_seconds(value):
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, dict):
        return float(value.get("secs", 0)) + float(value.get("nanos", 0)) / 1_000_000_000
    return 0.0


def event_identity(event):
    binary_id = event.get("binary_id")
    test = event.get("name", "unknown")
    if binary_id:
        package, binary = split_binary(binary_id)
        return package, binary, test
    if "$" in test:
        binary_id, test = test.split("$", 1)
        package, binary = split_binary(binary_id)
        return package, binary, test
    return "unknown", "unknown", test


def parse_run(path, run, output, meta):
    rows = []
    failures = 0
    timeouts = 0
    with open(path, encoding="utf-8", errors="replace") as source:
        for line in source:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("type") != "test":
                continue
            status = event.get("event", "")
            if status not in {"ok", "failed", "timeout"}:
                continue
            package, binary, test = event_identity(event)
            duration = elapsed_seconds(event.get("exec_time", event.get("elapsed", event.get("duration", 0))))
            rows.append((package, binary, test, duration, status))
            failures += status == "failed"
            timeouts += status == "timeout"
    maximum = max((row[3] for row in rows), default=0.0)
    with open(output, "w", encoding="utf-8") as target:
        for package, binary, test, duration, status in rows:
            target.write(f"sample\t{run}\t{package}\t{binary}\t{test}\t{duration:.6f}\t{status}\t\t\t{category(package, test)}\n")
        target.write(f"summary\t{run}\t\t\t__run_summary__\t\tcomplete\t{len(rows)}\t{maximum:.6f}\t\n")
    with open(meta, "w", encoding="utf-8") as target:
        target.write(f"{len(rows)}\t{maximum:.9f}\t{failures}\t{timeouts}\n")


if sys.argv[1] == "import":
    import_log(sys.argv[2], sys.argv[3])
elif sys.argv[1] == "run":
    parse_run(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
PY

if [[ -n $import_log ]]; then
  (($# == 0)) || die 'nextest arguments cannot be combined with --import-nextest-log'
  [[ -f $import_log ]] || die "log not found: $import_log"
  imported=$(python3 "$classify_script" import "$import_log" "$output") || die 'failed to import nextest log'
  [[ $imported == 98 ]] || die "expected 98 source hotspots, found $imported"
  printf 'Imported %s hotspots into %s\n' "$imported" "$output"
  exit 0
fi

(($# > 0)) || die 'pass cargo-nextest selection arguments after --'
nextest_args=("$@")

printf 'kind\trun\tcrate\tbinary\ttest\tduration_seconds\tstatus\ttest_count\tmax_seconds\tcategory\n' >"$output"
printf 'Warming selected tests (compilation only)...\n'
if ! cargo nextest run --profile perf-audit --no-run "${nextest_args[@]}"; then
  printf 'Compilation warmup failed; partial report: %s\n' "$output" >&2
  exit 1
fi

overall_count=0
overall_max=0
audit_failed=0
for run in $(seq 1 "$runs"); do
  run_log=$(mktemp "$report_dir/.run-$run.XXXXXX")
  run_tsv=$(mktemp "$report_dir/.run-$run.XXXXXX")
  run_meta=$(mktemp "$report_dir/.run-$run.XXXXXX")
  printf 'Audit run %s/%s...\n' "$run" "$runs"
  NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 cargo nextest run --profile perf-audit --message-format libtest-json-plus "${nextest_args[@]}" >"$run_log" 2>&1
  cargo_status=$?
  cat "$run_log"
  python3 "$classify_script" run "$run_log" "$run" "$run_tsv" "$run_meta" || die "could not parse run $run"
  cat "$run_tsv" >>"$output"
  IFS=$'\t' read -r test_count run_max failures timeouts <"$run_meta"
  overall_count=$((overall_count + test_count))
  if awk -v left="$run_max" -v right="$overall_max" 'BEGIN { exit !(left > right) }'; then
    overall_max=$run_max
  fi
  if ((cargo_status != 0 || test_count == 0 || failures > 0 || timeouts > 0)); then
    audit_failed=1
  fi
  if awk -v actual="$run_max" -v allowed="$max_seconds" 'BEGIN { exit !(actual > allowed) }'; then
    audit_failed=1
  fi
  rm -f "$run_log" "$run_tsv" "$run_meta"
  run_log=''
  run_tsv=''
  run_meta=''
done

printf 'summary\tall\t\t\t__audit_summary__\t\tcomplete\t%s\t%.6f\t\n' "$overall_count" "$overall_max" >>"$output"
printf 'Audit report: %s (%s samples, max %.6fs, limit %ss)\n' "$output" "$overall_count" "$overall_max" "$max_seconds"
exit "$audit_failed"
