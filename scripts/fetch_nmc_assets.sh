#!/usr/bin/env bash
# =============================================================================
# fetch_nmc_assets.sh - NMC model asset fetcher (WI-C2; ADR-196 D4 preflight)
# =============================================================================
# Parity twin of scripts/fetch_nmc_assets.ps1: same manifest, same row checks,
# same exit codes, ASCII-only on both sides (G-85/PS5.1 discipline).
#
# Behavior:
#   1. Parse assets/nmc/manifest.toml ([[model]] rows).
#   2. Per row: READY (url and sha256 both set) or PENDING (both empty).
#      A half-filled row is malformed input -> exit 2.
#   3. Default is a DRY RUN: print the plan, touch nothing, exit 0.
#   4. --apply downloads READY rows into TARGET_DIR (default
#      assets/nmc/models), verifies SHA-256, fails loudly on mismatch.
#      PENDING rows are skipped with an explicit message. Model binaries are
#      NEVER committed (see .gitignore: assets/nmc/models/).
#
# Usage: bash scripts/fetch_nmc_assets.sh [MANIFEST] [TARGET_DIR] [--apply]
# Exit codes: 0 ok / 1 apply failure (download or hash mismatch) / 2 malformed
# input (manifest unreadable, no rows, bad row).
set -eu

MANIFEST="assets/nmc/manifest.toml"
TARGET_DIR="assets/nmc/models"
APPLY=0
POS=0
for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=1 ;;
        *)
            POS=$((POS + 1))
            if [ "$POS" -eq 1 ]; then MANIFEST="$arg"; fi
            if [ "$POS" -eq 2 ]; then TARGET_DIR="$arg"; fi
            ;;
    esac
done

if [ ! -f "$MANIFEST" ]; then
    echo "ERROR: manifest not found: $MANIFEST"
    exit 2
fi

# ---- parse [[model]] rows (main shell; here-string keeps control flow real) --
ROWS=""
CUR_TYPE=""; CUR_FILE=""; CUR_URL=""; CUR_SHA=""; IN_ROW=0

flush_row() {
    if [ "$IN_ROW" = "1" ]; then
        ROWS="${ROWS}${CUR_TYPE}|${CUR_FILE}|${CUR_URL}|${CUR_SHA}
"
    fi
}

while IFS= read -r raw || [ -n "$raw" ]; do
    line="$(printf '%s' "$raw" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    case "$line" in
        "[[model]]")
            flush_row
            CUR_TYPE=""; CUR_FILE=""; CUR_URL=""; CUR_SHA=""; IN_ROW=1
            continue
            ;;
        ""|\#*) continue ;;
    esac
    if [ "$IN_ROW" != "1" ]; then continue; fi
    key="$(printf '%s' "$line" | sed -n 's/^\([A-Za-z_]*\)[[:space:]]*=.*/\1/p')"
    val="$(printf '%s' "$line" | sed -n 's/^[A-Za-z_]*[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p')"
    case "$key" in
        model_type) CUR_TYPE="$val" ;;
        file) CUR_FILE="$val" ;;
        url) CUR_URL="$val" ;;
        sha256) CUR_SHA="$val" ;;
    esac
done < "$MANIFEST"
flush_row

ROW_COUNT="$(printf '%s' "$ROWS" | grep -c '|' || true)"
if [ "$ROW_COUNT" -eq 0 ]; then
    echo "ERROR: no [[model]] rows in $MANIFEST"
    exit 2
fi

# ---- validate + plan (main shell; exit in-loop must really exit) -------------
READY=0
PENDING=0
echo "== NMC asset plan ($MANIFEST) =="
while IFS='|' read -r mt file url sha; do
    [ -z "$mt" ] && continue
    case "$mt" in
        Image|Video|Audio|Text) ;;
        *) echo "ERROR: unknown model_type '$mt'"; exit 2 ;;
    esac
    if { [ -n "$url" ] && [ -z "$sha" ]; } || { [ -z "$url" ] && [ -n "$sha" ]; }; then
        echo "ERROR: half-filled row ($mt): url and sha256 must be set together"
        exit 2
    fi
    if [ -n "$url" ]; then
        READY=$((READY + 1))
        printf '  READY   %s -> %s <- %s\n' "$mt" "$file" "$url"
    else
        PENDING=$((PENDING + 1))
        printf '  PENDING %s -> %s (asset source/license not adjudicated yet)\n' "$mt" "$file"
    fi
done <<< "$ROWS"
echo "  ready=$READY pending=$PENDING apply=$APPLY"

if [ "$APPLY" != "1" ]; then
    echo "DRY RUN (pass --apply to download READY rows)"
    exit 0
fi

# ---- apply: download + SHA-256 verify ---------------------------------------
mkdir -p "$TARGET_DIR"

while IFS='|' read -r mt file url sha; do
    [ -z "$mt" ] && continue
    if [ -z "$url" ]; then
        echo "SKIP  $mt: PENDING (no url/sha256 yet)"
        continue
    fi
    dest="$TARGET_DIR/$file"
    echo "GET   $mt: $url -> $dest"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL -o "$dest" "$url" || { echo "ERROR: download failed ($mt)"; exit 1; }
    elif command -v wget > /dev/null 2>&1; then
        wget -q -O "$dest" "$url" || { echo "ERROR: download failed ($mt)"; exit 1; }
    else
        echo "ERROR: neither curl nor wget available"
        exit 1
    fi
    if command -v sha256sum > /dev/null 2>&1; then
        actual="$(sha256sum "$dest" | awk '{print $1}')"
    else
        actual="$(shasum -a 256 "$dest" | awk '{print $1}')"
    fi
    expect="$(printf '%s' "$sha" | tr '[:upper:]' '[:lower:]')"
    if [ "$actual" != "$expect" ]; then
        echo "ERROR: SHA-256 mismatch ($mt): expected $expect got $actual"
        rm -f "$dest" 2>/dev/null || true
        exit 1
    fi
    echo "OK    $mt: verified $actual"
done <<< "$ROWS"

echo "DONE: all READY rows fetched and verified"
exit 0
