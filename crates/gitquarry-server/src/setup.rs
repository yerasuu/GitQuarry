//! First-run web wizard: entered when `gitquarry.toml` doesn't exist yet
//! anywhere in the usual resolution order, or when it does but the bucket it
//! points at has never had an admin user created (a fresh bucket behind an
//! already-written config — infra-as-code, a config copied ahead of the
//! first real deploy, ...) — see `crate::serve::run` for both checks. One
//! form, served on the same `--web-bind` the real web UI will use once
//! configured: a storage driver (s3/r2/azure/gcs/local) + its credentials +
//! encryption passphrase, plus an initial admin user for the web UI. On the
//! second path every field above the admin user is pre-filled from the
//! config already on disk (still editable — this is the one screen, not a
//! stripped-down variant), so the operator only has to actually type the
//! admin username/password. On submit everything is verified for real
//! against the storage backend (`gitquarry_core::crypto::bootstrap::get_or_create_cipher`,
//! which also bootstraps a brand-new bucket's encryption) before anything is
//! written — a bad credential or passphrase re-shows the form with an error
//! instead of leaving a broken config on disk. Once it succeeds, the admin
//! user is seeded into the bucket, `gitquarry.toml` is written if it didn't
//! already exist, and this temporary server hands off to normal startup in
//! the same process: no restart, no separate `gitquarry init` step.

use anyhow::{Context, Result, bail, ensure};
use axum::Router;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use gitquarry_auth_crypto::password as pw;
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::User;
use gitquarry_core::config::{
    self, AzureParams, BackendConfig, BucketConfig, GcsParams, LocalEngine, LocalParams, R2Params, S3Params,
};
use gitquarry_core::crypto::Cipher;
use gitquarry_core::storage::{self, Backend};
use gitquarry_webui::webui::{esc, layout, non_empty};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tracing::{info, warn};

/// What `run` hands back to [`crate::serve::run`] once the wizard succeeds —
/// exactly what it would otherwise have loaded from an existing
/// `gitquarry.toml`, so the caller can fall straight into the same startup
/// path regardless of which branch produced these.
pub struct WizardOutcome {
    pub bucket: String,
    pub bucket_cfg: BucketConfig,
    pub backend: Arc<dyn Backend>,
    pub cipher: Cipher,
}

/// What already exists and should pre-fill the form instead of starting
/// blank — a `gitquarry.toml` whose bucket connects fine but has no admin
/// user yet (see `crate::serve::run`'s "fresh bucket, existing config"
/// check). `name` is only ever pre-filled, never locked: nothing stops
/// resubmitting a different one, same freedom the blank-form path already has.
pub struct Prefill {
    pub name: String,
    pub bucket_cfg: BucketConfig,
}

