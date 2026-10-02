#!/bin/bash
# cargo fmt over this workspace's own packages; arguments pass through.
# --all would also format local path dependencies, which may live in another repository.

set -euo pipefail

args=()
while read -r package; do
	args+=(-p "$package")
done < <(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name')

exec cargo fmt "${args[@]}" "$@"
