#!/usr/bin/env bash
# Re-vendors highlight.js + its line-numbers plugin + both theme CSS files
# into crates/gitquarry-webui/src/webui/style/vendor/, the same "pinned by
# tag, re-run after a bump" story `styler.js`/`styler.css` already follow
# (see `page_chrome.rs`'s `STYLE` doc comment). Bump HLJS_VERSION/
# LINE_NUMBERS_VERSION below, then run this from the repo root.
#
# The two theme CSS files are the one non-literal step: hljs's own
# `github`/`github-dark` themes ship as bare, unscoped `.hljs-*` selectors,
# so this script rewrites them to fit this app's existing light-is-default/
# dark-is-override cascade (see `hljs-light.css`/`hljs-dark.css`'s own
# header comments for the exact reasoning) instead of hand-editing the
# generated output after every bump.
set -euo pipefail

HLJS_VERSION="11.10.0"
LINE_NUMBERS_VERSION="2.9.0"
BASE="https://cdnjs.cloudflare.com/ajax/libs"
OUT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/crates/gitquarry-webui/src/webui/style/vendor"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -sL -o "$OUT/hljs.js" "$BASE/highlight.js/$HLJS_VERSION/highlight.min.js"
curl -sL -o "$OUT/hljs-line-numbers.js" "$BASE/highlightjs-line-numbers.js/$LINE_NUMBERS_VERSION/highlightjs-line-numbers.min.js"
curl -sL -o "$TMP/github.min.css" "$BASE/highlight.js/$HLJS_VERSION/styles/github.min.css"
curl -sL -o "$TMP/github-dark.min.css" "$BASE/highlight.js/$HLJS_VERSION/styles/github-dark.min.css"

python3 "$(dirname "${BASH_SOURCE[0]}")/gen-hljs-theme.py" light "$TMP/github.min.css" > "$OUT/hljs-light.css"
python3 "$(dirname "${BASH_SOURCE[0]}")/gen-hljs-theme.py" dark "$TMP/github-dark.min.css" > "$OUT/hljs-dark.css"

echo "Vendored highlight.js $HLJS_VERSION + line-numbers $LINE_NUMBERS_VERSION into $OUT"
