#!/usr/bin/env python3
"""Rewrites a vendored highlight.js theme CSS file (`github.min.css`/
`github-dark.min.css`, minified) into this app's light/dark cascade.
Called by `vendor-hljs.sh` -- not meant to be run standalone against
anything else, the two hardcoded rewrites below are specific to how
`hljs-light.css`/`hljs-dark.css` need to look.

- light: vendored close to verbatim (unguarded -- it's the default, same
  as `light.css`'s own custom properties), minus `.hljs`'s own
  `background` (this app's own `pre.blob`/`pre.diff` already paint
  `--code-bg` on the surrounding `<pre>`, see `base.css`).
- dark: every color selector duplicated under two guards -- one inside
  `@media (prefers-color-scheme: dark)` (the "OS default, no explicit
  override" case) and one under a bare `:root[data-theme='dark']`
  attribute selector (the explicit override, must apply regardless of OS
  preference) -- the same two-branch shape `dark.css` itself needs,
  spelled out as real selectors here instead of bare declarations
  (a `.hljs-keyword{...}` rule can't be dropped inside another selector's
  `{ }` without CSS nesting syntax the way a `--x: value;` declaration
  can). Its two theme-independent layout resets (`pre code.hljs`/
  `code.hljs`: padding/overflow, identical either theme) are skipped here
  and left living in `hljs-light.css` only.
"""

import re
import sys

THEME_INDEPENDENT = {"pre code.hljs", "code.hljs"}


def parse_rules(css: str) -> list[tuple[str, str]]:
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    rules = []
    for selectors, decls in re.findall(r"([^{}]+)\{([^{}]*)\}", css):
        if any(s.strip() == ".hljs" for s in selectors.split(",")):
            decls = ";".join(d for d in decls.split(";") if d and not d.strip().startswith("background"))
        for sel in selectors.split(","):
            sel = sel.strip()
            if sel:
                rules.append((sel, decls))
    return rules


def render_light(rules: list[tuple[str, str]]) -> str:
    header = (
        "/* highlight.js light theme (github, cdnjs), vendored verbatim\n"
        "   except `.hljs`'s own `background` (dropped -- `pre.blob`/`pre.diff` in\n"
        "   base.css already paint `--code-bg` on the surrounding `<pre>`, see\n"
        "   `scripts/vendor-hljs.sh`). Unguarded: this is the default, same as\n"
        "   `light.css`'s own variables -- `hljs-dark.css` is the guarded override,\n"
        "   except this file's own layout resets (`pre code.hljs`/`code.hljs`:\n"
        "   padding/overflow, identical in both themes), which live here only. */"
    )
    return "\n".join([header] + [f"{sel}{{{decls}}}" for sel, decls in rules])


def render_dark(rules: list[tuple[str, str]]) -> str:
    header = (
        "/* highlight.js dark theme (github-dark, cdnjs) -- selectors\n"
        "   mechanically re-prefixed by scripts/vendor-hljs.sh from the unmodified\n"
        "   upstream file (`.hljs`'s own `background` dropped, same reasoning as\n"
        "   `hljs-light.css`; its theme-independent layout resets skipped, they\n"
        "   live in `hljs-light.css` only) so it slots into this app's existing\n"
        "   light-is-default/dark-is-override cascade (see dark.css's own doc\n"
        "   comment) without any special-casing at the page_chrome.rs include\n"
        "   site: same guard conditions as every other theme file, just written as\n"
        "   real selectors instead of bare custom properties (a plain\n"
        "   `.hljs-keyword{...}` rule can't be dropped inside a\n"
        "   `:root[data-theme='dark']{ }` block the way a `--x: value;` declaration\n"
        "   can, without CSS nesting syntax this project otherwise avoids). */"
    )
    color_rules = [(s, d) for s, d in rules if s not in THEME_INDEPENDENT]
    lines = [header, "@media (prefers-color-scheme: dark) {"]
    lines += [f":root:not([data-theme='light']) {sel}{{{decls}}}" for sel, decls in color_rules]
    lines.append("}")
    lines += [f":root[data-theme='dark'] {sel}{{{decls}}}" for sel, decls in color_rules]
    return "\n".join(lines)


def main() -> None:
    which, path = sys.argv[1], sys.argv[2]
    rules = parse_rules(open(path, encoding="utf-8").read())
    print(render_light(rules) if which == "light" else render_dark(rules))


if __name__ == "__main__":
    main()
