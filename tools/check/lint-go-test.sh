#!/usr/bin/env bash
# Copyright 2026 AsterSQL.
set -euo pipefail
runner=${1:-$(cd "$(dirname "$0")" && pwd)/lint-go.sh}
fixture=$(mktemp -d "${TMPDIR:-/tmp}/astersql-lint-test.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/repo" "$fixture/outside"
cd "$fixture/repo"
git init -q
mkdir -p pkg/example pkg/util/hack br cmd dumpling nested target/debug/deps
printf 'module example.com/lint\n' > go.mod
printf 'target/\n' > .gitignore
printf 'module example.com/nested\n' > nested/go.mod
for file in pkg/example/tracked.go pkg/util/hack/unsafe.go br/excluded.go cmd/excluded.go dumpling/excluded.go nested/excluded.go; do
    printf 'package fixture\n' > "$file"
done
git add .
printf 'package fixture\n' > 'pkg/example/untracked space.go'
printf 'package fixture\n' > target/debug/deps/artifact.go
cat > "$fixture/revive" <<'TOOL'
#!/usr/bin/env bash
set -eu
for arg in "$@"; do
    case "$arg" in *.go) printf '%s\n' "$arg" >> "$LINT_CAPTURE_FILE";; esac
done
exit "${LINT_EXIT_CODE:-0}"
TOOL
chmod +x "$fixture/revive"
export LINT_CAPTURE_FILE="$fixture/captured"
"$runner" "$fixture/revive"
LC_ALL=C sort "$LINT_CAPTURE_FILE" > "$fixture/actual"
printf '%s\n' 'pkg/example/tracked.go' 'pkg/example/untracked space.go' > "$fixture/expected"
cmp "$fixture/expected" "$fixture/actual"
# Linter errors and source enumeration errors must propagate to make.
export LINT_EXIT_CODE=7
if "$runner" "$fixture/revive"; then echo 'lost revive failure' >&2; exit 1; else test "$?" -eq 7; fi
cd "$fixture/outside"
if "$runner" "$fixture/revive" 2>"$fixture/git-error"; then echo 'lost git failure' >&2; exit 1; fi
# Empty repositories must not invoke revive's implicit current-directory fallback.
cd "$fixture/repo"
git rm -qr --cached .
rm -rf pkg br cmd dumpling nested target
if "$runner" "$fixture/revive" 2>"$fixture/empty-error"; then echo 'accepted empty source list' >&2; exit 1; fi
grep -q 'no Go source files' "$fixture/empty-error"
printf 'lint-go regression: source scope, spaces, ignored artifacts, nested modules and failure propagation passed\n'
