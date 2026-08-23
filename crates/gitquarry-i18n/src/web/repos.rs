//! `webui::repos`'s page copy — the logged-in viewer's own repo dashboard.

use super::super::{Locale, msg};

msg!(repos_title() { Locale::Es => "repos", Locale::En => "repos" });
msg!(repos_tab_yours() { Locale::Es => "tuyos", Locale::En => "yours" });
msg!(repos_tab_groups() { Locale::Es => "grupos", Locale::En => "groups" });
msg!(repos_tab_shared() { Locale::Es => "compartidos", Locale::En => "shared" });
msg!(repos_tab_explore() { Locale::Es => "explorar", Locale::En => "explore" });

msg!(repos_yours_heading() { Locale::Es => "en tu espacio personal", Locale::En => "in your own namespace" });
msg!(repos_none_yours() {
    Locale::Es => "(todavía no tienes repos propios)",
    Locale::En => "(you don't own any repo yet)",
});
msg!(repos_none_groups() {
    Locale::Es => "(no perteneces a ningún grupo)",
    Locale::En => "(you don't belong to any group)",
});

msg!(
    /// The "shared" tab lists repos reached through an explicit per-repo
    /// grant only — group access has its own tab, and mixing the two would
    /// hide which repos actually depend on a grant that can be revoked
    /// one at a time.
    repos_shared_prose() {
        Locale::Es => "Repos donde tienes un permiso otorgado directamente, fuera de tus grupos.",
        Locale::En => "Repos where you hold a permission granted directly to you, outside your groups.",
    }
);
msg!(repos_none_shared() {
    Locale::Es => "(nadie te ha otorgado acceso a un repo suelto)",
    Locale::En => "(nobody granted you access to an individual repo)",
});
msg!(repos_new_link() { Locale::Es => "nuevo repo", Locale::En => "new repo" });
