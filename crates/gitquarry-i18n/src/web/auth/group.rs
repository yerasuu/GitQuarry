//! `webui::auth::web::group`'s page copy — the per-group page.

use super::super::super::{Locale, msg};

msg!(group_subgroups_heading() { Locale::Es => "subgrupos", Locale::En => "subgroups" });
msg!(group_repos_heading() { Locale::Es => "repositorios", Locale::En => "repositories" });
msg!(group_no_subgroups() { Locale::Es => "(sin subgrupos)", Locale::En => "(no subgroups)" });
msg!(group_no_repos_visible() {
    Locale::Es => "(no hay repos visibles para ti en este grupo)",
    Locale::En => "(no repos here are visible to you)",
});
msg!(group_manage_link() { Locale::Es => "administrar el grupo", Locale::En => "manage this group" });

msg!(
    /// Why the member list isn't on the page for someone outside the group —
    /// who belongs to a group is only shown to the group itself, same
    /// reasoning its private repos aren't listed either.
    group_members_hidden() {
        Locale::Es => "(solo los miembros del grupo ven la lista de miembros)",
        Locale::En => "(only the group's own members see who belongs to it)",
    }
);

msg!(
    /// The role a member holds here comes from an ancestor group, not from
    /// this one — GitLab shows the same distinction, and without it a
    /// subgroup's page looks like it granted access it never did.
    group_inherited_role_suffix() {
        Locale::Es => " (heredado)",
        Locale::En => " (inherited)",
    }
);
