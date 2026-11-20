#!/usr/bin/env bash
# Checks folder shape for tracked source files under crates/ and examples/.
#   prefix family  x.rs or x/ plus 2+ x_*.rs siblings, or 3+ x_*.rs alone: error, "move into x/"
#   folder breadth > 40 non-test source files in one folder: warn; > 60: error
# Files directly in examples/, benches/ and bin/ are skipped (each is its own program).
# Test files (*_tests.rs, *_test.rs, tests.rs, *.test.ts, ...) are not counted. The prefix is the stem
# up to the first "_" or "-". Existing violations are listed in scripts/folder-shape-exceptions.txt
# ("<key> [ceiling]", key is "<dir>/<prefix>_*" or "<dir>/"; the ceiling is optional); ceilings only go down.
# Usage: scripts/folder-shape.sh [--warn-only] [--update-exceptions [--allow-raise]]
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

WARN=40 HARD=60 FAM=3
EXC=scripts/folder-shape-exceptions.txt
warn_only=0 update=0 raise=0
for a in "$@"; do
  case "$a" in
    --warn-only) warn_only=1 ;;
    --update-exceptions) update=1 ;;
    --allow-raise) raise=1 ;;
    *) echo "unknown option: $a" >&2; exit 1 ;;
  esac
done

found=$(
  git ls-files crates examples | awk -v warn="$WARN" -v fam="$FAM" '
    BEGIN {
      src[".rs"] = 1; src[".ts"] = 1; src[".tsx"] = 1; src[".swift"] = 1
      prog["examples"] = 1; prog["benches"] = 1; prog["bin"] = 1
    }
    {
      sub(/\r$/, "")
      if ($0 == "") next
      slash = 0
      for (i = length($0); i >= 1; i--) {
        if (substr($0, i, 1) == "/") { slash = i; break }
      }
      if (slash > 0) {
        d = substr($0, 1, slash - 1)
        name = substr($0, slash + 1)
      } else {
        d = ""
        name = $0
      }
      nparts = split(d, parts, "/")
      if (nparts >= 3) {
        parent = parts[1] "/" parts[2]
        for (i = 3; i <= nparts; i++) {
          has_sub[parent SUBSEP parts[i]] = 1
          dirs[parent] = 1
          parent = parent "/" parts[i]
        }
      }
      if (nparts > 0 && (parts[nparts] in prog)) next
      dot = 0
      for (i = length(name); i >= 1; i--) {
        if (substr(name, i, 1) == ".") { dot = i; break }
      }
      if (dot > 1 && substr(name, 1, dot - 1) !~ /^\.+$/) {
        stem = substr(name, 1, dot - 1)
        ext = substr(name, dot)
      } else {
        stem = name
        ext = ""
      }
      if (!(ext in src)) next
      if (stem ~ /(^|[_.-])tests?$/) next
      if (index("/" d "/", "/tests/") > 0) next

      dirs[d] = 1
      dir_count[d]++
      has_stem[d SUBSEP stem] = 1
      if (match(stem, /^[^_-]+[_-]/)) {
        p = substr(stem, 1, RLENGTH - 1)
        if (!((d SUBSEP p) in fam_count)) {
          fam_n[d]++
          fam_list[d SUBSEP fam_n[d]] = p
        }
        fam_count[d SUBSEP p]++
      }
    }
    END {
      for (d in dirs) {
        m = fam_n[d] + 0
        for (i = 1; i <= m; i++) {
          p = fam_list[d SUBSEP i]
          base = ((d SUBSEP p) in has_stem) || ((d SUBSEP p) in has_sub)
          limit = base ? 2 : fam
          cnt = fam_count[d SUBSEP p]
          if (cnt >= limit) {
            printf "%s/%s_*\t%d\tmove into %s/\n", d, p, cnt, p
          }
        }
        if (dir_count[d] + 0 > warn) {
          printf "%s/\t%d\tgroup into capability subfolders\n", d, dir_count[d]
        }
      }
    }
  ' | LC_ALL=C sort -t $'\t' -k1,1
)

ceil_of() {
  [ -f "$EXC" ] || return 0
  awk -v k="$1" '
    { sub(/\r$/, "") }
    /^#/ || NF == 0 { next }
    $1 == k { val = (NF > 1 ? $2 : 1000000000); seen = 1 }
    END { if (seen) print val }
  ' "$EXC"
}

if [ "$update" -eq 1 ]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  {
    echo "# <key> <ceiling>: existing folder-shape violations (see scripts/folder-shape.sh). Ceilings only go down."
    echo "# Refresh with scripts/folder-shape.sh --update-exceptions."
    if [ -n "$found" ]; then
      while IFS=$'\t' read -r k n _; do
        case "$k" in
          *_\*) ;;
          *) [ "$n" -gt "$HARD" ] || continue ;;
        esac
        old=$(ceil_of "$k")
        if [ -z "$old" ]; then
          if [ "$raise" -eq 1 ]; then
            echo "$k $n"
          else
            echo "not listing new violation $k ($n); use --allow-raise" >&2
          fi
        elif [ "$n" -le "$old" ] || [ "$raise" -eq 1 ]; then
          echo "$k $n"
        else
          echo "$k $old"
        fi
      done <<< "$found"
    fi
  } > "$tmp"
  mv "$tmp" "$EXC"
  trap - EXIT
  entries=$(awk '/^#/ || NF == 0 { next } { c++ } END { print c + 0 }' "$EXC")
  echo "wrote $EXC ($entries entries)"
  exit 0
fi

errors=0 warns=0 families=0 over_warn=0
rows=$(mktemp)
trap 'rm -f "$rows"' EXIT
if [ -n "$found" ]; then
  while IFS=$'\t' read -r k n hint; do
    case "$k" in
      *_\*) fam=1; families=$((families + 1)) ;;
      *) fam=0; over_warn=$((over_warn + 1)) ;;
    esac
    if [ "$fam" -eq 1 ] || [ "$n" -gt "$HARD" ]; then
      limit=$(ceil_of "$k")
      if [ -n "$limit" ] && [ "$n" -le "$limit" ]; then
        warns=$((warns + 1))
        printf '%6d  warn   %s (%s; exception, ceiling %d)\n' "$n" "$k" "$hint" "$limit" >> "$rows"
      else
        errors=$((errors + 1))
        if [ -n "$limit" ]; then
          printf '%6d  ERROR  %s (%s; limit %d)\n' "$n" "$k" "$hint" "$limit" >> "$rows"
        else
          printf '%6d  ERROR  %s (%s)\n' "$n" "$k" "$hint" >> "$rows"
        fi
        if [ "${GITHUB_ACTIONS:-}" = true ]; then
          printf '::error file=%s::%d files: %s\n' "$k" "$n" "$hint"
        fi
      fi
    else
      warns=$((warns + 1))
      printf '%6d  warn   %s (%s, soft limit %d)\n' "$n" "$k" "$hint" "$WARN" >> "$rows"
    fi
  done <<< "$found"
fi

if [ -s "$rows" ]; then
  cat "$rows"
fi
echo "folder shape: families: $families, folders over $WARN: $over_warn, errors: $errors, warnings: $warns"
[ "$warn_only" -eq 1 ] || [ "$errors" -eq 0 ]
