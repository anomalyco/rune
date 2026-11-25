#!/usr/bin/env bash
# Stage, package, verify, and (optionally) publish Rune's 13-crate development build
# (`rune-ui` plus twelve `rune-ui-*` internal crates) to crates.io without modifying any
# committed manifest in the repository.
#
# Usage:
#   scripts/publish-crates.sh --version 0.0.0-dev.1 [--rev <git-ref>] [--stage-dir <dir>]
#                             [--target-dir <dir>] [--skip-verify] [--publish] [--wait]
#
# Dry run by default: never uploads unless --publish is explicitly passed.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/publish-crates.sh --version <semver> [options]

Stages a clean git archive of <rev> outside the repository, rewrites only the
staged manifests for the 13-crate core closure (rune-ui + twelve rune-ui-*
internal crates), scans for public-repo hygiene and secrets, packages all 13
.crate archives in dependency order, verifies an isolated unpacked consumer
build, and publishes only when --publish is explicitly passed.

Options:
  --version <semver>   Required. SemVer 2.0.0 version (e.g. 0.0.0-dev.1).
                       Pre-release counters must be dot-separated (0.0.0-dev.N).
  --rev <git-ref>      Git revision to export via git archive (default: HEAD).
  --stage-dir <path>   Staging directory outside the repo (default: temp dir).
  --target-dir <path>  Optional CARGO_TARGET_DIR for packaging and verification.
  --skip-verify        Package and inspect .crate archives without compiling the
                       isolated consumer smoke test.
  --publish            Upload packaged crates to crates.io in dependency order.
                       Requires CARGO_REGISTRY_TOKEN in the environment.
  --wait               Wait out crates.io HTTP 429 rate limits during --publish
                       (5-crate initial burst, then 1 new crate per 10 minutes).
  -h, --help           Show this help message and exit.
EOF
}

VERSION=""
REV="HEAD"
STAGE_DIR=""
TARGET_DIR=""
DO_PUBLISH=0
DO_WAIT=0
SKIP_VERIFY=0
AUTO_STAGE_DIR=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || { echo "error: --version requires a value" >&2; exit 2; }
      VERSION="$2"
      shift 2
      ;;
    --rev)
      [ "$#" -ge 2 ] || { echo "error: --rev requires a value" >&2; exit 2; }
      REV="$2"
      shift 2
      ;;
    --stage-dir)
      [ "$#" -ge 2 ] || { echo "error: --stage-dir requires a value" >&2; exit 2; }
      STAGE_DIR="$2"
      shift 2
      ;;
    --target-dir)
      [ "$#" -ge 2 ] || { echo "error: --target-dir requires a value" >&2; exit 2; }
      TARGET_DIR="$2"
      shift 2
      ;;
    --skip-verify)
      SKIP_VERIFY=1
      shift
      ;;
    --publish)
      DO_PUBLISH=1
      shift
      ;;
    --wait)
      DO_WAIT=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "error: --version is required (for example: --version 0.0.0-dev.1)" >&2
  exit 2
fi

# Validate SemVer 2.0.0, requiring dot-separated numeric pre-release counters when using -dev.
if ! printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'; then
  printf 'error: invalid SemVer 2.0.0 version: %s\n' "$VERSION" >&2
  exit 2
fi
if printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+-dev[0-9]+$'; then
  printf 'error: pre-release counter must be dot-separated (use %s-dev.N, not %s)\n' "${VERSION%%-*}" "$VERSION" >&2
  exit 2
fi

FULL_SHA="$(git -C "$REPO_ROOT" rev-parse --verify "${REV}^{commit}")"
SHORT_SHA="${FULL_SHA:0:12}"

to_host_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -m "$1"
  else
    printf '%s\n' "$1"
  fi
}

to_unix_path() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -u "$1"
  else
    printf '%s\n' "$1"
  fi
}

if [ -z "$STAGE_DIR" ]; then
  STAGE_DIR="$(mktemp -d -t rune-publish-XXXXXX)"
  AUTO_STAGE_DIR=1
else
  STAGE_DIR="$(to_unix_path "$STAGE_DIR")"
  mkdir -p "$STAGE_DIR"
fi
STAGE_DIR="$(cd "$STAGE_DIR" && pwd)"

