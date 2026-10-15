#!/usr/bin/env bash
# The project's coding practices (CLAUDE.md, Code) that clippy and rustfmt
# don't cover, checked with cheap text heuristics. A heuristic can't read
# intent, so each rule aims at the plain form of a mistake and leaves the
# judgement calls to review: `fail` findings block a commit, `review`
# findings are printed for a human to look at and never fail the run.
#
#   scripts/check-practices.sh          what is staged (pre-commit)
#   scripts/check-practices.sh --all    every tracked .rs file
#   GITHUB_BASE_REF=… (CI)              the PR's diff against its base
set -euo pipefail

failures=0
reviews=0
fail() {
    echo "PRACTICE ($1): $2"
    failures=$((failures + 1))
}
review() {
    echo "REVIEW ($1): $2"
    reviews=$((reviews + 1))
}

# Prints `line: text` for each line of file $2 matching ERE $1 that carries
# no reason: neither a `//` comment on the same line nor a `//` block right
# above it. Lets a deliberate exception (CLI stdout, a fallback where every
# failure means the same) pass once it is explained, instead of forever.
unexplained() {
    awk -v pat="$1" '
        /^[[:space:]]*\/\/[^\/!]/ { explained = 1; next }
        {
            if ($0 ~ pat && !explained && $0 !~ /\/\/[^\/!]/) print FNR ": " $0
            explained = 0
        }
    ' "$2"
}

# Prints `line: comment` for each one-line `//` comment whose words all
# reappear in the code line under it (`// increment the counter` over
# `counter += 1`): it restates that code. Doc comments, multi-line blocks
# and TODO/FIXME/SAFETY-style markers are left to review, since a reason
# needs words the code doesn't have and a heuristic can't weigh them.
restating_comments() {
    awk '
        function words(text, out,    n, i, c, buf, parts, k) {
            # Splits identifiers on `_` and camelCase, lowercases, and keeps
            # words of 3+ letters, so `lastSyncedSha` and `last synced sha` match.
            buf = ""
            n = length(text)
            for (i = 1; i <= n; i++) {
                c = substr(text, i, 1)
                if (c ~ /[A-Z]/ && i > 1 && substr(text, i - 1, 1) ~ /[a-z0-9]/) buf = buf " "
                buf = buf (c ~ /[A-Za-z0-9]/ ? c : " ")
            }
            n = split(tolower(buf), parts, " ")
            delete out
            k = 0
            for (i = 1; i <= n; i++) {
                if (length(parts[i]) < 3 || parts[i] in stop) continue
                out[stem(parts[i])] = 1
                k++
            }
            return k
        }
        function stem(w) {
            if (w ~ /ing$/ && length(w) > 5) return substr(w, 1, length(w) - 3)
            if (w ~ /ed$/ && length(w) > 4) return substr(w, 1, length(w) - 2)
            if (w ~ /es$/ && length(w) > 4) return substr(w, 1, length(w) - 2)
            if (w ~ /s$/ && length(w) > 3) return substr(w, 1, length(w) - 1)
            return w
        }
        BEGIN {
            split("the and for with from into this that its are was were has have will can not " \
                  "all each any one new our out get set let mut pub self use via then else when", s, " ")
            for (i in s) stop[s[i]] = 1
        }
        /^[[:space:]]*\/\/[^\/!]/ || /^[[:space:]]*\/\/$/ {
            lines++
            text = $0
            sub(/^[[:space:]]*\/\/[[:space:]]*/, "", text)
            at = FNR
            next
        }
        {
            if (lines == 1 && $0 ~ /[^[:space:]]/ \
                && text !~ /^(TODO|FIXME|NOTE|SAFETY|HACK|XXX|WARN)/) {
                total = words(text, said)
                words($0, code)
                hit = 0
                for (w in said) if (w in code) hit++
                if (total >= 2 && hit == total) print at ": // " text
            }
            lines = 0
        }
    ' "$1"
}

if [ "${1:-}" = "--all" ]; then
    range="origin/master..HEAD"
    files=$(git ls-files -- '*.rs')
elif [ -n "${GITHUB_BASE_REF:-}" ]; then
    git fetch --depth=1 origin "$GITHUB_BASE_REF" >/dev/null 2>&1
    range="origin/${GITHUB_BASE_REF}...HEAD"
    files=$(git diff --name-only --diff-filter=ACM "$range" -- '*.rs')
else
    range="origin/master..HEAD"
    files=$(git diff --cached --name-only --diff-filter=ACM -- '*.rs')
fi

