use super::super::{Locale, msg};

msg!(
    search_intro() {
        Locale::Es => "Busca nombres de archivo y mensajes de commit en todos los repos que podes ver (los mismos que el indice: publicos siempre, limitados si iniciaste sesion).",
        Locale::En => "Search filenames and commit messages across every repo you can see (same as the index: public always, limited if logged in).",
    }
);

msg!(search_placeholder() { Locale::Es => "buscar", Locale::En => "search" });
msg!(search_button() { Locale::Es => "buscar", Locale::En => "search" });

msg!(filenames_heading() { Locale::Es => "archivos", Locale::En => "filenames" });
msg!(commits_heading() { Locale::Es => "commits", Locale::En => "commits" });

msg!(no_filename_matches() { Locale::Es => "(sin coincidencias)", Locale::En => "(no matches)" });
msg!(no_commit_matches() { Locale::Es => "(sin coincidencias)", Locale::En => "(no matches)" });

msg!(table_header_repo() { Locale::Es => "repo", Locale::En => "repo" });
msg!(table_header_path() { Locale::Es => "archivo", Locale::En => "file" });
msg!(table_header_commit() { Locale::Es => "commit", Locale::En => "commit" });
msg!(table_header_description() { Locale::Es => "descripcion", Locale::En => "description" });

msg!(
    /// Shown under either results table when the total across all repos
    /// exceeded `crate::webui::search::MAX_RESULTS` — never a silent cut.
    results_truncated(limit: usize) {
        Locale::Es => "mostrando las primeras {limit} coincidencias, afina la busqueda",
        Locale::En => "showing the first {limit} matches, refine your search",
    }
);
