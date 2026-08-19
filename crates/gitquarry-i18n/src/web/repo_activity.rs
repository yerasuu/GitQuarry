//! `webui::repo::activity`'s page copy.

use super::super::{Locale, msg};

msg!(recent_commits_heading() { Locale::Es => "commits recientes", Locale::En => "recent commits" });
msg!(events_heading() { Locale::Es => "eventos", Locale::En => "events" });
msg!(when_column() { Locale::Es => "cuando", Locale::En => "when" });
msg!(who_column() { Locale::Es => "quien", Locale::En => "who" });
msg!(action_column() { Locale::Es => "accion", Locale::En => "action" });
msg!(detail_column() { Locale::Es => "detalle", Locale::En => "detail" });
msg!(no_recent_events_hint() { Locale::Es => "(sin eventos recientes)", Locale::En => "(no recent events)" });