REPO_CANON="$(cd "$REPO_ROOT" && pwd)"
case "$STAGE_DIR" in
  "$REPO_CANON"|"$REPO_CANON"/*)
    printf 'error: --stage-dir (%s) must be outside the repository (%s)\n' "$STAGE_DIR" "$REPO_CANON" >&2
    exit 2
    ;;
esac

if [ -n "$TARGET_DIR" ]; then
  TARGET_DIR="$(to_unix_path "$TARGET_DIR")"
  mkdir -p "$TARGET_DIR"
  TARGET_DIR="$(cd "$TARGET_DIR" && pwd)"
else
  TARGET_DIR="$STAGE_DIR/target"
  mkdir -p "$TARGET_DIR"
fi

CARGO_TARGET_HOST="$(to_host_path "$TARGET_DIR")"
export CARGO_TARGET_DIR="$CARGO_TARGET_HOST"

# 13-crate minimal core closure in topological publish order:
#   folder|repo_package|published_name|lib_name|license
CRATES=(
  "crates/rune-motion|rune-motion|rune-ui-motion|rune_ui_motion|MIT"
  "crates/rune-staging|rune-staging|rune-ui-staging|rune_ui_staging|MIT"
  "crates/rune-taffy|rune-taffy|rune-ui-taffy|rune_ui_taffy|MIT"
  "crates/procwatch|procwatch|rune-ui-procwatch|rune_ui_procwatch|MIT"
  "vendor/tiny-skia-path|rune-tiny-skia-path|rune-ui-tiny-skia-path|rune_ui_tiny_skia_path|BSD-3-Clause"
  "crates/rune-core|rune-core|rune-ui-core|rune_ui_core|MIT"
  "crates/rune-gpu|rune-gpu|rune-ui-gpu|rune_ui_gpu|MIT"
  "crates/rune-image|rune-image|rune-ui-image|rune_ui_image|MIT"
  "crates/rune-spell|rune-spell|rune-ui-spell|rune_ui_spell|MIT"
  "crates/rune-text|rune-text|rune-ui-text|rune_ui_text|MIT AND OFL-1.1"
  "crates/rune-edit|rune-edit|rune-ui-edit|rune_ui_edit|MIT"
  "crates/rune-media|rune-media|rune-ui-media|rune_ui_media|MIT"
  "crates/rune|rune|rune-ui|rune|MIT"
)

PATCHED_VENDOR_CRATES=(
  "wgpu-hal"
  "naga"
  "winit"
  "sctk-adwaita"
  "parley_engine"
  "accesskit_windows"
  "accesskit_atspi_common"
  "accesskit_macos"
)

SRC="$STAGE_DIR/src"
CRATES_OUT="$STAGE_DIR/crates"
VERIFY_DIR="$STAGE_DIR/verify"
UNPACKED_DIR="$VERIFY_DIR/unpacked"
CONSUMER_DIR="$VERIFY_DIR/consumer"

printf '==> Staging Rune %s from %s (%s)\n' "$VERSION" "$REV" "$SHORT_SHA"
printf '    stage-dir:  %s\n' "$STAGE_DIR"
printf '    target-dir: %s\n' "$TARGET_DIR"
if [ "$DO_PUBLISH" -eq 1 ]; then
  echo "    mode:       PUBLISH (--publish)"
else
  echo "    mode:       DRY RUN (pass --publish to upload)"
fi

# 1. Clean export of <rev> via git archive so uncommitted files never enter .crate archives.
rm -rf "$SRC" "$CRATES_OUT" "$VERIFY_DIR"
mkdir -p "$SRC" "$CRATES_OUT" "$UNPACKED_DIR" "$CONSUMER_DIR/src"
git -C "$REPO_ROOT" archive --format=tar "$FULL_SHA" | tar -xf - -C "$SRC"
rm -f "$SRC/Cargo.lock"

# Locate the crates.io README template (prefer archived copy, fall back to script's sibling).
if [ -f "$SRC/scripts/publish-crates-readme.md" ]; then
  README_TEMPLATE="$SRC/scripts/publish-crates-readme.md"
elif [ -f "$SCRIPT_DIR/publish-crates-readme.md" ]; then
  README_TEMPLATE="$SCRIPT_DIR/publish-crates-readme.md"
else
  echo "error: missing scripts/publish-crates-readme.md template" >&2
  exit 1
fi

# 2. Copy default fonts and OFL licenses into staged crates/rune-text/assets/
#    and rewrite include_bytes! paths in default_fonts.rs.
echo "==> Copying default fonts into staged rune-ui-text"
FONT_FILES=(
  "Inter-Regular.ttf"
  "Inter-Medium.ttf"
  "Inter-SemiBold.ttf"
  "IBMPlexMono-Regular.ttf"
  "Inter-LICENSE.txt"
  "IBMPlexMono-LICENSE.txt"
  "web/first/Inter-Regular.ttf"
  "web/first/Inter-Medium.ttf"
  "web/first/Inter-SemiBold.ttf"
)
mkdir -p "$SRC/crates/rune-text/assets/web/first"
for rel in "${FONT_FILES[@]}"; do
  src_f="$SRC/crates/rune/assets/$rel"
  dst_f="$SRC/crates/rune-text/assets/$rel"
  [ -f "$src_f" ] || { printf 'error: expected font asset missing at %s: %s\n' "$SHORT_SHA" "$rel" >&2; exit 1; }
  cp "$src_f" "$dst_f"
done

DEFAULT_FONTS_RS="$SRC/crates/rune-text/src/fonts/default_fonts.rs"
[ -f "$DEFAULT_FONTS_RS" ] || { echo "error: missing crates/rune-text/src/fonts/default_fonts.rs" >&2; exit 1; }
awk '
  {
    sub(/\r$/, "")
    gsub(/include_bytes!\("..\/..\/..\/rune\/assets\//, "include_bytes!(\"../../assets/")
    print
  }
' "$DEFAULT_FONTS_RS" > "$DEFAULT_FONTS_RS.tmp"
mv "$DEFAULT_FONTS_RS.tmp" "$DEFAULT_FONTS_RS"

if grep -q '../../../rune/assets/' "$DEFAULT_FONTS_RS"; then
  echo "error: leftover ../../../rune/assets/ path in staged default_fonts.rs" >&2
  exit 1
fi
rewritten_count="$(grep -c 'include_bytes!("../../assets/' "$DEFAULT_FONTS_RS" || true)"
if [ "$rewritten_count" -lt 7 ]; then
  printf 'error: expected at least 7 rewritten font include_bytes! paths, found %s\n' "$rewritten_count" >&2
  exit 1
fi

# Copy MIT LICENSE into each MIT crate folder so every .crate tarball includes its license text.
[ -f "$SRC/LICENSE" ] || { echo "error: missing root LICENSE file" >&2; exit 1; }
for entry in "${CRATES[@]}"; do
  IFS='|' read -r dir _repo_pkg _pub_name _lib_name _lic <<< "$entry"
  if [ "$dir" != "vendor/tiny-skia-path" ] && [ ! -f "$SRC/$dir/LICENSE" ]; then
    cp "$SRC/LICENSE" "$SRC/$dir/LICENSE"
  fi
done

# Render crates.io README for rune-ui.
awk -v ver="$VERSION" -v rev="$FULL_SHA" '
  {
    sub(/\r$/, "")
    gsub(/[{][{]VERSION[}][}]/, ver)
    gsub(/[{][{]REV[}][}]/, rev)
    print
  }
' "$README_TEMPLATE" > "$SRC/crates/rune/README.crates-io.md"

# 3. Rewrite staged root Cargo.toml to include only the 13 staged members and map
#    internal [workspace.dependencies] entries to their published rune-ui* names with exact =<ver>.
echo "==> Rewriting staged manifests"
awk -v ver="$VERSION" '
  BEGIN {
    in_members = 0
    in_default_members = 0
    in_ws_deps = 0
    map["procwatch"] = "rune-ui-procwatch"
    map["rune"] = "rune-ui"
    map["rune-core"] = "rune-ui-core"
    map["rune-edit"] = "rune-ui-edit"
    map["rune-gpu"] = "rune-ui-gpu"
    map["rune-image"] = "rune-ui-image"
    map["rune-media"] = "rune-ui-media"
    map["rune-motion"] = "rune-ui-motion"
    map["rune-spell"] = "rune-ui-spell"
    map["rune-staging"] = "rune-ui-staging"
    map["rune-taffy"] = "rune-ui-taffy"
    map["rune-text"] = "rune-ui-text"
  }
  function print_members(key) {
    print key " = ["
    print "    \"crates/rune-motion\","
    print "    \"crates/rune-staging\","
    print "    \"crates/rune-taffy\","
    print "    \"crates/procwatch\","
    print "    \"vendor/tiny-skia-path\","
    print "    \"crates/rune-core\","
    print "    \"crates/rune-gpu\","
    print "    \"crates/rune-image\","
    print "    \"crates/rune-spell\","
    print "    \"crates/rune-text\","
    print "    \"crates/rune-edit\","
    print "    \"crates/rune-media\","
    print "    \"crates/rune\","
    print "]"
  }
  {
    sub(/\r$/, "")
    if (in_members) {
      if ($0 ~ /^\]/) { in_members = 0 }
      next
    }
    if (in_default_members) {
      if ($0 ~ /^\]/) { in_default_members = 0 }
      next
    }
    if ($0 ~ /^\[/) {
      in_ws_deps = ($0 ~ /^\[workspace\.dependencies\][ \t]*$/)
    }
    if ($0 ~ /^members[ \t]*=[ \t]*\[/) {
      print_members("members")
      if ($0 !~ /\][ \t]*$/) { in_members = 1 }
      next
    }
    if ($0 ~ /^default-members[ \t]*=[ \t]*\[/) {
      print_members("default-members")
      if ($0 !~ /\][ \t]*$/) { in_default_members = 1 }
      next
    }
    if ($0 ~ /^exclude[ \t]*=[ \t]*\[/) {
      gsub(/"vendor\/tiny-skia-path",[ \t]*/, "")
      gsub(/,[ \t]*"vendor\/tiny-skia-path"/, "")
      print
      next
    }
    if (in_ws_deps) {
      for (k in map) {
        pat = "^" k "[ \t]*=[ \t]*[{]"
        if ($0 ~ pat) {
          sub(/[{][ \t]*/, "{ package = \"" map[k] "\", version = \"=" ver "\", ")
          break
        }
      }
    }
    print
  }
