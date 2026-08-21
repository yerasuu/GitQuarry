//! Labels for `webui::mod`'s shared chrome (topnav, admin/account tab bars,
//! repo header tab bar) — the one place every page routes through. Kept
//! separate from the tab **ids** (`"settings"`, `"groups"`, etc.), which
//! stay plain English string constants forever: those ids are also used as
//! `tabs_bar`'s active-tab match key, and a past sweep silently broke
//! active-tab highlighting by translating a label that doubled as its own
//! match key. Never route an id through here.

use super::super::{Locale, msg};

msg!(settings_tab() { Locale::Es => "ajustes", Locale::En => "settings" });
msg!(users_tab() { Locale::Es => "usuarios", Locale::En => "users" });
msg!(groups_tab() { Locale::Es => "grupos", Locale::En => "groups" });
msg!(audit_tab() { Locale::Es => "auditoria", Locale::En => "audit" });

msg!(profile_tab() { Locale::Es => "perfil", Locale::En => "profile" });
msg!(favorites_tab() { Locale::Es => "favoritos", Locale::En => "favorites" });
msg!(keys_tab() { Locale::Es => "claves", Locale::En => "keys" });
msg!(security_tab() { Locale::Es => "seguridad", Locale::En => "security" });
msg!(pat_tab() { Locale::Es => "tokens personales", Locale::En => "personal tokens" });
msg!(account_settings_tab() { Locale::Es => "ajustes", Locale::En => "settings" });
msg!(
    /// `/_account/profile` sidebar entry — avatar/public-profile-facing
    /// settings, distinct from [`profile_tab`] (the public repo-listing page
    /// itself, never part of the settings sidebar).
    account_profile_section() {
        Locale::Es => "perfil",
        Locale::En => "profile",
    }
);
msg!(account_appearance_section() { Locale::Es => "apariencia", Locale::En => "appearance" });
msg!(settings_sidebar_nav_label() { Locale::Es => "navegacion de ajustes de cuenta", Locale::En => "account settings navigation" });

msg!(about_tab() { Locale::Es => "acerca de", Locale::En => "about" });
msg!(summary_tab() { Locale::Es => "resumen", Locale::En => "summary" });
msg!(refs_tab() { Locale::Es => "referencias", Locale::En => "refs" });
msg!(log_tab() { Locale::Es => "historial", Locale::En => "log" });
msg!(tree_tab() { Locale::Es => "arbol", Locale::En => "tree" });
msg!(stats_tab() { Locale::Es => "estadisticas", Locale::En => "stats" });
msg!(releases_tab() { Locale::Es => "lanzamientos", Locale::En => "releases" });
msg!(merges_tab() { Locale::Es => "merge requests", Locale::En => "merge requests" });

msg!(repos_nav() { Locale::Es => "repos", Locale::En => "repos" });
msg!(new_repo_nav() { Locale::Es => "nuevo repo", Locale::En => "new repo" });
msg!(skip_to_content() { Locale::Es => "saltar al contenido", Locale::En => "skip to content" });
msg!(menu_toggle_label() { Locale::Es => "menu", Locale::En => "menu" });
msg!(page_tabs_nav_label() { Locale::Es => "secciones de la pagina", Locale::En => "page sections" });
msg!(page_tabs_toggle_label() { Locale::Es => "menu de secciones de la pagina", Locale::En => "page sections menu" });
msg!(breadcrumb_nav_label() { Locale::Es => "ruta", Locale::En => "breadcrumb" });

msg!(archived_notice() {
    Locale::Es => "archivado",
    Locale::En => "archived",
});
msg!(archived_notice_detail() {
    Locale::Es => "rechaza pushes, se mantiene de solo lectura",
    Locale::En => "rejects pushes, stays read-only",
});
msg!(owner_label() { Locale::Es => "propietario", Locale::En => "owner" });

msg!(limited_visibility_badge() { Locale::Es => "limitado", Locale::En => "limited" });
msg!(private_visibility_badge() { Locale::Es => "privado", Locale::En => "private" });
msg!(changelog_nav() { Locale::Es => "changelog", Locale::En => "changelog" });
msg!(changelog_heading() { Locale::Es => "cambios", Locale::En => "changelog" });
msg!(changelog_running_version(version: &str) {
    Locale::Es => "Esta instancia ejecuta la version {version}. Estas notas son las de ese binario, no las de la ultima publicada.",
    Locale::En => "This instance runs version {version}. These notes are that binary's own, not the latest published ones.",
});
msg!(footer_copyright(year: &str, holder: &str) {
    Locale::Es => "© {year} {holder}",
    Locale::En => "© {year} {holder}",
});
msg!(license_heading() { Locale::Es => "licencia", Locale::En => "license" });