struct WizardState {
    config_path: PathBuf,
    prefill: Option<Prefill>,
    // `Option` + taken on the one successful submit: a `oneshot::Sender`
    // only sends once, and this wizard only ever completes once (the
    // server shuts down right after).
    result_tx: Mutex<Option<oneshot::Sender<WizardOutcome>>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

type State_ = axum::extract::State<Arc<WizardState>>;

pub async fn run(config_path: &Path, web_bind: &str, prefill: Option<Prefill>) -> Result<WizardOutcome> {
    let is_prefilled = prefill.is_some();
    let (result_tx, result_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let state = Arc::new(WizardState {
        config_path: config_path.to_path_buf(),
        prefill,
        result_tx: Mutex::new(Some(result_tx)),
        shutdown_tx: Mutex::new(Some(shutdown_tx)),
    });

    let app = Router::new()
        .route("/", get(handler_get).post(handler_post))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(web_bind)
        .await
        .with_context(|| format!("couldn't listen on {web_bind} for the setup wizard"))?;
    info!(
        "{} — open http://{web_bind} to {}",
        if is_prefilled {
            format!("{} connects but has no admin user yet", config_path.display())
        } else {
            format!("couldn't find {}", config_path.display())
        },
        if is_prefilled {
            "create one"
        } else {
            "configure the bucket and the admin user"
        }
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await
        .context("setup wizard failed")?;

    result_rx
        .await
        .context("setup wizard closed without finishing configuration")
}

async fn handler_get(State(state): State_) -> Html<String> {
    Html(render_form(None, state.prefill.as_ref()))
}

/// Every field is driver-prefixed and read only by that one driver's arm of
/// [`build_backend_config`] — no field is shared across two driver sections
/// (there used to be one `bucket`/`access_key_id`/`secret_access_key` each,
/// reused by whichever driver was picked; confusing to fill out since
/// nothing on the page said which of the five sections a given value
/// actually belonged to once submitted).
// `pub`/field visibility only widened under `test`/`test-util` (see
// gitquarry-core's `mirror::hook::RefUpdate` for the same dual-definition
// pattern): `tests/setup.rs` constructs this struct via literal, touching
// every field, but it must stay crate-private in a normal build.
#[cfg(any(test, feature = "test-util"))]
#[derive(Deserialize)]
pub struct WizardForm {
    /// `[buckets.<name>]` section key / `--bucket` value — a separate,
    /// purely local concept from any driver's own physical storage-target
    /// identifier (see `S3Params::bucket`'s doc comment).
    pub name: String,
    pub driver: String,
    #[serde(default)]
    pub s3_bucket: String,
    #[serde(default)]
    pub s3_region: String,
    #[serde(default)]
    pub s3_endpoint_url: String,
    #[serde(default)]
    pub s3_access_key_id: String,
    #[serde(default)]
    pub s3_secret_access_key: String,
    #[serde(default)]
    pub s3_path_style: String,
    #[serde(default)]
    pub r2_bucket: String,
    #[serde(default)]
    pub r2_account_id: String,
    #[serde(default)]
    pub r2_access_key_id: String,
    #[serde(default)]
    pub r2_secret_access_key: String,
    #[serde(default)]
    pub azure_bucket: String,
    #[serde(default)]
    pub azure_account_name: String,
    #[serde(default)]
    pub azure_tenant_id: String,
    #[serde(default)]
    pub azure_client_id: String,
    #[serde(default)]
    pub azure_client_secret: String,
    #[serde(default)]
    pub gcs_bucket: String,
    #[serde(default)]
    pub gcs_service_account_json_path: String,
    #[serde(default)]
    pub local_bucket: String,
    pub passphrase: String,
    pub admin_username: String,
    /// Optional, but the only chance to set one at creation: `User::email`
    /// can be changed afterward only by an admin through `/_users`
    /// (`auth::domain::user::set_email`), and this account *is* the first
    /// admin. Left blank, every merge commit it stamps carries the
    /// synthetic `<username>@users.noreply.gitquarry` address instead
    /// (`domain::merge_request::merger_identity`).
    #[serde(default)]
    pub admin_email: String,
    pub admin_password: String,
    pub admin_password_confirm: String,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Deserialize)]
struct WizardForm {
    name: String,
    driver: String,
    #[serde(default)]
    s3_bucket: String,
    #[serde(default)]
    s3_region: String,
    #[serde(default)]
    s3_endpoint_url: String,
    #[serde(default)]
    s3_access_key_id: String,
    #[serde(default)]
    s3_secret_access_key: String,
    #[serde(default)]
    s3_path_style: String,
    #[serde(default)]
    r2_bucket: String,
    #[serde(default)]
    r2_account_id: String,
    #[serde(default)]
    r2_access_key_id: String,
    #[serde(default)]
    r2_secret_access_key: String,
    #[serde(default)]
    azure_bucket: String,
    #[serde(default)]
    azure_account_name: String,
    #[serde(default)]
    azure_tenant_id: String,
    #[serde(default)]
    azure_client_id: String,
    #[serde(default)]
    azure_client_secret: String,
    #[serde(default)]
    gcs_bucket: String,
    #[serde(default)]
    gcs_service_account_json_path: String,
    #[serde(default)]
    local_bucket: String,
    passphrase: String,
    admin_username: String,
    /// Optional, but the only chance to set one: `User::email` can be
    /// changed afterward only by an admin through `/_users`
    /// (`auth::domain::user::set_email`), and this account *is* the first
    /// admin. Left blank, every merge commit it stamps carries the
    /// synthetic `<username>@users.noreply.gitquarry` address instead
    /// (`domain::merge_request::merger_identity`).
    #[serde(default)]
    admin_email: String,
    admin_password: String,
    admin_password_confirm: String,
}

async fn handler_post(State(state): State_, Form(form): Form<WizardForm>) -> Response {
    match try_setup(&state, &form).await {
        Ok(outcome) => {
            // Hand off the result and trigger shutdown only once startup can
            // actually proceed — everything that can fail already happened
            // inside `try_setup` above, so there's nothing left here that
            // could strand the browser on a "done" page while the process
            // exits without ever using what it was just given.
            let result_tx = state.result_tx.lock().expect("wizard mutex never poisoned").take();
            let shutdown_tx = state.shutdown_tx.lock().expect("wizard mutex never poisoned").take();
            if let (Some(result_tx), Some(shutdown_tx)) = (result_tx, shutdown_tx) {
                let _ = result_tx.send(outcome);
                let _ = shutdown_tx.send(());
            }
            Html(layout(
                "done",
                "",
                "<p>Configuration saved. Starting the server...</p>",
                "",
                "",
                "",
                gitquarry_i18n::Locale::DEFAULT,
            ))
            .into_response()
        }
        Err(e) => {
            warn!("setup wizard: {e:#}");
            (
                StatusCode::BAD_REQUEST,
                Html(render_form(Some(&format!("{e:#}")), state.prefill.as_ref())),
            )
                .into_response()
        }
    }
}

async fn try_setup(state: &WizardState, form: &WizardForm) -> Result<WizardOutcome> {
    let name = form.name.trim().to_string();
    ensure!(!name.is_empty(), "entry name can't be empty");

    let admin_username = form.admin_username.trim().to_string();
    access::validate_username(&admin_username)?;
    let admin_email = match form.admin_email.trim() {
        "" => None,
        raw => Some(gitquarry_core::validate::validate_secondary_email(raw)?),
    };
    ensure!(!form.admin_password.is_empty(), "admin password can't be empty");
    ensure!(
        form.admin_password == form.admin_password_confirm,
        "admin password and confirmation don't match"
    );
    // Same rule `config::Config::load_from` enforces for an existing file —
    // checked here too so a typo shows up as a form error instead of a
    // config file that's already unusable the moment it's written.
    ensure!(!form.passphrase.trim().is_empty(), "passphrase can't be empty");

    let backend_cfg = build_backend_config(form, state.prefill.as_ref().map(|p| &p.bucket_cfg.backend))?;
    let bucket_cfg = BucketConfig {
        backend: backend_cfg,
        passphrase: form.passphrase.clone(),
        resilience: Default::default(),
    };

    let backend = storage::build_backend(&bucket_cfg)?;
    // The real round trip: wrong creds/endpoint/bucket, or a passphrase
    // that doesn't match a bucket already used before, all fail clearly
    // right here instead of writing a config that can't actually connect.
    let cipher =
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?;

    seed_admin_user(
        backend.as_ref(),
        &cipher,
        &admin_username,
        admin_email.as_deref(),
        &form.admin_password,
    )
    .await?;
    // Only for the blank-form path: a pre-filled run's `gitquarry.toml`
    // already exists (that's what got it here — see `crate::serve::run`),
    // and `write_new` refuses to touch a path that's already there anyway.
    if state.prefill.is_none() {
        config::Config::write_new(&state.config_path, &name, &bucket_cfg)?;
    }

    Ok(WizardOutcome {
        bucket: name,
        bucket_cfg,
        backend,
        cipher,
    })
}

/// Builds the right [`BackendConfig`] variant for `form.driver`, validating
/// only the fields that driver actually needs. The form always submits
/// every driver's fields together — no JavaScript in this app to show/hide
/// sections per the selected driver (see `gitquarry_webui::webui`'s module doc) — so an
/// empty field belonging to a driver that isn't selected is expected, not
/// an error.
///
/// `existing` is the prefilled run's already-on-disk backend config, if
/// any (see [`Prefill`]) — only consulted by the `"local"` arm, to decide
/// [`LocalParams::engine`] (see there for why).
#[cfg(any(test, feature = "test-util"))]
pub fn build_backend_config(form: &WizardForm, existing: Option<&BackendConfig>) -> Result<BackendConfig> {
    build_backend_config_impl(form, existing)
}
#[cfg(not(any(test, feature = "test-util")))]
fn build_backend_config(form: &WizardForm, existing: Option<&BackendConfig>) -> Result<BackendConfig> {
    build_backend_config_impl(form, existing)
}

fn build_backend_config_impl(form: &WizardForm, existing: Option<&BackendConfig>) -> Result<BackendConfig> {
    match form.driver.as_str() {
        "s3" => {
            let bucket = form.s3_bucket.trim().to_string();
            ensure!(!bucket.is_empty(), "s3: bucket name can't be empty");
            let access_key_id = form.s3_access_key_id.trim().to_string();
            let secret_access_key = form.s3_secret_access_key.trim().to_string();
            ensure!(!access_key_id.is_empty(), "s3: access key id is empty");
            ensure!(!secret_access_key.is_empty(), "s3: secret access key is empty");
            Ok(BackendConfig::S3(S3Params {
                bucket,
                region: non_empty(form.s3_region.clone()),
                endpoint_url: non_empty(form.s3_endpoint_url.clone()),
                access_key_id,
                secret_access_key: secret_access_key.into(),
                path_style: form.s3_path_style == "1",
                // Off, deliberately: an `http://` endpoint would put these
                // credentials and every object body on the wire in the
                // clear. An operator pointing at a local MinIO opts in by
                // editing the config by hand.
                allow_insecure_http: false,
            }))
        }
        "r2" => {
            let bucket = form.r2_bucket.trim().to_string();
            ensure!(!bucket.is_empty(), "r2: bucket name can't be empty");
            let account_id = form.r2_account_id.trim().to_string();
            ensure!(!account_id.is_empty(), "r2: account id is empty");
            let access_key_id = form.r2_access_key_id.trim().to_string();
            let secret_access_key = form.r2_secret_access_key.trim().to_string();
            ensure!(!access_key_id.is_empty(), "r2: access key id is empty");
            ensure!(!secret_access_key.is_empty(), "r2: secret access key is empty");
            Ok(BackendConfig::R2(R2Params {
                bucket,
                account_id,
                access_key_id,
                secret_access_key: secret_access_key.into(),
            }))
        }
        "azure" => {
            let bucket = form.azure_bucket.trim().to_string();
            ensure!(!bucket.is_empty(), "azure: container name can't be empty");
            let account_name = form.azure_account_name.trim().to_string();
            let tenant_id = form.azure_tenant_id.trim().to_string();
            let client_id = form.azure_client_id.trim().to_string();
            let client_secret = form.azure_client_secret.trim().to_string();
            ensure!(!account_name.is_empty(), "azure: account name is empty");
            ensure!(!tenant_id.is_empty(), "azure: tenant id is empty");
            ensure!(!client_id.is_empty(), "azure: client id is empty");
            ensure!(!client_secret.is_empty(), "azure: client secret is empty");
            Ok(BackendConfig::Azure(AzureParams {
                bucket,
                account_name,
                tenant_id,
                client_id,
                client_secret: client_secret.into(),
            }))
        }
        "gcs" => {
            let bucket = form.gcs_bucket.trim().to_string();
            ensure!(!bucket.is_empty(), "gcs: bucket name can't be empty");
            let service_account_json_path = form.gcs_service_account_json_path.trim().to_string();
            ensure!(
                !service_account_json_path.is_empty(),
                "gcs: service account JSON path is empty"
            );
            Ok(BackendConfig::Gcs(GcsParams {
                bucket,
                service_account_json_path,
            }))
        }
        "local" => {
            let bucket = form.local_bucket.trim().to_string();
            ensure!(!bucket.is_empty(), "local: directory can't be empty");
            // A resubmission of an already-existing local bucket (the
            // prefill path) must keep whatever engine it's actually
            // formatted as on disk -- flipping it here would mean writing
            // a config that claims a format the directory was never
            // migrated to (see LocalParams::engine's own doc: "not
            // additive/mergeable... needs an explicit migration"). Only a
            // genuinely brand-new bucket (no prefill, nothing on disk yet)
            // gets to start on the better default.
            let engine = match existing {
                Some(BackendConfig::Local(existing)) => existing.engine,
                _ => LocalEngine::Redb,
            };
            Ok(BackendConfig::Local(LocalParams { bucket, engine }))
        }
        other => bail!("unknown driver: {other}"),
    }
}

/// Only creates the admin if the bucket has never been used with
/// gitquarry's web UI before (has no users yet) — pointing the wizard at a
/// bucket that's already set up shouldn't silently add a second admin or
/// touch existing users — manage it from `/_users` instead.
async fn seed_admin_user(
    backend: &dyn Backend,
    cipher: &Cipher,
    username: &str,
    email: Option<&str>,
    password: &str,
) -> Result<()> {
    if !access::list_usernames(backend).await?.is_empty() {
        return Ok(());
    }
    // A lost race here means another concurrent wizard run against the same
    // fresh bucket won already — same "already set up, leave it" reasoning
    // as the checks above, not an error worth failing this wizard over.
    if !access::reserve_namespace(backend, cipher, username, "user").await? {
        return Ok(());
    }
    let password_hash = pw::hash_password(password)?;
    let user = User {
        uid: gitquarry_core::storage::uid::Uid::new(),
        username: username.to_string(),
        email: email.map(str::to_string),
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: std::collections::BTreeMap::new(),
        default_permission: None,
        password_hash: Some(password_hash),
        session_epoch: 0,
        is_admin: true,
        is_federation_moderator: false,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    };
    if access::put_user(backend, cipher, &user, None).await? == gitquarry_core::storage::Cas::Ok {
        let _ = user.claim_uid(backend, cipher).await;
        // Without this the address is stored on the record but resolves to
        // nobody — OIDC login looks it up through the index, not the field,
        // and another account could later claim the same one.
        let _ = user.claim_email(backend, cipher).await;
        // Otherwise `password_login_available` would find no counter object
        // and wrongly conclude no one can log in with a password, right
        // after creating an admin who can.
        access::bump_password_login_count(backend, cipher, 1).await;
    }
    Ok(())
}

/// Every field the form can pre-fill, minus the admin user (never
/// pre-filled: seeding one is the entire reason this run of the wizard
/// exists). [`Self::blank`] for the no-config-at-all path; [`Self::from`] for
/// the existing-but-fresh-bucket one.
// `pub`/field visibility only widened under `test`/`test-util` — see
// `WizardForm`'s dual-definition note above; `tests/setup.rs` reads a
// subset of these fields off the value `FieldValues::from` returns.
#[cfg(any(test, feature = "test-util"))]
pub struct FieldValues {
    pub name: String,
    pub driver: &'static str,
    pub s3_bucket: String,
    pub s3_region: String,
    s3_endpoint_url: String,
    pub s3_access_key_id: String,
    pub s3_secret_access_key: String,
    pub s3_path_style: bool,
    r2_bucket: String,
    pub r2_account_id: String,
    r2_access_key_id: String,
    r2_secret_access_key: String,
    azure_bucket: String,
    azure_account_name: String,
    pub azure_tenant_id: String,
    azure_client_id: String,
    azure_client_secret: String,
    gcs_bucket: String,
    gcs_service_account_json_path: String,
    local_bucket: String,
    pub passphrase: String,
}
#[cfg(not(any(test, feature = "test-util")))]
struct FieldValues {
    name: String,
    driver: &'static str,
    s3_bucket: String,
    s3_region: String,
    s3_endpoint_url: String,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_path_style: bool,
    r2_bucket: String,
    r2_account_id: String,
    r2_access_key_id: String,
    r2_secret_access_key: String,
    azure_bucket: String,
    azure_account_name: String,
    azure_tenant_id: String,
    azure_client_id: String,
    azure_client_secret: String,
    gcs_bucket: String,
    gcs_service_account_json_path: String,
    local_bucket: String,
    passphrase: String,
}

impl FieldValues {
    fn blank() -> Self {
        FieldValues {
            name: String::new(),
            driver: "s3",
            s3_bucket: String::new(),
            s3_region: String::new(),
            s3_endpoint_url: String::new(),
            s3_access_key_id: String::new(),
            s3_secret_access_key: String::new(),
            s3_path_style: false,
            r2_bucket: String::new(),
            r2_account_id: String::new(),
            r2_access_key_id: String::new(),
            r2_secret_access_key: String::new(),
            azure_bucket: String::new(),
            azure_account_name: String::new(),
            azure_tenant_id: String::new(),
            azure_client_id: String::new(),
            azure_client_secret: String::new(),
            gcs_bucket: String::new(),
            gcs_service_account_json_path: String::new(),
            local_bucket: String::new(),
            passphrase: String::new(),
        }
    }
}

impl From<&Prefill> for FieldValues {
    fn from(prefill: &Prefill) -> Self {
        let mut v = FieldValues::blank();
        v.name = prefill.name.clone();
        v.passphrase = prefill.bucket_cfg.passphrase.clone();
        match &prefill.bucket_cfg.backend {
            BackendConfig::S3(s) => {
                v.driver = "s3";
                v.s3_bucket = s.bucket.clone();
                v.s3_region = s.region.clone().unwrap_or_default();
                v.s3_endpoint_url = s.endpoint_url.clone().unwrap_or_default();
                v.s3_access_key_id = s.access_key_id.clone();
                v.s3_secret_access_key = s.secret_access_key.to_string();
                v.s3_path_style = s.path_style;
            }
            BackendConfig::R2(r) => {
                v.driver = "r2";
                v.r2_bucket = r.bucket.clone();
                v.r2_account_id = r.account_id.clone();
                v.r2_access_key_id = r.access_key_id.clone();
                v.r2_secret_access_key = r.secret_access_key.to_string();
            }
            BackendConfig::Azure(a) => {
                v.driver = "azure";
                v.azure_bucket = a.bucket.clone();
                v.azure_account_name = a.account_name.clone();
                v.azure_tenant_id = a.tenant_id.clone();
                v.azure_client_id = a.client_id.clone();
                v.azure_client_secret = a.client_secret.to_string();
            }
            BackendConfig::Gcs(g) => {
                v.driver = "gcs";
                v.gcs_bucket = g.bucket.clone();
                v.gcs_service_account_json_path = g.service_account_json_path.clone();
            }
            BackendConfig::Local(l) => {
                v.driver = "local";
                v.local_bucket = l.bucket.clone();
            }
        }
        v
    }
}

#[cfg(any(test, feature = "test-util"))]
pub fn render_form(error: Option<&str>, prefill: Option<&Prefill>) -> String {
    render_form_impl(error, prefill)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_form(error: Option<&str>, prefill: Option<&Prefill>) -> String {
    render_form_impl(error, prefill)
}

fn render_form_impl(error: Option<&str>, prefill: Option<&Prefill>) -> String {
    let flash = error
        .map(|e| format!("<p class=\"flash flash-err\">{}</p>", esc(e)))
        .unwrap_or_default();
    let intro = if prefill.is_some() {
        r#"<p class="muted">This bucket's config already connects fine, but nobody has created an
admin user for the web UI yet — everything below is filled in from <code>gitquarry.toml</code>,
edit anything that needs it. Only the admin user at the bottom is new.</p>"#
    } else {
        r#"<p class="muted">Couldn't find a config. Fill this out once: the entry name, the
storage driver and its credentials, the passphrase that encrypts everything that gets uploaded,
and an admin user for the web UI. It gets saved to <code>gitquarry.toml</code> (local) and to the
bucket itself.</p>"#
    };
    let v = prefill.map(FieldValues::from).unwrap_or_else(FieldValues::blank);
    let selected = |driver: &str| if v.driver == driver { "selected" } else { "" };
    let checked = |flag: bool| if flag { "checked" } else { "" };
    let val = esc; // same escaping an attribute value needs as a text node
    let body = format!(
        r#"{intro}
<p class="muted">This page doesn't use JavaScript, so it shows all five drivers' fields together
instead of showing/hiding based on what you pick in "driver" — only the section for the driver you
picked is read, the other four are ignored. Each driver has its own fields below, nothing is
shared between sections.</p>
{flash}
<form method="post" action="/">
<h4>entry</h4>
<table class="commit-meta">
<tr><td>name</td><td><input name="name" size="40" required autofocus value="{name}" placeholder="how you'll refer to this (--bucket, [buckets.&lt;name&gt;])"></td></tr>
<tr><td>driver</td><td>
<select name="driver">
<option value="s3" {s3_sel}>s3 (AWS S3 or compatible: MinIO, etc.)</option>
<option value="r2" {r2_sel}>r2 (Cloudflare R2)</option>
<option value="azure" {azure_sel}>azure (Azure Blob Storage)</option>
<option value="gcs" {gcs_sel}>gcs (Google Cloud Storage)</option>
<option value="local" {local_sel}>local (directory on this filesystem)</option>
</select>
</td></tr>
</table>

<h4>driver: s3</h4>
<table class="commit-meta">
<tr><td>bucket</td><td><input name="s3_bucket" size="40" value="{s3_bucket}"></td></tr>
<tr><td>region</td><td><input name="s3_region" size="40" value="{s3_region}" placeholder="us-east-1"></td></tr>
<tr><td>endpoint url</td><td><input name="s3_endpoint_url" size="40" value="{s3_endpoint_url}" placeholder="(empty = AWS S3; for MinIO/etc)"></td></tr>
<tr><td>access key id</td><td><input name="s3_access_key_id" size="40" value="{s3_access_key_id}"></td></tr>
<tr><td>secret access key</td><td><input type="password" name="s3_secret_access_key" size="40" value="{s3_secret_access_key}"></td></tr>
<tr><td>path-style</td><td><label><input type="checkbox" name="s3_path_style" value="1" {s3_path_style_checked}> needed for some S3-compatible providers</label></td></tr>
</table>

<h4>driver: r2</h4>
<table class="commit-meta">
<tr><td>bucket</td><td><input name="r2_bucket" size="40" value="{r2_bucket}"></td></tr>
<tr><td>account id</td><td><input name="r2_account_id" size="40" value="{r2_account_id}"></td></tr>
<tr><td>access key id</td><td><input name="r2_access_key_id" size="40" value="{r2_access_key_id}"></td></tr>
<tr><td>secret access key</td><td><input type="password" name="r2_secret_access_key" size="40" value="{r2_secret_access_key}"></td></tr>
</table>

<h4>driver: azure</h4>
<p class="muted">Auth is Entra ID (an app registration's client secret), not a storage account key.</p>
<table class="commit-meta">
<tr><td>container</td><td><input name="azure_bucket" size="40" value="{azure_bucket}"></td></tr>
<tr><td>account name</td><td><input name="azure_account_name" size="40" value="{azure_account_name}"></td></tr>
<tr><td>tenant id</td><td><input name="azure_tenant_id" size="40" value="{azure_tenant_id}"></td></tr>
<tr><td>client id</td><td><input name="azure_client_id" size="40" value="{azure_client_id}"></td></tr>
<tr><td>client secret</td><td><input type="password" name="azure_client_secret" size="40" value="{azure_client_secret}"></td></tr>
</table>

<h4>driver: gcs</h4>
<table class="commit-meta">
<tr><td>bucket</td><td><input name="gcs_bucket" size="40" value="{gcs_bucket}"></td></tr>
<tr><td>service account json path</td><td><input name="gcs_service_account_json_path" size="40" value="{gcs_service_account_json_path}" placeholder="/etc/gitquarry/gcs-key.json"></td></tr>
</table>

<h4>driver: local</h4>
<table class="commit-meta">
<tr><td>directory</td><td><input name="local_bucket" size="40" value="{local_bucket}" placeholder="/var/lib/gitquarry/my-bucket"></td></tr>
</table>

<h4>encryption</h4>
<table class="commit-meta">
<tr><td>passphrase</td><td><input type="password" name="passphrase" size="40" required value="{passphrase}"></td></tr>
</table>
<h4>admin user (web UI)</h4>
<table class="commit-meta">
<tr><td>username</td><td><input name="admin_username" size="30" required></td></tr>\n<tr><td>email</td><td><input type="email" name="admin_email" size="30" placeholder="optional, but only settable here or later via /_users"></td></tr>
<tr><td>password</td><td><input type="password" name="admin_password" size="30" required></td></tr>
<tr><td>confirm password</td><td><input type="password" name="admin_password_confirm" size="30" required></td></tr>
</table>
<p><button type="submit">create and start</button></p>
</form>"#,
        name = val(&v.name),
        s3_sel = selected("s3"),
        r2_sel = selected("r2"),
        azure_sel = selected("azure"),
        gcs_sel = selected("gcs"),
        local_sel = selected("local"),
        s3_bucket = val(&v.s3_bucket),
        s3_region = val(&v.s3_region),
        s3_endpoint_url = val(&v.s3_endpoint_url),
        s3_access_key_id = val(&v.s3_access_key_id),
        s3_secret_access_key = val(&v.s3_secret_access_key),
        s3_path_style_checked = checked(v.s3_path_style),
        r2_bucket = val(&v.r2_bucket),
        r2_account_id = val(&v.r2_account_id),
        r2_access_key_id = val(&v.r2_access_key_id),
        r2_secret_access_key = val(&v.r2_secret_access_key),
        azure_bucket = val(&v.azure_bucket),
        azure_account_name = val(&v.azure_account_name),
        azure_tenant_id = val(&v.azure_tenant_id),
        azure_client_id = val(&v.azure_client_id),
        azure_client_secret = val(&v.azure_client_secret),
        gcs_bucket = val(&v.gcs_bucket),
        gcs_service_account_json_path = val(&v.gcs_service_account_json_path),
        local_bucket = val(&v.local_bucket),
        passphrase = val(&v.passphrase),
    );
    layout("setup", "", &body, "", "", "", gitquarry_i18n::Locale::DEFAULT)
}
