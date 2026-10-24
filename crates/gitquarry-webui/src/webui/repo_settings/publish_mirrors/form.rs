//! The add/edit form of a publish mirror: what it shows and how a submission
//! becomes a [`PublishMirror`].

use crate::webui::repo_link;
use crate::webui::repo_settings::RepoConfForm;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::publish_mirrors as t;
use gitquarry_mirror_external::publish::{
    FilterConfig, FilterMode, IdentityMapping, MirrorToken, PublishMirror, PublishMode, Publisher, SecretAllow,
    UnmappedPolicy,
};
use maud::{Markup, PreEscaped, html};

/// Applies the form's fields over `mirror` (a stored one on edit, so its id
/// survives). `Err` carries a message for the flash; everything that reaches
/// storage is validated again by `publish::create`/`update`.
pub(super) fn mirror_from_form(
    mut mirror: PublishMirror,
    form: &RepoConfForm,
    locale: Locale,
) -> Result<PublishMirror, String> {
    mirror.label = form.label.trim().to_string();
    mirror.remote_url = form.remote_url.trim().to_string();
    // Blank keeps the stored one: the form never shows a token back.
    if !form.token.trim().is_empty() {
        mirror.token = MirrorToken::new(form.token.trim().to_string());
    }
    mirror.enabled = form.mirror_enabled == "1";
    mirror.mode = match form.publish_run_mode.as_str() {
        "snapshot" | "" => PublishMode::Snapshot,
        "history" => PublishMode::History,
        other => return Err(t::bad_run_mode(locale, other)),
    };
    // Only ever by one, and only from the stored epoch: `publish::update`
    // refuses anything else, and refuses an output change without it.
    if form.publish_confirm_epoch_bump == "1" {
        mirror.filter_epoch = mirror.filter_epoch.saturating_add(1);
    }
    let (name, email) = (form.publish_publisher_name.trim(), form.publish_publisher_email.trim());
    mirror.publisher = match (name.is_empty(), email.is_empty()) {
        (true, true) => None,
        (false, false) => Some(Publisher {
            name: name.to_string(),
            email: email.to_string(),
        }),
        _ => return Err(t::publisher_incomplete(locale)),
    };
    mirror.filter = FilterConfig {
        mode: match form.publish_mode.as_str() {
            "exclude" => FilterMode::Exclude,
            _ => FilterMode::Allowlist,
        },
        include: lines(&form.publish_include).map(str::to_string).collect(),
        exclude: lines(&form.publish_exclude).map(str::to_string).collect(),
        exclude_submodules: form.publish_exclude_submodules == "1",
        strip_empty_dirs: form.publish_strip_empty_dirs == "1",
    };
    mirror.publish_branches = gitquarry_core::domain::repo::parse_csv_field(&form.publish_branches);
    mirror.publish_tags = gitquarry_core::domain::repo::parse_csv_field(&form.publish_tags);
    mirror.identities = lines(&form.publish_identities)
        .map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(private), Some(public)) => {
                    let name = parts.collect::<Vec<_>>().join(" ");
                    Ok(IdentityMapping {
                        private: private.to_string(),
                        public: public.to_string(),
                        name: (!name.is_empty()).then_some(name),
                    })
                }
                _ => Err(t::bad_identity_line(locale, line)),
            }
        })
        .collect::<Result<_, _>>()?;
    mirror.unmapped = match form.publish_unmapped.as_str() {
        "passthrough" => UnmappedPolicy::Passthrough,
        _ => UnmappedPolicy::Block,
    };
    let spdx = form.publish_spdx.trim();
    mirror.spdx = (!spdx.is_empty()).then(|| spdx.to_string());
    mirror.secret_allowlist = lines(&form.publish_secret_allowlist)
        .map(|line| match line.split_once(char::is_whitespace) {
            Some((sha, path)) => SecretAllow {
                path: path.trim().to_string(),
                sha: sha.to_string(),
            },
            None => SecretAllow {
                path: String::new(),
                sha: line.to_string(),
            },
        })
        .collect();
    Ok(mirror)
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

