#!/bin/bash
# Fail when the CodeQL workflow has drifted from the repository it scans.
#
# Usage: ./check-codeql.sh
#
# Checks that default setup is off, that the scan of HEAD succeeded (waiting for
# one still queued or running), that every CodeQL language GitHub detects is
# scanned or listed in UNSCANNED, and that each SARIF filter pattern still
# removes something. Needs gh and python3.
# Run before a release, after pushing HEAD.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(dirname "$SCRIPT_DIR")"
cd "$CRATE_DIR"

WORKFLOW=.github/workflows/codeql.yml
PATTERNS=.github/codeql/sarif-filter.txt
# bench/ holds the only C source.
UNSCANNED=(c-cpp)

failed=0
fail() {
	echo "codeql: $*" >&2
	failed=1
}

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"

state="$(gh api "repos/$repo/code-scanning/default-setup" --jq .state)"
if [ "$state" != not-configured ]; then
	fail "default setup is $state; turn it off, it conflicts with $WORKFLOW"
fi

codeql_language() {
	case "$1" in
	C | C++ | Objective-C) echo c-cpp ;;
	C#) echo csharp ;;
	Go) echo go ;;
	Java | Kotlin) echo java-kotlin ;;
	JavaScript | TypeScript) echo javascript-typescript ;;
	Python) echo python ;;
	Ruby) echo ruby ;;
	Rust) echo rust ;;
	Swift) echo swift ;;
	esac
}

mapfile -t scanned < <(yq '.jobs.analyze.strategy.matrix.language[]' "$WORKFLOW")
detected=()
if [ -n "$(git ls-files .github/workflows)" ]; then
	detected+=(actions)
fi
while read -r name; do
	lang="$(codeql_language "$name")"
	[ -n "$lang" ] && detected+=("$lang")
done < <(gh api "repos/$repo/languages" --jq 'keys[]')

contains() {
	local needle="$1" item
	shift
	for item in "$@"; do
		[ "$item" = "$needle" ] && return 0
	done
	return 1
}

for lang in "${detected[@]}"; do
	if ! contains "$lang" "${scanned[@]}" "${UNSCANNED[@]}"; then
		fail "$lang is in the repository but neither in $WORKFLOW nor UNSCANNED"
	fi
done
for lang in "${UNSCANNED[@]}"; do
	if ! contains "$lang" "${detected[@]}"; then
		fail "$lang is in UNSCANNED but no longer in the repository"
	fi
done

sha="$(git rev-parse HEAD)"
run="$(gh run list --workflow "$(basename "$WORKFLOW")" --commit "$sha" --limit 1 \
	--json databaseId,status,conclusion --jq '.[0] // empty')"
if [ -z "$run" ]; then
	fail "no $WORKFLOW run for HEAD ($sha); push it first"
	exit 1
fi
run_id="$(jq -r .databaseId <<<"$run")"
if [ "$(jq -r .status <<<"$run")" != completed ]; then
	if ! "$SCRIPT_DIR/watch-ci.sh" HEAD "$(basename "$WORKFLOW")"; then
		fail "$WORKFLOW run $run_id for HEAD did not succeed"
		exit 1
	fi
elif [ "$(jq -r .conclusion <<<"$run")" != success ]; then
	fail "$WORKFLOW run $run_id for HEAD is $(jq -r '.status + " " + .conclusion' <<<"$run")"
	exit 1
fi

work="$(mktemp -d "$CRATE_DIR/target/check-codeql.XXXXXX")"
trap 'rm -rf "$work"' EXIT

gh run download "$run_id" --pattern 'codeql-unfiltered-*' --dir "$work/sarif"

# The pinned filter-sarif's own matcher, so a pattern counts exactly as CI applies it.
filter_sha="$(sed -n 's|.*advanced-security/filter-sarif@\([0-9a-f]\{40\}\).*|\1|p' "$WORKFLOW")"
for file in filter_sarif.py globber.py; do
	gh api -H 'Accept: application/vnd.github.raw' \
		"repos/advanced-security/filter-sarif/contents/$file?ref=$filter_sha" >"$work/$file"
done

count_results() {
	jq '[.runs[].results[]] | length' "$1"
}

while read -r pattern; do
	[ -z "$pattern" ] && continue
	removed=0
	for sarif in "$work"/sarif/*/*.sarif; do
		python3 "$work/filter_sarif.py" --input "$sarif" --output "$work/out.sarif" \
			-- "$pattern" >/dev/null
		removed=$((removed + $(count_results "$sarif") - $(count_results "$work/out.sarif")))
	done
	if [ "$removed" -eq 0 ]; then
		fail "pattern $pattern removed nothing in run $run_id; delete it from $PATTERNS"
	else
		echo "$pattern removes $removed result(s)"
	fi
done <"$PATTERNS"

[ "$failed" -eq 0 ] && echo "codeql ok"
exit "$failed"
