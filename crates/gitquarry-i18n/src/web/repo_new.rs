use super::super::{Locale, msg};

msg!(repo_new_heading() { Locale::Es => "nuevo repo", Locale::En => "new repo" });

// The prose paragraph is split into fragments around fixed `code`/`a`
// elements (`refs.json`, `namespace/name`, `git push`, `owner`, the
// `/_groups` link) that stay untranslated identifiers/paths — each fragment
// keeps the same position relative to those elements in both locales so the
// rendered sentence still reads naturally once concatenated.
msg!(repo_new_prose_creates_empty() { Locale::Es => "Crea un ", Locale::En => "Creates an empty " });
msg!(repo_new_prose_in_bucket_under() {
    Locale::Es => " vacío en el bucket, bajo ",
    Locale::En => " in the bucket, under ",
});
msg!(repo_new_prose_your_own_folder() {
    Locale::Es => " — tu propia carpeta (ver ",
    Locale::En => " — your own folder (see ",
});
msg!(repo_new_your_profile_link() { Locale::Es => "tu perfil", Locale::En => "your profile" });
msg!(repo_new_prose_or_existing_group() {
    Locale::Es => ") o un grupo existente (",
    Locale::En => ") or an existing group (",
});
msg!(repo_new_prose_close_paren() { Locale::Es => "). ", Locale::En => "). " });
msg!(repo_new_prose_push_fills_refs() {
    Locale::Es => " completa después las referencias, igual que si el push hubiera creado el repo por su cuenta; ",
    Locale::En => " afterward fills in the refs, same as if the push had created the repo on its own; ",
});
msg!(repo_new_prose_avoids_blind_push() {
    Locale::Es => "esto solo evita ese primer push \"a ciegas\" cuando quieres description/visibility definidos desde el inicio. ",
    Locale::En => "this just avoids that first \"blind\" push when you want description/visibility set from the start. ",
});
msg!(repo_new_prose_owner_note() {
    Locale::Es => " queda fijado al namespace elegido, no es un campo aparte.",
    Locale::En => " ends up set to the chosen namespace, it's not a separate field.",
});

msg!(repo_new_namespace_label() { Locale::Es => "espacio de nombres", Locale::En => "namespace" });
msg!(repo_new_me_suffix() { Locale::Es => " (yo)", Locale::En => " (me)" });
msg!(repo_new_name_label() { Locale::Es => "nombre", Locale::En => "name" });
msg!(repo_new_name_placeholder() { Locale::Es => "proyecto", Locale::En => "project" });
msg!(repo_new_name_hint() {
    Locale::Es => "cualquier texto vale; la url se genera a partir de este nombre.",
    Locale::En => "any text works; the url is generated from this name."
});
msg!(repo_new_description_label() { Locale::Es => "descripción", Locale::En => "description" });
msg!(repo_new_visibility_label() { Locale::Es => "visibilidad", Locale::En => "visibility" });
msg!(repo_new_visibility_public() { Locale::Es => "público", Locale::En => "public" });
msg!(repo_new_visibility_limited() {
    Locale::Es => "limitado (solo usuarios con sesión iniciada)",
    Locale::En => "limited (logged-in users only)",
});
msg!(repo_new_visibility_private() { Locale::Es => "privado", Locale::En => "private" });
msg!(repo_new_create_button() { Locale::Es => "crear", Locale::En => "create" });
msg!(repo_new_template_label() { Locale::Es => "plantilla", Locale::En => "template" });
msg!(repo_new_template_placeholder() {
    Locale::Es => "namespace/repo (opcional)",
    Locale::En => "namespace/repo (optional)",
});

msg!(repo_new_no_session_error() {
    Locale::Es => "no hay usuario con sesión iniciada — configura OIDC o una contraseña en /_users primero.",
    Locale::En => "no logged-in user — configure OIDC or a password in /_users first.",
});
