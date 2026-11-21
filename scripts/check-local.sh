#!/usr/bin/env bash
# Fast, opt-in local developer checks for Rune (pure POSIX bash/awk/grep; no Python required).
# Usage: scripts/check-local.sh [--wasm] [--all] [--tests] [--help]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

usage() {
  cat <<'EOF'
Usage: scripts/check-local.sh [options]

Fast, opt-in local checks for Rune (no Python required).

Default steps:
  1. scripts/file-size.sh (includes scripts/folder-shape.sh)
  2. Public-repo hygiene scan on staged + unstaged diff (git diff HEAD)
  3. cargo check (default workspace members)
  4. cargo clippy --lib --bins (default workspace members)

Options:
  --wasm        Also run cargo check-wasm and cargo check-webgl
  --all         Check and lint --workspace --all-targets instead of default members
  --tests       Also run cargo test-fast
  -h, --help    Show this help message and exit
EOF
}

run_wasm=0
run_all=0
run_tests=0

for arg in "$@"; do
  case "$arg" in
    --wasm) run_wasm=1 ;;
    --all) run_all=1 ;;
    --tests) run_tests=1 ;;
    -h|--help) usage; exit 0 ;;
    *)
      printf 'unknown option: %s\n\n' "$arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

hygiene_tmp=""
trap 'rm -f "${hygiene_tmp:-}"' EXIT

check_hygiene() {
  # Bracketed slashes keep the pattern identical to the repo hygiene regex
  # without matching this script itself.
  local re='[/]Users[/]|[/]private[/]var[/]|\.ts\.net|ses_[A-Za-z0-9]{20}'
  local rc=0
  hygiene_tmp=$(mktemp) || return 1
  git -c core.quotepath=false diff --no-color --no-ext-diff --src-prefix=a/ --dst-prefix=b/ -U0 HEAD -- \
    | awk '
        /^\+\+\+ b\// {
          file = substr($0, 7)
          sub(/\r$/, "", file)
          sub(/\t$/, "", file)
          next
        }
        /^@@ / {
          if (match($0, /\+[0-9]+/)) {
            line = substr($0, RSTART + 1, RLENGTH - 1) + 0
          }
          next
        }
        /^\+/ && !/^\+\+\+/ {
          body = substr($0, 2)
          sub(/\r$/, "", body)
          check = body
          gsub(/host\.example\.ts\.net/, "", check)
          printf "%s:%d: %s\n", file, line, check
          line++
        }
      ' \
    | grep -E -- "$re" > "$hygiene_tmp" || rc=$?

  if [ -s "$hygiene_tmp" ]; then
    echo "Public-repo hygiene check found disallowed patterns in diff against HEAD:" >&2
    cat "$hygiene_tmp" >&2
    rm -f "$hygiene_tmp"
    hygiene_tmp=""
    return 1
  fi
  rm -f "$hygiene_tmp"
  hygiene_tmp=""
  # grep exits 1 on zero matches; any other non-zero status means git/awk/grep errored.
  [ "$rc" -eq 0 ] || [ "$rc" -eq 1 ]
}

step_names=()
step_status=()
step_secs=()
failures=0

run_step() {
  local name="$1"
  shift
  printf '\n==> %s\n' "$name"
  local start="$SECONDS"
  local rc=0
  "$@" || rc=$?
  local elapsed=$((SECONDS - start))
  step_names+=("$name")
  step_secs+=("$elapsed")
  if [ "$rc" -eq 0 ]; then
    step_status+=("PASS")
  else
    step_status+=("FAIL")
    failures=$((failures + 1))
  fi
}

total_start="$SECONDS"

run_step "file-size" bash scripts/file-size.sh
run_step "public-repo hygiene" check_hygiene

if [ "$run_all" -eq 1 ]; then
  run_step "cargo check --workspace --all-targets" cargo check --workspace --all-targets
  run_step "cargo lint-all" cargo lint-all
else
  run_step "cargo check" cargo check
  run_step "cargo clippy --lib --bins" cargo clippy --lib --bins
fi

if [ "$run_wasm" -eq 1 ]; then
  run_step "cargo check-wasm" cargo check-wasm
  run_step "cargo check-webgl" cargo check-webgl
fi

if [ "$run_tests" -eq 1 ]; then
  run_step "cargo test-fast" cargo test-fast
fi

total_elapsed=$((SECONDS - total_start))
if [ "$failures" -eq 0 ]; then
  overall="PASS"
else
  overall="FAIL"
fi

printf '\nSummary:\n'
printf '  %-38s  %-6s  %8s\n' "step" "status" "elapsed"
printf '  %-38s  %-6s  %8s\n' "--------------------------------------" "------" "--------"
i=0
while [ "$i" -lt "${#step_names[@]}" ]; do
  printf '  %-38s  %-6s  %7ds\n' "${step_names[$i]}" "${step_status[$i]}" "${step_secs[$i]}"
  i=$((i + 1))
done
printf '  %-38s  %-6s  %8s\n' "--------------------------------------" "------" "--------"
printf '  %-38s  %-6s  %7ds\n' "total" "$overall" "$total_elapsed"

[ "$failures" -eq 0 ]
