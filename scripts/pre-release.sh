#!/bin/bash
# Everything that must pass before tagging and publishing a release.
#
# Usage: ./pre-release.sh
#
# Requires a live FreeSWITCH ESL listener on at least one of LIVE_ESL_PORTS
# (default 8022) for the live suites, the
# x86_64-pc-windows-msvc target for the cross-check, rustup for the MSRV
# toolchain, cargo-semver-checks, and gh with HEAD pushed and scanned by CodeQL.
# Traced (set -x) so a failure names the gate that stopped it.

set -euxo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(dirname "$SCRIPT_DIR")"
cd "$CRATE_DIR"

"$SCRIPT_DIR/fmt-workspace.sh"
"$SCRIPT_DIR/check-feature-matrix.sh"
"$SCRIPT_DIR/check-msrv.sh"
cargo clippy --workspace --release --all-features -- -D warnings

# The slowest gates spend their time running tests or waiting on GitHub, not
# compiling, so they overlap; each log prints whole once all have finished.
run_concurrently() {
	local logs names=() pids=() failed=() i
	logs="$(mktemp -d "$CRATE_DIR/target/pre-release.XXXXXX")"
	while [ "$#" -gt 0 ]; do
		names+=("$1")
		bash -c "$2" >"$logs/${#names[@]}.log" 2>&1 &
		pids+=("$!")
		shift 2
	done

	set +x
	for i in "${!names[@]}"; do
		wait "${pids[$i]}" || failed+=("${names[$i]}")
	done
	for i in "${!names[@]}"; do
		printf '\n=== %s ===\n' "${names[$i]}"
		# Expected once cargo runs overlap.
		grep -v '^ *Blocking waiting for file lock on ' "$logs/$((i + 1)).log" || [ "$?" -eq 1 ]
	done
	rm -rf "$logs"
	set -x

	if [ "${#failed[@]}" -gt 0 ]; then
		echo "failed: ${failed[*]}" >&2
		return 1
	fi
}

# One live run per switch listening on LIVE_ESL_PORTS, so two FreeSWITCH trees are
# measured side by side; a port with nothing on it is skipped, all of them failing.
live_gates=()
for port in ${LIVE_ESL_PORTS:-8022}; do
	if [ -n "$(ss -Htln "sport = :$port")" ]; then
		live_gates+=("live-$port" "ESL_PORT=$port cargo test --release --test 'live_*' -- --ignored")
	else
		echo "no ESL listener on port $port, its live run is skipped" >&2
	fi
done
if [ "${#live_gates[@]}" -eq 0 ]; then
	echo "no ESL listener on any of: ${LIVE_ESL_PORTS:-8022}" >&2
	exit 1
fi

run_concurrently \
	tests "cargo test --workspace --release --all-features" \
	"${live_gates[@]}" \
	codeql "$SCRIPT_DIR/check-codeql.sh" \
	actions "$SCRIPT_DIR/check-actions.sh"

cargo build --workspace --release --all-features
cargo build --examples --all-features
cargo check --workspace --all-features --target x86_64-pc-windows-msvc
# --all-features or the check is blind to sdp and conference-info, which are
# off by default: a removed method there passes an unqualified run untouched.
cargo semver-checks check-release --all-features -p freeswitch-types
cargo semver-checks check-release --all-features -p freeswitch-esl-tokio
# Only types: freeswitch-esl-tokio requires a freeswitch-types floor that is not
# on crates.io until the publish step below actually runs.
cargo publish --dry-run -p freeswitch-types

# docs/next-major.md is actionable only while a major bump is in flight, and
# nothing else in the release path surfaces it.
announce_deferred_breaking_changes() {
	local crate="$1" manifest="$2"
	local local_major stable_max

	local_major="$(sed -n '0,/^version = /s/^version = "\([0-9]\+\)\..*/\1/p' "$manifest")"
	# Prereleases are excluded in jq rather than by grep: a prerelease of the
	# new major must not count as the baseline it is being compared against.
	stable_max="$(
		curl -sSf "https://index.crates.io/fr/ee/$crate" |
			jq -r 'select(.yanked | not) | .vers | select(contains("-") | not)' |
			sort -V | tail -1
	)"

	if [ -z "$stable_max" ] || [ "$local_major" -le "${stable_max%%.*}" ]; then
		return
	fi

	# Untraced so the list reads as a list rather than interleaved with set -x.
	set +x
	printf '\n=== %s %s.x follows %s: docs/next-major.md ===\n\n' \
		"$crate" "$local_major" "$stable_max"
	cat "$CRATE_DIR/docs/next-major.md"
	set -x
}

announce_deferred_breaking_changes freeswitch-types freeswitch-types/Cargo.toml
announce_deferred_breaking_changes freeswitch-esl-tokio Cargo.toml
