//! `webui::auth::web::groups`'s page copy.

use super::super::super::{Locale, msg};

msg!(all_group_repos_scope() {
    Locale::Es => "todos los repos del grupo",
    Locale::En => "every repo in the group",
});

// The intro paragraph is split into fragments around fixed `code` elements
// (the config path, the `<group>/<repo>` namespace shapes, the permission
// words) that stay untranslated identifiers or get their own catalog entry —
// each fragment keeps the same position relative to those elements in both
// locales so the rendered sentence still reads naturally once concatenated.
msg!(groups_prose_stored_in() { Locale::Es => "Se guarda uno por grupo en ", Locale::En => "Stored one per group in " });
msg!(groups_prose_inside_bucket() {
    Locale::Es => " dentro del bucket. Un grupo es un espacio de nombres compartido — ",
    Locale::En => " inside the bucket. A group is a shared namespace — ",
});
msg!(groups_prose_same_shape_as() { Locale::Es => ", con la misma forma que ", Locale::En => ", same shape as " });
msg!(groups_prose_access_via_teams() {
    Locale::Es => ", nunca ambos a la vez. El acceso a los repos del grupo pasa por sus equipos: cada equipo otorga ",
    Locale::En => ", never both at once. Access to the group's repos goes through its teams: each team grants ",
});
msg!(groups_prose_to_members_and_owner() {
    Locale::Es => " a sus miembros, sobre todos los repos del grupo o solo los que liste. Un propietario del grupo tiene acceso completo sin necesitar un equipo, y administra el grupo (equipos, miembros, otros propietarios) sin ser administrador global.",
    Locale::En => " to its members, on every repo in the group or only the ones it lists. A group owner has full access without needing a team, and manages the group (teams, members, other owners) without being a global admin.",
});

msg!(groups_permission_guest() { Locale::Es => "invitado", Locale::En => "guest" });
msg!(groups_permission_reporter() { Locale::Es => "lector", Locale::En => "reporter" });
msg!(groups_permission_developer() { Locale::Es => "desarrollador", Locale::En => "developer" });
msg!(groups_permission_maintainer() { Locale::Es => "mantenedor", Locale::En => "maintainer" });
msg!(groups_permission_owner() { Locale::Es => "propietario", Locale::En => "owner" });

msg!(groups_empty_hint() {
    Locale::Es => "(aún no administras ningún grupo)",
    Locale::En => "(you don't manage any group yet)",
});
msg!(groups_search_placeholder() { Locale::Es => "buscar grupos", Locale::En => "search groups" });
msg!(groups_search_button() { Locale::Es => "buscar", Locale::En => "search" });
msg!(groups_create_heading() { Locale::Es => "crear grupo", Locale::En => "create group" });
msg!(groups_name_placeholder() { Locale::Es => "acme", Locale::En => "acme" });
msg!(groups_description_placeholder() { Locale::Es => "descripción (opcional)", Locale::En => "description (optional)" });
msg!(groups_create_button() { Locale::Es => "crear", Locale::En => "create" });

msg!(groups_save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(groups_no_owners_hint() {
    Locale::Es => "(sin propietarios — solo los administradores globales lo gestionan)",
    Locale::En => "(no owners — only global admins manage it)",
});
msg!(groups_username_placeholder() { Locale::Es => "nombre de usuario existente", Locale::En => "existing username" });
msg!(groups_add_owner_button() { Locale::Es => "agregar propietario", Locale::En => "add owner" });
msg!(groups_new_team_heading() { Locale::Es => "nuevo equipo", Locale::En => "new team" });
msg!(groups_team_name_placeholder() { Locale::Es => "frontend", Locale::En => "frontend" });
msg!(groups_repos_scope_placeholder() {
    Locale::Es => "repos separados por comas, vacío = todos",
    Locale::En => "comma-separated repos, empty = all",
});
msg!(groups_create_team_button() { Locale::Es => "crear equipo", Locale::En => "create team" });
msg!(groups_delete_group_button() { Locale::Es => "eliminar grupo", Locale::En => "delete group" });
msg!(groups_delete_group_confirm(name: &str) {
    Locale::Es => "¿eliminar el grupo '{name}'? no se puede deshacer.",
    Locale::En => "delete group '{name}'? this cannot be undone.",
});

msg!(groups_team_label_prefix() { Locale::Es => "equipo: ", Locale::En => "team: " });
msg!(groups_scope_in() { Locale::Es => " en ", Locale::En => " in " });
msg!(groups_member_column() { Locale::Es => "miembro", Locale::En => "member" });
msg!(groups_no_members_hint() { Locale::Es => "(sin miembros todavía)", Locale::En => "(no members yet)" });
msg!(groups_add_member_button() { Locale::Es => "agregar miembro", Locale::En => "add member" });
msg!(groups_delete_team_button() { Locale::Es => "eliminar equipo", Locale::En => "delete team" });
msg!(groups_delete_team_confirm(name: &str) {
    Locale::Es => "¿eliminar el equipo '{name}'? no se puede deshacer.",
    Locale::En => "delete team '{name}'? this cannot be undone.",
});
msg!(groups_remove_button() { Locale::Es => "eliminar", Locale::En => "remove" });
msg!(groups_remove_member_confirm(name: &str) {
    Locale::Es => "¿quitar a '{name}'?",
    Locale::En => "remove '{name}'?",
});
