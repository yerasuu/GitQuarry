//! `webui::audit`'s page copy.

use super::super::{Locale, msg};

msg!(
    /// Opening half of the audit page's intro sentence, up to (not
    /// including) the literal `.conf/audit/` path — that path isn't
    /// language-specific, so it stays a plain maud literal at the call
    /// site instead of living inside this catalog string.
    audit_intro_prefix(limit: usize) {
        Locale::Es => "Ultimas {limit} entradas — creacion/eliminacion/traslados de repos, cambios de ajustes/permisos, usuarios y grupos. Guardado en ",
        Locale::En => "Last {limit} entries — repo creation/deletion/moves, settings/permission changes, users and groups. Stored in ",
    }
);

msg!(
    /// Closing half of the audit page's intro sentence, right after the
    /// literal `.conf/audit/` path.
    audit_intro_suffix() {
        Locale::Es => " dentro del bucket, una entrada por objeto.",
        Locale::En => " inside the bucket, one entry per object.",
    }
);

msg!(when_column() { Locale::Es => "cuando", Locale::En => "when" });
msg!(who_column() { Locale::Es => "quien", Locale::En => "who" });
msg!(action_column() { Locale::Es => "accion", Locale::En => "action" });
msg!(on_column() { Locale::Es => "en", Locale::En => "on" });
msg!(detail_column() { Locale::Es => "detalle", Locale::En => "detail" });

msg!(
    /// Table body placeholder when `storage::audit::list_recent`
    /// comes back empty.
    no_entries_hint() {
        Locale::Es => "(vacio)",
        Locale::En => "(empty)",
    }
);
