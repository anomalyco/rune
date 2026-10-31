#!/usr/bin/env bash
# Checks Rust file sizes. Counts non-test lines per tracked .rs file under crates/ and examples/
# (inline #[cfg(test)] items are excluded; test-only files are measured separately).
#   warn  > 800 non-test lines       error > 1500 non-test lines unless listed in
#   warn  > 1000 lines in a test file or inline test module   scripts/file-size-exceptions.txt ("<path> <ceiling>")
# Also runs scripts/folder-shape.sh (prefix families and folder breadth); its errors and warnings are
# added to the summary line, and --update-exceptions refreshes both exception lists.
# Usage: scripts/file-size.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SOFT=800 HARD=1500 TESTCAP=1000
EXC=scripts/file-size-exceptions.txt
warn_only=0 update=0 raise=0
for a in "$@"; do
  case "$a" in
    --warn-only) warn_only=1 ;;
    --update-exceptions) update=1 ;;
    --allow-raise) raise=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

measure() {
  git ls-files -z 'crates/*.rs' 'examples/*.rs' | xargs -0 awk '
    function reset() { prod = 0; test = 0; st = 0; depth = 0; maxtest = 0; cur = 0 }
    function braces(s,   t, o, c) {
      gsub(/\\./, "", s); gsub(/"[^"]*"/, "", s); gsub(/\047.\047/, "", s)
      sub(/\/\/.*/, "", s)
      t = s; o = gsub(/\{/, "", t); t = s; c = gsub(/\}/, "", t)
      return o - c
    }
    function flush() {
      if (name != "") {
        if (cur > maxtest) maxtest = cur
        printf "%s %d %d %d\n", name, prod, test, maxtest
      }
    }
    FNR == 1 { flush(); reset(); name = FILENAME
      testfile = (FILENAME ~ /(^|\/)tests?\.rs$/ || FILENAME ~ /_tests\.rs$/ || FILENAME ~ /(^|\/)tests\//) }
    {
      if (testfile) { test++; cur++; next }
      if (st == 0) {
        if ($0 ~ /^[ \t]*#\[cfg\(test\)\]/) { st = 1; cur = 1; test++; next }
        prod++; next
      }
      test++; cur++
      if (st == 1) {                      # attribute seen, find the item head
        if ($0 ~ /^[ \t]*#\[/) next
        d = braces($0)
        if (d > 0 || $0 ~ /\{/) { depth = d; st = (depth > 0) ? 2 : 0 }
        else if ($0 ~ /;[ \t]*$/) st = 0
        if (st == 0) { if (cur > maxtest) maxtest = cur; cur = 0 }
        next
      }
      depth += braces($0)
      if (depth <= 0) { st = 0; if (cur > maxtest) maxtest = cur; cur = 0 }
    }
    END { flush() }'
}

rows=$(measure)
ceil_of() { [ -f "$EXC" ] && awk -v p="$1" '$1 == p { print $2; exit }' "$EXC" || true; }

errors=0 warns=0 oversoft=0 overhard=0
out=$(mktemp); trap 'rm -f "$out"' EXIT
ann() { [ "${GITHUB_ACTIONS:-}" = true ] && echo "::$1 file=$2::$3" || true; }

while read -r p prod test maxtest; do
  if [ "$prod" -eq 0 ] && [ "$test" -gt 0 ] && [ "$maxtest" -eq "$test" ] && [ "$test" -gt "$TESTCAP" ]; then
    # test-only file
    warns=$((warns + 1))
    printf '%6d  %-6s %s\n' "$test" "tests" "$p" >> "$out"
    ann warning "$p" "test file has $test lines (limit $TESTCAP)"
    continue
  fi
  if [ "$maxtest" -gt "$TESTCAP" ]; then
    warns=$((warns + 1))
    printf '%6d  %-6s %s\n' "$maxtest" "tests" "$p" >> "$out"
    ann warning "$p" "inline test module has $maxtest lines (limit $TESTCAP)"
  fi
  [ "$prod" -gt "$SOFT" ] || continue
  oversoft=$((oversoft + 1))
  if [ "$prod" -gt "$HARD" ]; then
    overhard=$((overhard + 1))
    limit=$(ceil_of "$p"); limit=${limit:-$HARD}
    if [ "$prod" -gt "$limit" ]; then
      errors=$((errors + 1))
      printf '%6d  %-6s %s (limit %d)\n' "$prod" "ERROR" "$p" "$limit" >> "$out"
      ann error "$p" "$prod non-test lines, limit $limit; split a module out before adding more"
    else
      warns=$((warns + 1))
      printf '%6d  %-6s %s (exception, ceiling %d)\n' "$prod" "warn" "$p" "$limit" >> "$out"
      ann warning "$p" "$prod non-test lines (exception ceiling $limit, soft limit $SOFT)"
    fi
  else
    warns=$((warns + 1))
    printf '%6d  %-6s %s\n' "$prod" "warn" "$p" >> "$out"
    ann warning "$p" "$prod non-test lines (soft limit $SOFT)"
  fi
done <<< "$rows"

sort -rn "$out"
shape_args=()
[ "$update" -eq 1 ] && shape_args+=(--update-exceptions)
[ "$raise" -eq 1 ] && shape_args+=(--allow-raise)
shape_out=$(bash scripts/folder-shape.sh ${shape_args[@]+"${shape_args[@]}"} --warn-only)
if [ "$update" -eq 0 ]; then
  printf '%s\n' "$shape_out"
  shape_sum=$(printf '%s\n' "$shape_out" | tail -1)
  errors=$((errors + $(sed -n 's/.*errors: \([0-9]*\),.*/\1/p' <<< "$shape_sum")))
  warns=$((warns + $(sed -n 's/.*warnings: \([0-9]*\)$/\1/p' <<< "$shape_sum")))
else
  printf '%s\n' "$shape_out"
fi
echo "over $SOFT: $oversoft, over $HARD: $overhard, errors: $errors, warnings: $warns"

if [ "$update" -eq 1 ]; then
  tmp=$(mktemp)
  {
    echo "# <path> <ceiling>: non-test line ceilings for files above $HARD. Ceilings only go down."
    echo "# Refresh with scripts/file-size.sh --update-exceptions."
    while read -r p prod _ _; do
      [ "$prod" -gt "$HARD" ] || continue
      old=$(ceil_of "$p")
      if [ -z "$old" ]; then
        if [ "$raise" -eq 1 ]; then echo "$p $prod"; else echo "not listing new oversized file $p ($prod); use --allow-raise" >&2; fi
      elif [ "$prod" -gt "$old" ] && [ "$raise" -eq 0 ]; then
        echo "$p $old"
      else
        echo "$p $prod"
      fi
    done <<< "$rows" | sort
  } > "$tmp"
  mv "$tmp" "$EXC"
  echo "wrote $EXC ($(grep -vc '^#' "$EXC") entries)"
  exit 0
fi

[ "$warn_only" -eq 1 ] && exit 0
[ "$errors" -eq 0 ]
