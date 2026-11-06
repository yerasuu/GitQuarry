//! `webui::repo::milestones`'s page copy.

use super::super::super::{Locale, msg};

msg!(milestones_heading() { Locale::Es => "milestones", Locale::En => "milestones" });
msg!(no_open_milestones_hint() { Locale::Es => "(sin milestones abiertos)", Locale::En => "(no open milestones)" });
msg!(no_closed_milestones_hint() { Locale::Es => "(sin milestones cerrados)", Locale::En => "(no closed milestones)" });
msg!(open_milestones_heading() { Locale::Es => "abiertos", Locale::En => "open" });
msg!(closed_milestones_heading() { Locale::Es => "cerrados", Locale::En => "closed" });
msg!(due_date_prefix() { Locale::Es => "vence: ", Locale::En => "due: " });
msg!(progress_summary(open: u32, closed: u32) {
    Locale::Es => "{open} issue(s) abierto(s) · {closed} cerrado(s)",
    Locale::En => "{open} open issue(s) · {closed} closed",
});
msg!(close_button() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(reopen_button() { Locale::Es => "reabrir", Locale::En => "reopen" });
msg!(edit_button() { Locale::Es => "editar", Locale::En => "edit" });
msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });

msg!(new_milestone_heading() { Locale::Es => "nuevo milestone", Locale::En => "new milestone" });
msg!(title_label() { Locale::Es => "titulo", Locale::En => "title" });
msg!(description_label() { Locale::Es => "descripcion", Locale::En => "description" });
msg!(due_date_label() { Locale::Es => "fecha limite (opcional)", Locale::En => "due date (optional)" });
msg!(create_milestone_button() { Locale::Es => "crear milestone", Locale::En => "create milestone" });

msg!(no_milestone_option() { Locale::Es => "(ninguno)", Locale::En => "(none)" });

msg!(bulk_select_heading() { Locale::Es => "acciones masivas", Locale::En => "bulk actions" });
msg!(bulk_close_option() { Locale::Es => "cerrar", Locale::En => "close" });
msg!(bulk_reopen_option() { Locale::Es => "reabrir", Locale::En => "reopen" });
msg!(bulk_apply_button() { Locale::Es => "aplicar a los seleccionados", Locale::En => "apply to selected" });
