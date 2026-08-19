//! `webui::ci`'s page copy — the deploy-approval-gate list
//! (`docs/gitquarry-ci-sdk-design.md`).

use super::super::{Locale, msg};

msg!(pending_approvals_title() { Locale::Es => "despliegues pendientes de aprobacion", Locale::En => "pending deploys" });

msg!(
    pending_approvals_intro() {
        Locale::Es => "Jobs de despliegue que requieren aprobacion de un maintainer antes de encolarse.",
        Locale::En => "Deploy jobs that need a maintainer's approval before they're queued.",
    }
);

msg!(job_column() { Locale::Es => "job", Locale::En => "job" });
msg!(sha_column() { Locale::Es => "commit", Locale::En => "commit" });
msg!(kind_column() { Locale::Es => "tipo", Locale::En => "kind" });
msg!(approve_button() { Locale::Es => "aprobar", Locale::En => "approve" });

msg!(
    no_pending_runs() {
        Locale::Es => "no hay despliegues pendientes de aprobacion.",
        Locale::En => "no deploys are waiting on approval.",
    }
);

msg!(
    ci_not_configured() {
        Locale::Es => "gitquarry-ci no esta configurado en esta instancia.",
        Locale::En => "gitquarry-ci is not configured on this instance.",
    }
);
