//! `webui::auth::web::groups`'s page copy.

use super::super::super::{Locale, msg};

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
msg!(groups_prose_access_via_members() {
    Locale::Es => ", nunca ambos a la vez. Cada miembro del grupo tiene un rol — ",
    Locale::En => ", never both at once. Every member of the group holds one role — ",
});
msg!(groups_prose_role_applies_to_every_repo() {
    Locale::Es => " — que se aplica a todos los repos del grupo sin necesidad de otorgarlos uno a uno. El rol de propietario además administra el grupo (miembros y roles) sin ser administrador global.",
    Locale::En => " — which applies to every repo in the group without granting them one by one. The owner role additionally runs the group itself (members and roles) without being a global admin.",
});

msg!(groups_permission_guest() { Locale::Es => "invitado", Locale::En => "guest" });
msg!(groups_permission_reporter() { Locale::Es => "lector", Locale::En => "reporter" });
msg!(groups_permission_developer() { Locale::Es => "desarrollador", Locale::En => "developer" });
msg!(groups_permission_maintainer() { Locale::Es => "mantenedor", Locale::En => "maintainer" });
msg!(groups_permission_owner() { Locale::Es => "propietario", Locale::En => "owner" });

msg!(groups_empty_hint() {
    Locale::Es => "(aún no perteneces a ningún grupo)",
    Locale::En => "(you don't belong to any group yet)",
});
msg!(groups_search_placeholder() { Locale::Es => "buscar grupos", Locale::En => "search groups" });
msg!(groups_search_button() { Locale::Es => "buscar", Locale::En => "search" });
msg!(groups_create_heading() { Locale::Es => "crear grupo", Locale::En => "create group" });
msg!(groups_name_placeholder() { Locale::Es => "acme", Locale::En => "acme" });
msg!(groups_description_placeholder() { Locale::Es => "descripción (opcional)", Locale::En => "description (optional)" });
msg!(groups_create_button() { Locale::Es => "crear", Locale::En => "create" });

msg!(groups_save_button() { Locale::Es => "guardar", Locale::En => "save" });
msg!(groups_username_placeholder() { Locale::Es => "nombre de usuario existente", Locale::En => "existing username" });
msg!(groups_delete_group_button() { Locale::Es => "eliminar grupo", Locale::En => "delete group" });
msg!(groups_delete_group_confirm(name: &str) {
    Locale::Es => "¿eliminar el grupo '{name}'? no se puede deshacer.",
    Locale::En => "delete group '{name}'? this cannot be undone.",
});

msg!(groups_members_heading() { Locale::Es => "miembros", Locale::En => "members" });
msg!(groups_member_column() { Locale::Es => "miembro", Locale::En => "member" });
msg!(groups_role_column() { Locale::Es => "rol", Locale::En => "role" });
msg!(groups_no_members_hint() { Locale::Es => "(sin miembros todavía)", Locale::En => "(no members yet)" });
msg!(groups_add_member_button() { Locale::Es => "agregar miembro", Locale::En => "add member" });
msg!(groups_set_role_button() { Locale::Es => "cambiar rol", Locale::En => "change role" });
msg!(groups_remove_button() { Locale::Es => "eliminar", Locale::En => "remove" });
msg!(groups_remove_member_confirm(name: &str) {
    Locale::Es => "¿quitar a '{name}'?",
    Locale::En => "remove '{name}'?",
});

msg!(
    /// Shown on a group whose stored record still carries pre-`members`
    /// owners/teams: those still grant access (see `Group::role_of`), but a
    /// team scoped to only some of the group's repos can't be shown as a
    /// group-wide role, so the page says so instead of quietly listing an
    /// incomplete member table.
    groups_legacy_pending_warning() {
    Locale::Es => "este grupo todavía tiene propietarios/equipos del modelo anterior: siguen dando acceso, pero un equipo limitado a algunos repos no aparece aquí. Ejecuta 'gitquarry migrate-group-members --apply' para convertirlos.",
    Locale::En => "this group still carries owners/teams from the old model: they still grant access, but a team scoped to only some repos isn't listed here. Run 'gitquarry migrate-group-members --apply' to convert them.",
    }
);
