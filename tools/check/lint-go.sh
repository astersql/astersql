#!/usr/bin/env bash
# Copyright 2026 AsterSQL.

# Match the root-module lint scope without walking build artifacts. Keep this
# outside a command substitution: an enumeration error must fail make lint.
set -euo pipefail
revive=${1:?usage: lint-go.sh <revive>}
source_list=$(mktemp "${TMPDIR:-/tmp}/astersql-lint-sources.XXXXXX")
module_list=$(mktemp "${TMPDIR:-/tmp}/astersql-lint-modules.XXXXXX")
trap 'rm -f "$source_list" "$module_list"' EXIT

git ls-files -z --cached --others --exclude-standard -- '*.go' > "$source_list"
git ls-files -z --cached --others --exclude-standard -- 'go.mod' '**/go.mod' > "$module_list"
modules=()
while IFS= read -r -d '' file; do
    if [[ "$file" != go.mod ]]; then
        modules+=("${file%/go.mod}")
    fi
done < "$module_list"
files=()
while IFS= read -r -d '' file; do
    case "$file" in
        br/*|cmd/*|dumpling/*|pkg/util/hack/*) continue ;;
    esac
    nested=false
    for module in "${modules[@]}"; do
        if [[ "$file" == "$module/"* ]]; then nested=true; break; fi
    done
    if [[ "$nested" == false ]]; then files+=("$file"); fi
done < "$source_list"
if [[ ${#files[@]} -eq 0 ]]; then
    echo 'lint-go: no Go source files in the root module' >&2
    exit 1
fi
"$revive" -formatter friendly -config tools/check/revive.toml \
    -exclude pkg/util/hack/... -exclude ./pkg/util/hack/... "${files[@]}"