' "$SRC/Cargo.toml" > "$SRC/Cargo.toml.tmp"
mv "$SRC/Cargo.toml.tmp" "$SRC/Cargo.toml"

# Rewrite vendor/tiny-skia-path manifest and strip upstream crates.io packaging artifacts.
rm -f "$SRC/vendor/tiny-skia-path/.cargo_vcs_info.json" "$SRC/vendor/tiny-skia-path/Cargo.toml.orig"
awk -v ver="$VERSION" '
  BEGIN { section = ""; added_homepage = 0 }
  function finish_pkg() {
    if (!added_homepage) {
      print "homepage = \"https://anoma.ly/team/rune\""
      added_homepage = 1
    }
  }
  {
    sub(/\r$/, "")
    if ($0 ~ /^\[/) {
      if (section == "[package]") { finish_pkg() }
      section = $0
      print
      next
    }
    if (section == "[package]") {
      if ($0 ~ /^name[ \t]*=/) { print "name = \"rune-ui-tiny-skia-path\""; next }
      if ($0 ~ /^version[ \t]*=/) { print "version = \"" ver "\""; next }
      if ($0 ~ /^description[ \t]*=/) { print "description = \"Internal crate of rune-ui: tiny-skia-path fork with Skia conic splitting and stroker patches.\""; next }
      if ($0 ~ /^documentation[ \t]*=/) { next }
      if ($0 ~ /^readme[ \t]*=/) { print "readme = false"; next }
      if ($0 ~ /^license[ \t]*=/) { print "license = \"BSD-3-Clause\""; next }
      if ($0 ~ /^repository[ \t]*=/) { print "repository = \"https://github.com/anomalyco/rune\""; next }
      if ($0 ~ /^homepage[ \t]*=/) { print "homepage = \"https://anoma.ly/team/rune\""; added_homepage = 1; next }
    }
    if (section == "[lib]" && $0 ~ /^name[ \t]*=/) {
      print "name = \"rune_ui_tiny_skia_path\""
      next
    }
    print
  }
  END {
    if (section == "[package]") { finish_pkg() }
  }
' "$SRC/vendor/tiny-skia-path/Cargo.toml" > "$SRC/vendor/tiny-skia-path/Cargo.toml.tmp"
mv "$SRC/vendor/tiny-skia-path/Cargo.toml.tmp" "$SRC/vendor/tiny-skia-path/Cargo.toml"

# Rewrite each of the 12 crates/* staged Cargo.toml manifests.
rewrite_crate_manifest() {
  local dir="$1"
  local pub_name="$2"
  local lic="$3"
  local manifest="$SRC/$dir/Cargo.toml"

  awk -v pub_name="$pub_name" -v ver="$VERSION" -v lic="$lic" '
    BEGIN {
      section = ""
      skip_section = 0
      pkg_flushed = 0
    }
    function flush_pkg() {
      if (pkg_flushed) return
      pkg_flushed = 1
      print "repository = \"https://github.com/anomalyco/rune\""
      print "homepage = \"https://anoma.ly/team/rune\""
      print "autotests = false"
      print "autoexamples = false"
      print "autobenches = false"
      if (pub_name == "rune-ui") {
        print "documentation = \"https://anoma.ly/team/rune\""
        print "readme = \"README.crates-io.md\""
        print "keywords = [\"gui\", \"ui\", \"wgpu\", \"graphics\", \"framework\"]"
        print "categories = [\"gui\", \"graphics\"]"
        print "exclude = [\"assets/*\", \"tests/*\", \"examples/*\", \"benches/*\", \"README.md\"]"
        print ""
        print "[package.metadata.docs.rs]"
        print "targets = [\"x86_64-unknown-linux-gnu\"]"
        print ""
        print "[lib]"
        print "name = \"rune\""
      } else {
        print "readme = false"
        print "exclude = [\"tests/*\", \"examples/*\", \"benches/*\"]"
      }
    }
    {
      sub(/\r$/, "")
      if ($0 ~ /^\[/) {
        if (section == "[package]") {
          flush_pkg()
          print ""
        }
        section = $0
        if ($0 ~ /^\[\[(test|example|bench)\]\][ \t]*$/) {
          skip_section = 1
          next
        }
        skip_section = 0
        print
        next
      }
      if (skip_section) { next }
      if (section == "[package]") {
        if ($0 ~ /^name[ \t]*=/) { print "name = \"" pub_name "\""; next }
        if ($0 ~ /^version[ \t]*=/) { print "version = \"" ver "\""; next }
        if ($0 ~ /^publish[ \t]*=/) { next }
        if ($0 ~ /^(autotests|autoexamples|autobenches)[ \t]*=/) { next }
        if ($0 ~ /^repository[ \t]*=|^homepage[ \t]*=|^documentation[ \t]*=|^readme[ \t]*=|^exclude[ \t]*=/) { next }
        if (pub_name == "rune-ui-text" && $0 ~ /^license(\.workspace)?[ \t]*=/) {
          print "license = \"" lic "\""
          next
        }
        if (pub_name != "rune-ui" && $0 ~ /^description[ \t]*=[ \t]*"/) {
          desc = $0
          sub(/^description[ \t]*=[ \t]*"/, "", desc)
          sub(/"[ \t]*$/, "", desc)
          if (desc !~ /^Internal crate of rune-ui:/) {
            desc = "Internal crate of rune-ui: " desc
          }
          print "description = \"" desc "\""
          next
        }
      }
      if (pub_name == "rune-ui-text" && $0 ~ /^tiny-skia-path[ \t]*=/) {
        print "tiny-skia-path = { package = \"rune-ui-tiny-skia-path\", version = \"=" ver "\", path = \"../../vendor/tiny-skia-path\" }"
        next
      }
      # Upstream crates.io parley_engine 0.12.0 does not define the patch-only
      # `line-dictionaries` feature; Cargo rejects publishing a crate that requests an
      # unknown feature from crates.io. Dropping it in the staged manifest lets
      # rune-ui-text publish cleanly while vendor/parley_engine still uses Windows icu.dll
      # for word and SA line boundaries (and `segmenter-dictionaries` still opts into ICU4X).
      if (pub_name == "rune-ui-text" && $0 ~ /^parley_engine[ \t]*=.*line-dictionaries/) {
        print "parley_engine = { workspace = true }"
        next
      }
      print
    }
    END {
      if (section == "[package]") { flush_pkg() }
    }
  ' "$manifest" > "$manifest.tmp"
  mv "$manifest.tmp" "$manifest"
}

for entry in "${CRATES[@]}"; do
  IFS='|' read -r dir _repo_pkg pub_name _lib_name lic <<< "$entry"
  if [ "$dir" != "vendor/tiny-skia-path" ]; then
    rewrite_crate_manifest "$dir" "$pub_name" "$lic"
  fi
done

# 4. Scan staged files (and later unpacked .crate files) for public-repo hygiene and secrets.
# Bracketed letters keep the patterns identical without matching this script itself.
HYGIENE_RE='[/]Users[/]|[/]private[/]var[/]|\.ts\.net|ses_[A-Za-z0-9]{20}'
SECRET_RE='(^|[^A-Za-z0-9_])([c]io[A-Za-z0-9]{30,}|[g]hp_[A-Za-z0-9]{20,}|[g]ithub_pat_[A-Za-z0-9_]{20,}|[A]KIA[0-9A-Z]{16}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE [K]EY-----)'

scan_tree() {
  local label="$1"
  shift
  local hit_tmp
  hit_tmp="$(mktemp)"
  {
    find "$@" -type f \
        ! -path '*/tests/*' \
        ! -path '*/examples/*' \
        ! -path '*/benches/*' \
        ! -name '*.ttf' \
        ! -name '*.png' \
        ! -name '*.dxbc' \
        ! -name '*.metallib' \
        ! -name '*.bin' \
        ! -name '*.crate' \
        -print0 \
      | xargs -0 grep -HnE -- "$HYGIENE_RE" 2>/dev/null \
      | grep -Fv 'host.example.ts.net' || true
    find "$@" -type f \
        ! -path '*/tests/*' \
        ! -path '*/examples/*' \
        ! -path '*/benches/*' \
        ! -name '*.ttf' \
        ! -name '*.png' \
        ! -name '*.dxbc' \
        ! -name '*.metallib' \
        ! -name '*.bin' \
        ! -name '*.crate' \
        -print0 \
      | xargs -0 grep -HnE -- "$SECRET_RE" 2>/dev/null || true
  } > "$hit_tmp"
  if [ -s "$hit_tmp" ]; then
    printf 'error: hygiene/secret scan failed (%s):\n' "$label" >&2
    cat "$hit_tmp" >&2
    rm -f "$hit_tmp"
    exit 1
  fi
  rm -f "$hit_tmp"
}

echo "==> Scanning staged crate trees for public-repo hygiene and secrets"
STAGED_SCAN_DIRS=()
for entry in "${CRATES[@]}"; do
  IFS='|' read -r dir _ <<< "$entry"
  STAGED_SCAN_DIRS+=("$SRC/$dir")
done
scan_tree "staged sources" "${STAGED_SCAN_DIRS[@]}"

# 5. Package all 13 workspace crates with cargo package --workspace --no-verify --allow-dirty
#    (resolves sibling unpublished crates and validates all crates.io dependencies/features).
echo "==> Packaging 13 crates (workspace topological order)"
STAGE_MANIFEST_HOST="$(to_host_path "$SRC/Cargo.toml")"
cargo package --manifest-path "$STAGE_MANIFEST_HOST" --workspace --no-verify --allow-dirty

MAX_CRATE_BYTES=$((10 * 1024 * 1024))
WARN_CRATE_BYTES=$((5 * 1024 * 1024))

MANIFEST_ROWS=()
idx=0
for entry in "${CRATES[@]}"; do
  idx=$((idx + 1))
  IFS='|' read -r _dir _repo_pkg pub_name _lib_name lic <<< "$entry"

  pkg_crate="$TARGET_DIR/package/${pub_name}-${VERSION}.crate"
  [ -f "$pkg_crate" ] || { printf 'error: expected packaged archive missing: %s\n' "$pkg_crate" >&2; exit 1; }
  cp "$pkg_crate" "$CRATES_OUT/${pub_name}-${VERSION}.crate"

  bytes="$(wc -c < "$pkg_crate" | tr -d ' ')"
  files="$(tar -tzf "$pkg_crate" | wc -l | tr -d ' ')"

  # Verify archive paths never escape the crate root.
  bad_paths="$(tar -tzf "$pkg_crate" | awk -v prefix="${pub_name}-${VERSION}/" '
    {
      if ($0 ~ /^\// || $0 ~ /(^|\/)\.\.(\/|$)/ || index($0, prefix) != 1) {
        print
      }
    }
  ')"
  if [ -n "$bad_paths" ]; then
    printf 'error: %s contains unsafe archive paths:\n%s\n' "$pub_name" "$bad_paths" >&2
    exit 1
  fi

  if [ "$bytes" -ge "$MAX_CRATE_BYTES" ]; then
    printf 'error: %s is %s bytes (exceeds crates.io 10 MiB limit)\n' "$pub_name" "$bytes" >&2
    exit 1
  elif [ "$bytes" -ge "$WARN_CRATE_BYTES" ]; then
    printf 'warning: %s is %s bytes (over 5 MiB)\n' "$pub_name" "$bytes" >&2
  fi

  mib="$(awk -v b="$bytes" 'BEGIN { printf "%.2f", b / 1048576 }')"
  MANIFEST_ROWS+=("$(printf '%2d  %-24s  %-14s  %5s  %10s  %6s MiB  %s' "$idx" "$pub_name" "$VERSION" "$files" "$bytes" "$mib" "$lic")")
done

printf '\n==> Packaged crates (%s @ %s)\n' "$VERSION" "$SHORT_SHA"
printf '%2s  %-24s  %-14s  %5s  %10s  %10s  %s\n' "#" "Package" "Version" "Files" "Size (B)" "Size" "License"
printf '%2s  %-24s  %-14s  %5s  %10s  %10s  %s\n' "--" "------------------------" "--------------" "-----" "----------" "----------" "----------------"
for row in "${MANIFEST_ROWS[@]}"; do
  printf '%s\n' "$row"
done
printf '\nArchives written to: %s\n' "$CRATES_OUT"

# 6. Unpack all 13 .crate archives, re-scan unpacked contents, and generate the isolated consumer crate.
echo "==> Unpacking .crate archives into isolated consumer workspace"
for entry in "${CRATES[@]}"; do
  IFS='|' read -r _dir _repo_pkg pub_name _lib_name _lic <<< "$entry"
  tar -xzf "$CRATES_OUT/${pub_name}-${VERSION}.crate" -C "$UNPACKED_DIR"
done
scan_tree "unpacked .crate archives" "$UNPACKED_DIR"

{
  cat <<EOF
[package]
name = "rune-ui-consumer-smoke"
version = "0.1.0"
edition = "2024"

[features]
default = ["rune-ui/default"]
webgl = ["rune-ui/webgl"]
embedded-fonts = ["rune-ui/embedded-fonts"]

[dependencies]
rune-ui = { version = "=$VERSION", default-features = false }

[patch.crates-io]
EOF
  for entry in "${CRATES[@]}"; do
    IFS='|' read -r _dir _repo_pkg pub_name _lib_name _lic <<< "$entry"
    printf '%s = { path = "../unpacked/%s-%s" }\n' "$pub_name" "$pub_name" "$VERSION"
  done
  for v in "${PATCHED_VENDOR_CRATES[@]}"; do
    printf '%s = { path = "../../src/vendor/%s" }\n' "$v" "$v"
  done
} > "$CONSUMER_DIR/Cargo.toml"

cat > "$CONSUMER_DIR/src/main.rs" <<'EOF'
use rune::render::cpu_boot::{CpuPainter, Scene, Target};
use rune::{AppSpec, Color, Cx, El, Env, Runtime, col, text};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col()
        .w_full()
        .h_full()
        .center()
        .fill(Color { r: 0.1, g: 0.12, b: 0.16, a: 1.0 })
        .child(text("Hello, Rune").color(Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }))
}

#[allow(dead_code)]
fn readme_example_compiles() {
    let _spec = AppSpec::new("Hello", (400.0, 240.0), || (), view);
}

fn render_offscreen() -> usize {
    let (w, h) = (160u32, 80u32);
    let env = Env { system_dark: true, scale: 1.0 };
    let mut rt = Runtime::new((), view, (w as f32, h as f32), env);
    rt.frame(0.0);
    assert!(!rt.instances.is_empty(), "expected draw instances from Runtime::frame");

    let mut painter = CpuPainter::new(1);
    let _ = painter.fill_masks(&mut rt.text.atlas);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), rt.clear_color())
        .srgb_blend(true)
        .fade_stops(rt.fade_stops());
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    painter
        .paint(&scene, &mut Target::new(&mut pixels, w, h))
        .expect("CpuPainter paints offscreen frame");
    let lit = pixels
        .chunks_exact(4)
        .filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0 || p[3] != 0)
        .count();
    assert!(lit > 0, "expected non-empty rendered pixels");
    lit
}

