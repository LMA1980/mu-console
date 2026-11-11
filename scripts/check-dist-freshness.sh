#!/usr/bin/env bash
# scripts/check-dist-freshness.sh: Verify that dist/ is up-to-date with code/ui and code/data
set -euo pipefail

cd "$(dirname "$0")/.."

echo "Verifying dist/ directory completeness..."
for f in dist/muconsole-ui_bg.wasm dist/muconsole-data_bg.wasm dist/muconsole-ui.js dist/muconsole-data.js dist/muconsole-data_loader.js dist/tokens.css dist/index.html; do
    if [ ! -f "$f" ]; then
        echo "ERROR: Missing required dist artifact: $f" >&2
        exit 1
    fi
    if [ ! -s "$f" ]; then
        echo "ERROR: Dist artifact is empty: $f" >&2
        exit 1
    fi
done

if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    DIST_TS=$(git log -1 --format="%ct" -- dist/muconsole-ui_bg.wasm 2>/dev/null || echo 0)
    UI_TS=$(git log -1 --format="%ct" -- code/ui 2>/dev/null || echo 0)
    DATA_TS=$(git log -1 --format="%ct" -- code/data 2>/dev/null || echo 0)

    if [ "$UI_TS" -gt "$DIST_TS" ] || [ "$DATA_TS" -gt "$DIST_TS" ]; then
        echo "ERROR: dist/ is older than code/ui or code/data submodules in git history!" >&2
        echo "  dist/ timestamp:     $DIST_TS" >&2
        echo "  code/ui timestamp:   $UI_TS" >&2
        echo "  code/data timestamp: $DATA_TS" >&2
        echo "Run ./scripts/build-dist.sh and commit the updated dist/ files." >&2
        exit 1
    fi

    if [ -n "$(git status --porcelain dist/)" ]; then
        echo "ERROR: dist/ has uncommitted modifications or drift." >&2
        git status --porcelain dist/ >&2
        exit 1
    fi
fi

echo "dist/ freshness check passed successfully."
