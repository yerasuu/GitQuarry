use gitquarry_webui::webui::repo::components::*;

#[test]
fn label_pill_embeds_the_color_inline() {
    let html = label_pill("bug", "#d73a4a").into_string();
    assert!(html.contains("#d73a4a"));
    assert!(html.contains("bug"));
}

#[test]
fn decoration_badges_split_on_comma() {
    let out = decoration_badges("HEAD -> main, tag: v1.0").into_string();
    assert!(out.contains("HEAD -&gt; main"));
    assert!(out.contains("tag: v1.0"));
}

#[test]
fn decoration_badges_empty_for_no_refs() {
    assert_eq!(decoration_badges("").into_string(), "");
}

#[test]
fn side_by_side_pairs_dels_against_adds_row_by_row() {
    let out = side_by_side_diff("@@ -1,2 +1,2 @@\n-old one\n-old two\n+new one\n context\n").into_string();
    // The hunk header, two change rows (old one/new one, old two/[empty]), one context row.
    assert_eq!(out.matches("<tr>").count(), 4);
    // No `+++`/`---` header in this fixture -> no path tracked -> the
    // `language-plaintext` fallback (`language_tag`'s own doc comment).
    assert!(out.contains(r#"<td class="diff-del-line"><code class="language-plaintext">old one</code></td>"#));
    assert!(out.contains(r#"<td class="diff-add-line"><code class="language-plaintext">new one</code></td>"#));
    assert!(out.contains(r#"<td class="diff-del-line"><code class="language-plaintext">old two</code></td><td></td>"#));
    assert!(out.contains(
        r#"<td><code class="language-plaintext">context</code></td><td><code class="language-plaintext">context</code></td>"#
    ));
}

#[test]
fn side_by_side_spans_hunk_and_file_headers_across_both_columns() {
    let out = side_by_side_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n").into_string();
    assert!(out.contains(r#"<td class="diff-meta" colspan="2">diff --git a/f b/f</td>"#));
    assert!(out.contains(r#"<td class="diff-meta" colspan="2">--- a/f</td>"#));
    assert!(out.contains(r#"<td class="diff-meta" colspan="2">+++ b/f</td>"#));
    assert!(out.contains(r#"<td class="diff-hunk" colspan="2">@@ -1 +1 @@</td>"#));
}

#[test]
fn diff_headers_win_over_add_del() {
    let out = color_diff("+++ b/x\n--- a/x\n@@ -1 +1 @@\n+new\n-old\n");
    assert!(out.contains("<span class=\"diff-meta\">+++ b/x</span>"));
    assert!(out.contains("<span class=\"diff-meta\">--- a/x</span>"));
    assert!(out.contains("<span class=\"diff-hunk\">@@ -1 +1 @@</span>"));
    // `x` has no extension -> `language-plaintext` (`language_tag`'s own doc
    // comment); the `+`/`-` marker sits outside the `<code>`, same as it did
    // when the line had no tokenizing wrapper at all.
    assert!(out.contains("<span class=\"diff-add-line\">+<code class=\"language-plaintext\">new</code></span>"));
    assert!(out.contains("<span class=\"diff-del-line\">-<code class=\"language-plaintext\">old</code></span>"));
}
