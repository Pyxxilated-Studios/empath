#!/usr/bin/env bash
set -euo pipefail

# Fast helpers to run fmt/clippy/tests on only the crates that changed.
# Falls back to the full workspace when no crates can be detected.

usage() {
  cat <<'USAGE'
Usage: ./scripts/check-changed.sh [lint|test]
  lint: run cargo fmt + cargo clippy on changed crates (workspace fallback)
  test: run cargo fmt + cargo clippy + cargo test on changed crates (workspace fallback)
USAGE
}

mode="${1:-lint}"
case "$mode" in
  lint|test) ;;
  -h|--help) usage; exit 0 ;;
  *) echo "Unknown mode: $mode" >&2; usage; exit 1 ;;
esac

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for check-changed.sh" >&2
  exit 1
fi

# Collect changed files relative to HEAD
mapfile -t changed_files < <(git diff --name-only --diff-filter=ACMRTUXB HEAD --)

# Build map of package roots -> names
declare -A pkg_for_root
while IFS=$'\t' read -r name manifest; do
  root_dir=$(dirname "$manifest")
  pkg_for_root["$root_dir"]="$name"
done < <(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | "\(.name)\t\(.manifest_path)"')

declare -A touched_pkgs
for file in "${changed_files[@]}"; do
  for root in "${!pkg_for_root[@]}"; do
    if [[ "$file" == "$root"* ]]; then
      touched_pkgs["${pkg_for_root[$root]}"]=1
    fi
  done
done

readarray -t pkgs < <(printf '%s\n' "${!touched_pkgs[@]}" | sort)

if [[ ${#pkgs[@]} -eq 0 ]]; then
  echo "No changed crates detected; running against the full workspace."
  fmt_cmd=(cargo fmt --all)
  clippy_cmd=(cargo clippy --workspace --all-targets --all-features)
  test_cmd=(cargo test --workspace --all-targets)
else
  echo "Changed crates: ${pkgs[*]}"
  fmt_cmd=(cargo fmt)
  clippy_cmd=(cargo clippy --all-targets --all-features)
  test_cmd=(cargo test --all-targets)
  for pkg in "${pkgs[@]}"; do
    fmt_cmd+=(-p "$pkg")
    clippy_cmd+=(-p "$pkg")
    test_cmd+=(-p "$pkg")
  done
fi

echo "==> Running fmt"
"${fmt_cmd[@]}"

echo "==> Running clippy"
"${clippy_cmd[@]}"

if [[ "$mode" == "test" ]]; then
  echo "==> Running tests"
  "${test_cmd[@]}"
fi