/// What the form's inputs show, from a stored mirror or from what the user
/// typed. Never the token, so nothing secret can be echoed back.
struct FormValues {
    label: String,
    remote_url: String,
    enabled: bool,
    history_mode: bool,
    publisher_name: String,
    publisher_email: String,
    exclude_mode: bool,
    include: String,
    exclude: String,
    exclude_submodules: bool,
    strip_empty_dirs: bool,
    branches: String,
    tags: String,
    identities: String,
    passthrough: bool,
    spdx: String,
    allowlist: String,
}

impl FormValues {
    fn of_mirror(mirror: &PublishMirror) -> Self {
        let csv = |set: &std::collections::BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(", ");
        Self {
            label: mirror.label.clone(),
            remote_url: mirror.remote_url.clone(),
            enabled: mirror.enabled,
            history_mode: mirror.mode == PublishMode::History,
            publisher_name: mirror.publisher.as_ref().map_or_else(String::new, |p| p.name.clone()),
            publisher_email: mirror.publisher.as_ref().map_or_else(String::new, |p| p.email.clone()),
            exclude_mode: mirror.filter.mode == FilterMode::Exclude,
            include: mirror.filter.include.join("\n"),
            exclude: mirror.filter.exclude.join("\n"),
            exclude_submodules: mirror.filter.exclude_submodules,
            strip_empty_dirs: mirror.filter.strip_empty_dirs,
            branches: csv(&mirror.publish_branches),
            tags: csv(&mirror.publish_tags),
            identities: mirror
                .identities
                .iter()
                .map(|i| match &i.name {
                    Some(name) => format!("{} {} {name}", i.private, i.public),
                    None => format!("{} {}", i.private, i.public),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            passthrough: mirror.unmapped == UnmappedPolicy::Passthrough,
            spdx: mirror.spdx.clone().unwrap_or_default(),
            allowlist: mirror
                .secret_allowlist
                .iter()
                .map(|a| format!("{} {}", a.sha, a.path).trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// The raw fields as submitted, so a value the validation refused is
    /// still there to correct.
    fn of_form(form: &RepoConfForm) -> Self {
        Self {
            label: form.label.clone(),
            remote_url: form.remote_url.clone(),
            enabled: form.mirror_enabled == "1",
            history_mode: form.publish_run_mode == "history",
            publisher_name: form.publish_publisher_name.clone(),
            publisher_email: form.publish_publisher_email.clone(),
            exclude_mode: form.publish_mode == "exclude",
            include: form.publish_include.clone(),
            exclude: form.publish_exclude.clone(),
            exclude_submodules: form.publish_exclude_submodules == "1",
            strip_empty_dirs: form.publish_strip_empty_dirs == "1",
            branches: form.publish_branches.clone(),
            tags: form.publish_tags.clone(),
            identities: form.publish_identities.clone(),
            passthrough: form.publish_unmapped == "passthrough",
            spdx: form.publish_spdx.clone(),
            allowlist: form.publish_secret_allowlist.clone(),
        }
    }
}

/// `retry` is the rejected submission of this very form, if there was one.
/// It supplies the typed values and, on an edit, the etag the browser
/// loaded, so a conflict stays a conflict.
pub(super) fn render_form(
    repo_name: &str,
    existing: Option<(&PublishMirror, &str)>,
    retry: Option<&RepoConfForm>,
    csrf: &str,
    locale: Locale,
) -> Markup {
    let action_url = repo_link(repo_name, "settings", "").to_string();
    let default = PublishMirror::default();
    let (mirror, stored_etag) = existing.unwrap_or((&default, ""));
    let etag = retry.map_or(stored_etag, |f| f.publish_mirror_etag.as_str());
    let v = retry.map_or_else(|| FormValues::of_mirror(mirror), FormValues::of_form);
    html! {
        form method="post" action=(&action_url) {
            (PreEscaped(csrf))
            @if existing.is_some() {
                input type="hidden" name="conf_action" value="edit_publish_mirror";
                input type="hidden" name="publish_mirror_id" value=(mirror.id);
                input type="hidden" name="publish_mirror_etag" value=(etag);
            } @else {
                input type="hidden" name="conf_action" value="add_publish_mirror";
            }
            label { (t::field_label(locale)) " " input name="label" value=(v.label); }
            label {
                (t::field_remote_url(locale)) " "
                input type="url" name="remote_url" value=(v.remote_url) required size="40";
            }
            label {
                (t::field_token(locale)) " "
                input type="password" name="token" size="24" autocomplete="off"
                    placeholder=(if existing.is_some() && !mirror.token.is_empty() { t::token_placeholder_keep(locale) } else { t::token_placeholder_new(locale) });
            }
            label {
                input type="checkbox" name="mirror_enabled" value="1" checked[v.enabled];
                " " (t::field_enabled(locale))
            }
            label {
                (t::field_run_mode(locale)) " "
                select name="publish_run_mode" {
                    option value="snapshot" selected[!v.history_mode] { (t::run_mode_snapshot(locale)) }
                    option value="history" selected[v.history_mode] { (t::run_mode_history(locale)) }
                }
            }
            label { (t::field_publisher_name(locale)) " " input name="publish_publisher_name" value=(v.publisher_name) size="24"; }
            label { (t::field_publisher_email(locale)) " " input type="email" name="publish_publisher_email" value=(v.publisher_email) size="30"; }
            label {
                (t::field_mode(locale)) " "
                select name="publish_mode" {
                    option value="allowlist" selected[!v.exclude_mode] { (t::mode_allowlist(locale)) }
                    option value="exclude" selected[v.exclude_mode] { (t::mode_exclude(locale)) }
                }
            }
            label { (t::field_include(locale)) br; textarea name="publish_include" rows="4" cols="40" { (v.include) } }
            label { (t::field_exclude(locale)) br; textarea name="publish_exclude" rows="4" cols="40" { (v.exclude) } }
            label {
                input type="checkbox" name="publish_exclude_submodules" value="1" checked[v.exclude_submodules];
                " " (t::field_exclude_submodules(locale))
            }
            label {
                input type="checkbox" name="publish_strip_empty_dirs" value="1" checked[v.strip_empty_dirs];
                " " (t::field_strip_empty_dirs(locale))
            }
            label { (t::field_branches(locale)) " " input name="publish_branches" value=(v.branches) size="30"; }
            label { (t::field_tags(locale)) " " input name="publish_tags" value=(v.tags) size="30"; }
            label { (t::field_identities(locale)) br; textarea name="publish_identities" rows="3" cols="60" { (v.identities) } }
            label {
                (t::field_unmapped(locale)) " "
                select name="publish_unmapped" {
                    option value="block" selected[!v.passthrough] { (t::unmapped_block(locale)) }
                    option value="passthrough" selected[v.passthrough] { (t::unmapped_passthrough(locale)) }
                }
            }
            label { (t::field_spdx(locale)) " " input name="publish_spdx" value=(v.spdx) size="20"; }
            label { (t::field_secret_allowlist(locale)) br; textarea name="publish_secret_allowlist" rows="3" cols="60" { (v.allowlist) } }
            @if existing.is_some() {
                fieldset {
                    legend { (t::field_epoch(locale, mirror.filter_epoch)) }
                    label {
                        // Never pre-checked, not even on a retry: each bump
                        // is its own explicit decision.
                        input type="checkbox" name="publish_confirm_epoch_bump" value="1";
                        " " strong { (t::confirm_epoch_bump(locale)) }
                    }
                }
            }
            button type="submit" { (if existing.is_some() { t::save_button(locale) } else { t::add_button(locale) }) }
        }
    }
}
