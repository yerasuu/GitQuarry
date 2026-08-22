//! `webui::auth::web::appearance`'s page copy.

use super::super::super::{Locale, msg};

msg!(text_size_heading() { Locale::Es => "tamaño de texto", Locale::En => "text size" });
msg!(text_size_description() {
    Locale::Es => "Por defecto para la pestaña de resumen de cualquier repo — el boton \"A\" en esa pestaña solo cambia esto para la sesion actual; esto es lo que se usa hasta que lo toques.",
    Locale::En => "Default for the summary tab of any repo — the \"A\" button on that tab only changes this for the current session; this is what's used until you've touched it.",
});
msg!(text_size_small() { Locale::Es => "pequeño", Locale::En => "small" });
msg!(text_size_medium() { Locale::Es => "mediano", Locale::En => "medium" });
msg!(text_size_large() { Locale::Es => "grande", Locale::En => "large" });

msg!(save_button() { Locale::Es => "guardar", Locale::En => "save" });

msg!(theme_heading() { Locale::Es => "tema", Locale::En => "theme" });
msg!(theme_description() {
    Locale::Es => "Por defecto para cada pagina — los botones ☀/☾ en la barra superior solo cambian esto para la sesion actual; esto es lo que se usa hasta que lo toques.",
    Locale::En => "Default for every page — the ☀/☾ buttons in the top bar only change this for the current session; this is what's used until you've touched it.",
});
msg!(theme_automatic() { Locale::Es => "automatico (sigue al sistema)", Locale::En => "automatic (follows the system)" });
msg!(theme_light() { Locale::Es => "claro", Locale::En => "light" });
msg!(theme_dark() { Locale::Es => "oscuro", Locale::En => "dark" });

msg!(density_heading() { Locale::Es => "densidad", Locale::En => "density" });
msg!(density_description() {
    Locale::Es => "Que tan compacto se ve cada pagina — es una preferencia de accesibilidad, independiente del tema de color: elegi lo que te resulte mas facil de leer. Los botones ▤/▥ en la barra superior solo cambian esto para la sesion actual; esto es lo que se usa hasta que lo toques.",
    Locale::En => "How compact every page looks — an accessibility pick, independent of color theme: choose whichever reads easiest for you. The ▤/▥ buttons in the top bar only change this for the current session; this is what's used until you've touched it.",
});
msg!(density_default_label() { Locale::Es => "por defecto (denso)", Locale::En => "default (dense)" });
msg!(density_dense_label() { Locale::Es => "denso", Locale::En => "dense" });
msg!(density_airy_label() { Locale::Es => "espaciado", Locale::En => "airy" });

msg!(locale_heading() { Locale::Es => "idioma", Locale::En => "language" });
msg!(locale_description() {
    Locale::Es => "Idioma por defecto para cada pagina — esto es lo que se usa hasta que lo toques.",
    Locale::En => "Default language for every page — this is what's used until you've touched it.",
});
msg!(locale_automatic() { Locale::Es => "automatico (usa el idioma por defecto del sitio)", Locale::En => "automatic (uses the site's built-in default)" });
// Autonyms, not translations -- a language's own name stays the same in
// both arms on purpose, same reasoning a `<html lang>` value never gets
// localized: it's identifying the language itself, not describing it.
msg!(locale_es() { Locale::Es => "Español", Locale::En => "Español" });
msg!(locale_en() { Locale::Es => "English", Locale::En => "English" });
