//! `webui::repo::refs`'s page copy.

use super::super::super::{Locale, msg};

msg!(branches_heading() { Locale::Es => "ramas", Locale::En => "branches" });
msg!(tags_heading() { Locale::Es => "etiquetas", Locale::En => "tags" });

msg!(create_tag_heading() { Locale::Es => "crear etiqueta", Locale::En => "create tag" });
msg!(tag_name_label() { Locale::Es => "nombre", Locale::En => "name" });
msg!(tag_name_placeholder() { Locale::Es => "v1.0", Locale::En => "v1.0" });
msg!(tag_source_label() { Locale::Es => "desde", Locale::En => "from" });
msg!(create_tag_button() { Locale::Es => "crear etiqueta", Locale::En => "create tag" });
msg!(delete_tag_button() { Locale::Es => "eliminar", Locale::En => "delete" });
msg!(delete_branch_button() { Locale::Es => "eliminar", Locale::En => "delete" });

msg!(
    /// `webui::repo::tags::handler_create_post`: `source` doesn't resolve to an
    /// existing commit in this repo (typo, or a rev from a different repo the
    /// submitter had open in another tab).
    invalid_tag_source(source: &str) {
        Locale::Es => "'{source}' no resuelve a un commit existente en este repo",
        Locale::En => "'{source}' doesn't resolve to an existing commit in this repo",
    }
);

msg!(delete_tag_confirm(tag: &str) {
    Locale::Es => "¿eliminar la etiqueta '{tag}'? no se puede deshacer.",
    Locale::En => "delete tag '{tag}'? this cannot be undone.",
});
msg!(delete_branch_confirm(branch: &str) {
    Locale::Es => "¿eliminar la rama '{branch}'? no se puede deshacer.",
    Locale::En => "delete branch '{branch}'? this cannot be undone.",
});
