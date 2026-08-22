//! `webui::repo::summary`'s page copy.

use super::super::super::{Locale, msg};

msg!(clone_heading() { Locale::Es => "clonar", Locale::En => "clone" });

msg!(forked_from_label() { Locale::Es => "bifurcado de", Locale::En => "forked from" });
msg!(branches_heading() { Locale::Es => "ramas", Locale::En => "branches" });
msg!(tags_heading() { Locale::Es => "etiquetas", Locale::En => "tags" });
msg!(all_branches_and_tags_link() {
    Locale::Es => "todas las ramas y etiquetas »",
    Locale::En => "all branches and tags »",
});
msg!(recent_commits_heading() { Locale::Es => "commits recientes", Locale::En => "recent commits" });
msg!(full_log_link() { Locale::Es => "historial completo »", Locale::En => "full log »" });

msg!(about_heading() { Locale::Es => "acerca de", Locale::En => "about" });
msg!(community_heading() { Locale::Es => "comunidad", Locale::En => "community" });
msg!(releases_heading() { Locale::Es => "lanzamientos", Locale::En => "releases" });
msg!(readme_label() { Locale::Es => "readme", Locale::En => "readme" });
msg!(contributing_label() { Locale::Es => "contributing", Locale::En => "contributing" });
msg!(license_label() { Locale::Es => "licencia", Locale::En => "license" });
msg!(activity_label() { Locale::Es => "actividad", Locale::En => "activity" });
msg!(stars_label() { Locale::Es => "estrellas", Locale::En => "stars" });
msg!(forks_label() { Locale::Es => "bifurcaciones", Locale::En => "forks" });
msg!(watchers_label() { Locale::Es => "observadores", Locale::En => "watchers" });
msg!(languages_heading() { Locale::Es => "lenguajes", Locale::En => "languages" });
msg!(languages_empty_hint() { Locale::Es => "(sin lenguajes detectados)", Locale::En => "(no languages detected)" });
msg!(not_found() { Locale::Es => "no encontrado", Locale::En => "not found" });
msg!(see_below_link() { Locale::Es => "ver mas abajo", Locale::En => "below" });
msg!(recent_activity_link() { Locale::Es => "actividad reciente", Locale::En => "recent activity" });
msg!(your_favorites_link() { Locale::Es => "tus favoritos", Locale::En => "your favorites" });
msg!(no_releases() { Locale::Es => "sin lanzamientos", Locale::En => "no releases" });
msg!(one_release() { Locale::Es => "1 lanzamiento", Locale::En => "1 release" });
msg!(n_releases(n: usize) { Locale::Es => "{n} lanzamientos", Locale::En => "{n} releases" });

msg!(you_suffix() { Locale::Es => "(tu)", Locale::En => "(you)" });
msg!(name_placeholder() { Locale::Es => "nombre", Locale::En => "name" });
msg!(fork_button() { Locale::Es => "bifurcar", Locale::En => "fork" });