fn main() {
    let lit = render_offscreen();
    println!("rune-ui consumer smoke ok: {lit} lit pixels");
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_offscreen_frame() {
        assert!(super::render_offscreen() > 0);
    }
}
EOF

if [ "$SKIP_VERIFY" -eq 0 ]; then
  echo "==> Verifying isolated consumer crate (cargo check + headless off-screen test)"
  CONSUMER_MANIFEST="$(to_host_path "$CONSUMER_DIR/Cargo.toml")"
  cargo check --manifest-path "$CONSUMER_MANIFEST"
  cargo test --manifest-path "$CONSUMER_MANIFEST" -- --nocapture
  cargo run --manifest-path "$CONSUMER_MANIFEST"
fi

# 7. Publish (ONLY when --publish is explicitly passed).
if [ "$DO_PUBLISH" -eq 0 ]; then
  printf '\n==> Dry run complete. Nothing was published to crates.io.\n'
  if [ "$AUTO_STAGE_DIR" -eq 1 ]; then
    printf '    Staged artifacts kept at: %s\n' "$STAGE_DIR"
  fi
  exit 0
fi

if [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
  echo "error: --publish requires CARGO_REGISTRY_TOKEN to be set in the environment" >&2
  exit 1
fi

UA="rune-publish-crates (https://github.com/anomalyco/rune)"

crate_already_published() {
  local pkg="$1"
  local ver="$2"
  local code
  code="$(curl -sS -o /dev/null -w '%{http_code}' -A "$UA" "https://crates.io/api/v1/crates/${pkg}/${ver}" || echo "000")"
  [ "$code" = "200" ]
}

sparse_index_url() {
  local lower
  lower="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  local len="${#lower}"
  if [ "$len" -eq 1 ]; then
    printf 'https://index.crates.io/1/%s\n' "$lower"
  elif [ "$len" -eq 2 ]; then
    printf 'https://index.crates.io/2/%s\n' "$lower"
  elif [ "$len" -eq 3 ]; then
    printf 'https://index.crates.io/3/%s/%s\n' "${lower:0:1}" "$lower"
  else
    printf 'https://index.crates.io/%s/%s/%s\n' "${lower:0:2}" "${lower:2:2}" "$lower"
  fi
}

wait_for_index() {
  local pkg="$1"
  local ver="$2"
  local url
  url="$(sparse_index_url "$pkg")"
  local tries=0
  while [ "$tries" -lt 36 ]; do
    if curl -fsS -A "$UA" "$url" 2>/dev/null | grep -Fq "\"vers\":\"${ver}\""; then
      return 0
    fi
    tries=$((tries + 1))
    printf '    waiting for %s %s in crates.io index (%d/36)...\n' "$pkg" "$ver" "$tries"
    sleep 5
  done
  printf 'warning: %s %s not yet visible in sparse index after 180s; continuing\n' "$pkg" "$ver" >&2
}

compute_rate_limit_sleep() {
  local log_file="$1"
  local default_secs=615
  # crates.io 429 body often contains: "Please try again after <RFC 2822 date>."
  local after_str
  after_str="$(sed -n 's/.*Please try again after \([^.]*\).*/\1/p' "$log_file" | head -n 1 || true)"
  if [ -n "$after_str" ]; then
    local target_epoch now_epoch diff
    target_epoch="$(date -u -d "$after_str" +%s 2>/dev/null || date -u -j -f "%a, %d %b %Y %H:%M:%S %Z" "$after_str" +%s 2>/dev/null || echo "")"
    now_epoch="$(date -u +%s)"
    if [ -n "$target_epoch" ] && [ "$target_epoch" -gt "$now_epoch" ]; then
      diff=$((target_epoch - now_epoch + 15))
      if [ "$diff" -gt 0 ] && [ "$diff" -le 3600 ]; then
        printf '%s\n' "$diff"
        return 0
      fi
    fi
  fi
  printf '%s\n' "$default_secs"
}

echo "==> Publishing 13 crates to crates.io in topological order"
idx=0
for entry in "${CRATES[@]}"; do
  idx=$((idx + 1))
  IFS='|' read -r _dir _repo_pkg pub_name _lib_name _lic <<< "$entry"

  if crate_already_published "$pub_name" "$VERSION"; then
    printf '  [%2d/13] SKIP %s %s (already published on crates.io)\n' "$idx" "$pub_name" "$VERSION"
    continue
  fi

  attempt=0
  while :; do
    attempt=$((attempt + 1))
    pub_log="$(mktemp)"
    printf '  [%2d/13] cargo publish -p %s (attempt %d)\n' "$idx" "$pub_name" "$attempt"
    rc=0
    cargo publish --manifest-path "$STAGE_MANIFEST_HOST" --no-verify --allow-dirty -p "$pub_name" >"$pub_log" 2>&1 || rc=$?
    cat "$pub_log"

    if [ "$rc" -eq 0 ]; then
      rm -f "$pub_log"
      wait_for_index "$pub_name" "$VERSION"
      break
    fi

    if grep -qi 'already uploaded\|already exists' "$pub_log"; then
      rm -f "$pub_log"
      printf '  [%2d/13] SKIP %s %s (already uploaded)\n' "$idx" "$pub_name" "$VERSION"
      break
    fi

    if grep -qi '429\|Too Many Requests\|rate limit\|Please try again after' "$pub_log"; then
      if [ "$DO_WAIT" -eq 0 ]; then
        rm -f "$pub_log"
        echo "error: crates.io rate limit reached; re-run with --publish --wait (already-published crates are skipped)" >&2
        exit 1
      fi
      if [ "$attempt" -ge 20 ]; then
        rm -f "$pub_log"
        printf 'error: exceeded max rate-limit retries for %s\n' "$pub_name" >&2
        exit 1
      fi
      wait_secs="$(compute_rate_limit_sleep "$pub_log")"
      rm -f "$pub_log"
      printf '    rate limited by crates.io; sleeping %ss before retrying %s...\n' "$wait_secs" "$pub_name"
      sleep "$wait_secs"
      continue
    fi

    rm -f "$pub_log"
    printf 'error: cargo publish failed for %s (exit %d)\n' "$pub_name" "$rc" >&2
    exit "$rc"
  done
done

printf '\n==> Published all 13 crates at %s (commit %s).\n' "$VERSION" "$SHORT_SHA"
