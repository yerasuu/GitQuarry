//! `webui::auth::web::favorites`'s page copy.

use super::super::super::{Locale, msg};

msg!(
    /// `render_favorites_page`'s intro line — same for both the public and
    /// private variant of the sentence that follows it.
    starred_intro() {
        Locale::Es => "Repos que marcaste como favoritos — no cambia la visibilidad ni los permisos de nadie. ",
        Locale::En => "Repos you starred — doesn't change anyone's visibility or permissions. ",
    }
);

msg!(visibility_heading() { Locale::Es => "visibilidad", Locale::En => "visibility" });
msg!(
    /// `render_favorites_page`'s visibility-toggle section — the checkbox
    /// sits right below this, on the same page, now that the toggle isn't
    /// off on a separate preferences page anymore.
    visibility_description() {
        Locale::Es => "Si es publica, cualquiera puede ver tu lista en /_profile/<usuario>/favorites (filtrado a repos que esa persona ya puede ver — no expone repos privados/limitados que no deberia ver). Privado por defecto.",
        Locale::En => "If public, anyone can see your list at /_profile/<user>/favorites (filtered to repos that person can already see — doesn't expose private/limited repos they shouldn't see). Private by default.",
    }
);
msg!(public_checkbox_label() { Locale::Es => " lista de favoritos publica", Locale::En => " public favorites list" });

msg!(
    /// `render_favorites_page`'s empty-state hint: no starred repos yet.
    no_starred_repos_hint() {
        Locale::Es => "(todavia ninguno — usa el boton \"favorito\" en la pagina de un repo)",
        Locale::En => "(none yet — use the \"favorite\" button on a repo's page)",
    }
);

msg!(
    /// Table header shared by `render_favorites_page` and
    /// `render_public_favorites_page`'s repo-listing table.
    repo_column_header() {
        Locale::Es => "repo",
        Locale::En => "repo",
    }
);

msg!(
    /// `render_favorites_page`'s per-row unstar button.
    remove_button_label() {
        Locale::Es => "quitar",
        Locale::En => "remove",
    }
);

msg!(
    /// `render_public_favorites_page`'s intro line, up to the bolded
    /// username — split off from the username itself since that's wrapped
    /// in its own `<strong>` tag.
    public_favorites_of_prefix() {
        Locale::Es => "Favoritos publicos de ",
        Locale::En => "Public favorites of ",
    }
);

msg!(
    /// `render_public_favorites_page`'s empty-state hint: nothing the
    /// current viewer is allowed to see.
    no_favorites_visible_hint() {
        Locale::Es => "(ninguno visible)",
        Locale::En => "(none visible)",
    }
);
msg!(starred_heading() { Locale::Es => "repos marcados", Locale::En => "starred repos" });
