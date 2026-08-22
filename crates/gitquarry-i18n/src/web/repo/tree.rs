//! `webui::repo::tree`'s page copy.

use super::super::super::{Locale, msg};

msg!(name_column() { Locale::Es => "nombre", Locale::En => "name" });
msg!(recent_commits_heading() { Locale::Es => "commits recientes", Locale::En => "recent commits" });
msg!(recent_commits_in_path() { Locale::Es => " en ", Locale::En => " in " });
msg!(full_log_link() { Locale::Es => "historial completo »", Locale::En => "full log »" });
msg!(submodule_hint() { Locale::Es => "(submodulo)", Locale::En => "(submodule)" });
msg!(submodule_at_url_hint() { Locale::Es => "(submodulo → ", Locale::En => "(submodule → " });
