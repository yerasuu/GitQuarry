use super::super::super::{Locale, msg};

msg!(commits_by_author_heading() { Locale::Es => "commits por autor", Locale::En => "commits by author" });
msg!(commits_by_month_heading(n: usize) {
    Locale::Es => "commits por mes (ultimos {n})",
    Locale::En => "commits by month (last {n})",
});
msg!(no_data_hint() { Locale::Es => "(sin datos)", Locale::En => "(no data)" });