for f in $files; do
    # Comments must tell a technical reader something the code doesn't.
    while IFS= read -r hit; do
        [ -n "$hit" ] && fail "comment-restates-code" "$f:$hit"
    done < <(restating_comments "$f")

    # An `# Arguments` doc section is only worth keeping when some entry
    # states a contract; one that just names each parameter repeats the
    # signature.
    if grep -qE '^\s*///\s*#\s*[Aa]rguments' "$f"; then
        contractual=$(sed -n '/^\s*\/\/\/\s*#\s*[Aa]rguments/,/^\s*\/\/\/\s*#\s*[^Aa]/p' "$f" \
            | grep -iE '`[a-z_]+`\s*[-:]' \
            | grep -iE '(must|should|may not|cannot|ownership|transferred|borrowed|valid|non-empty|null|aligned|exclusive|shared|lifetime|outlives)' \
            || true)
        [ -z "$contractual" ] && fail "doc-restates-signature" "$f: # Arguments only restates the parameter types"
    fi

    # A primitive or String annotation that the initializer already fixes.
    # One-line statements only: a chain continuing below (`.parse()`,
    # `.collect()`) needs the annotation, and so does a bare numeric literal,
    # whose type nothing else pins.
    while IFS= read -r hit; do
        [ -n "$hit" ] && fail "trivial-type-annotation" "$f:$hit"
    done < <(grep -nE 'let\s+(mut\s+)?\w+\s*:\s*(u8|u16|u32|u64|i8|i16|i32|i64|f32|f64|bool|String)\s*=.*;\s*$' "$f" \
        | grep -vE '=\s*-?[0-9]' \
        | grep -vE '\b(as|From|from|into|parse|try_from|sum|product|collect|default)\b' || true)

    # `fn f<'a>(x: &'a T) -> &'a U` where `'a` is the only reference
    # parameter: elision gives the same signature. With a second reference
    # the output's source is ambiguous and `'a` is required.
    while IFS= read -r hit; do
        [ -n "$hit" ] && fail "explicit-lifetime-elision" "$f:$hit"
    done < <(awk '
        match($0, /fn[[:space:]]+[[:alnum:]_]+<[[:space:]]*'\''a[[:space:]]*>[[:space:]]*[(]/) {
            rest = substr($0, RSTART + RLENGTH); depth = 1; params = ""
            for (i = 1; i <= length(rest) && depth > 0; i++) {
                c = substr(rest, i, 1)
                if (c == "(") depth++
                else if (c == ")") depth--
                if (depth > 0) params = params c
            }
            refs = gsub(/&/, "&", params)
            ticks = gsub(/'\''a/, "'\''a", params)
            if (refs == 1 && ticks == 1 && params ~ /&[[:space:]]*'\''a[[:space:]]/ && $0 !~ /unsafe|extern|dyn/)
                print FNR ": " $0
        }
    ' "$f")

    # Blocking on a future from inside the async runtime stalls a worker.
    case "$f" in
        */tests/*) ;;
        *)
            while IFS= read -r hit; do
                [ -n "$hit" ] && fail "block-on-in-async" "$f:$hit"
            done < <(grep -nE 'block_on\s*\(' "$f" || true)
            ;;
    esac

    # Diagnostics go through `tracing`. A CLI's own stdout (--help,
    # --version, a command's report) is output, and passes once a comment
    # says so. Tests aren't production.
    case "$f" in
        */tests/*) ;;
        *)
            while IFS= read -r hit; do
                [ -n "$hit" ] && fail "println-in-production" "$f:$hit"
            done < <(unexplained '(e?println|e?print)![[:space:]]*[(]' "$f")
            ;;
    esac

    # Errors carry meaning: a catch-all over every failure needs a reason
    # why they all mean the same thing here.
    while IFS= read -r hit; do
        [ -n "$hit" ] && fail "wildcard-error-match" "$f:$hit"
    done < <(unexplained 'Err[(]_[)][[:space:]]*=>' "$f")

    # A counter or flag wants an atomic, not a lock.
    while IFS= read -r hit; do
        [ -n "$hit" ] && fail "mutex-on-primitive" "$f:$hit"
    done < <(grep -nE 'Mutex<(u8|u16|u32|u64|i8|i16|i32|i64|usize|isize|bool)>' "$f" || true)

    # Review only: whether a clone could be a borrow depends on lifetimes
    # the text doesn't show.
    while IFS= read -r hit; do
        [ -n "$hit" ] && review "possible-borrow-instead-of-clone" "$f:$hit"
    done < <(grep -nE '\.clone\(\)\s*$' "$f" | grep -vE '_owned|_cloned|to_send|Arc|Rc' || true)

    # Review only: more than 4 behavioural derives. Debug, serde and s3db's
    # Entity are structural and don't count (kb decision #97); whether the
    # rest is used only the compiler can tell.
    while IFS= read -r hit; do
        [ -n "$hit" ] && review "excessive-derives" "$f:$hit"
    done < <(awk '
        match($0, /#\[derive\(([^)]*)\)\]/) {
            inner = substr($0, RSTART + 9, RLENGTH - 11)
            n = split(inner, d, ",")
            k = 0
            for (i = 1; i <= n; i++) {
                gsub(/[[:space:]]/, "", d[i])
                if (d[i] != "" && d[i] !~ /^(Debug|Serialize|Deserialize|Entity)$/) k++
            }
            if (k > 4) print FNR ": " $0
        }
    ' "$f")
done

# Commit messages carry no AI co-author or attribution line.
for sha in $(git log --format='%H' "$range" 2>/dev/null || git log --format='%H' -5); do
    if git log -1 --format='%B' "$sha" | grep -qiE 'co-authored-by.*(claude|copilot|chatgpt|openai|gemini|cursor|aider)'; then
        fail "ai-attribution" "commit $sha has an AI co-author line"
    fi
done

if [ "$failures" -gt 0 ] || [ "$reviews" -gt 0 ]; then
    echo ""
    echo "$failures to fix, $reviews to review. See CLAUDE.md, Code."
fi
[ "$failures" -eq 0 ]
