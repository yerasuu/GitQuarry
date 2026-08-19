//! The `stats` tab: commits-per-author and commits-per-month bar charts.

use super::STATS_LOG_LIMIT;
use anyhow::Result;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo_stats as t;
use maud::{Markup, html};
use std::collections::BTreeMap;
use std::path::Path as FsPath;

pub async fn render_stats(bare_repo: &FsPath, rev: &str, locale: Locale) -> Result<String> {
    let counts = git::browse::shortlog_counts(bare_repo, rev).await?;
    let commits = git::browse::log(bare_repo, rev, STATS_LOG_LIMIT, None).await?;
    let mut by_month: BTreeMap<String, u64> = BTreeMap::new();
    for c in &commits {
        // `date` is `--date=format:%Y/%m/%d`, so the first 7 chars are
        // always the `YYYY/MM` bucket.
        if let Some(month) = c.date.get(..7) {
            *by_month.entry(month.to_string()).or_insert(0) += 1;
        }
    }

    Ok(html! {
        h3 { (t::commits_by_author_heading(locale)) }
        (bar_table(counts.iter().map(|(n, author)| (*n, author.clone())), locale))
        h3 { (t::commits_by_month_heading(locale, commits.len())) }
        (bar_table(by_month.into_iter().map(|(month, n)| (n, month)), locale))
    }
    .into_string())
}

fn bar_table(rows: impl Iterator<Item = (u64, String)>, locale: Locale) -> Markup {
    let rows: Vec<(u64, String)> = rows.collect();
    let max = rows.iter().map(|(n, _)| *n).max().unwrap_or(1).max(1);
    html! {
        div class="table-scroll" {
        table class="list" {
            @if rows.is_empty() {
                tr { td { em { (t::no_data_hint(locale)) } } }
            }
            @for (n, label) in &rows {
                @let width = (*n as f64 / max as f64 * 100.0).round() as u32;
                tr class="bar-row" {
                    td { (label) }
                    td { (n) }
                    td style="width:50%" { span class="bar" style=(format!("width:{width}%")) {} }
                }
            }
        }
        }
    }
}
