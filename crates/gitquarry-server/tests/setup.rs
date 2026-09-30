//! The first-run wizard driven over HTTP, the way an operator's browser
//! drives it: `setup::run` serves the form, a POST either re-renders it with
//! a 400 or completes setup and hands back the outcome.

use gitquarry::setup::{self, Prefill, WizardOutcome};
use gitquarry_core::config::{BackendConfig, BucketConfig, LocalEngine, LocalParams, S3Params};
use std::path::Path;
use std::time::Duration;
use tokio::task::JoinHandle;

struct Wizard {
    base: String,
    url: String,
    http: reqwest::Client,
    handle: JoinHandle<anyhow::Result<WizardOutcome>>,
}

async fn start(config_path: &Path, prefill: Option<Prefill>) -> Wizard {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bind = format!("127.0.0.1:{port}");
    let config_path = config_path.to_path_buf();
    let handle = tokio::spawn(async move { setup::run(&config_path, &bind, prefill).await });
    let wizard = Wizard {
        base: format!("http://127.0.0.1:{port}"),
        url: format!("http://127.0.0.1:{port}/_setup"),
        // Redirects are asserted on, not followed.
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        handle,
    };
    for _ in 0..100 {
        if wizard.http.get(&wizard.url).send().await.is_ok() {
            return wizard;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("setup wizard never started listening");
}

fn encode(fields: &[(&str, &str)]) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

impl Wizard {
    async fn get(&self) -> String {
        self.http.get(&self.url).send().await.unwrap().text().await.unwrap()
    }

    async fn post(&self, fields: &[(&str, &str)]) -> (u16, String) {
        let resp = self
            .http
            .post(&self.url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(encode(fields))
            .send()
            .await
            .unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }
}

/// Every field an operator must fill in regardless of driver.
fn with_admin<'a>(driver_fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut fields = vec![
        ("name", "main"),
        ("passphrase", "hunter2"),
        ("admin_username", "admin"),
        ("admin_password", "pw"),
        ("admin_password_confirm", "pw"),
    ];
    fields.extend_from_slice(driver_fields);
    fields
}

fn s3_prefill() -> Prefill {
    Prefill {
        name: "main".to_string(),
        bucket_cfg: BucketConfig {
            backend: BackendConfig::S3(S3Params {
                bucket: "my-bucket".to_string(),
                region: Some("us-east-1".to_string()),
                endpoint_url: None,
                access_key_id: "AKIA".to_string(),
                secret_access_key: "s3cret".to_string().into(),
                path_style: true,
                allow_insecure_http: false,
            }),
            passphrase: "hunter2".to_string(),
            resilience: Default::default(),
        },
    }
}

#[tokio::test]
async fn the_root_sends_to_setup_and_the_form_posts_back_there() {
    let dir = tempfile::tempdir().unwrap();
    let wizard = start(&dir.path().join("gitquarry.toml"), None).await;
    let root = wizard.http.get(format!("{}/", wizard.base)).send().await.unwrap();
    assert_eq!(root.status().as_u16(), 303);
    assert_eq!(root.headers()["location"], "/_setup");
    assert!(wizard.get().await.contains(r#"<form method="post" action="/_setup">"#));
}

/// The wizard is its own short-lived server, so it has to serve the
/// stylesheet and scripts the shared page chrome links, or the one page it
/// shows renders unstyled.
#[tokio::test]
async fn every_asset_the_wizard_page_links_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let wizard = start(&dir.path().join("gitquarry.toml"), None).await;
    let html = wizard.get().await;
    let assets: Vec<&str> = html
        .split(['"', '\''])
        .filter(|chunk| chunk.starts_with("/_assets/"))
        .collect();
    assert!(assets.iter().any(|a| a.starts_with("/_assets/style.css")), "{html}");
    for asset in assets {
        let resp = wizard.http.get(format!("{}{asset}", wizard.base)).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 200, "{asset}");
    }
}

#[tokio::test]
async fn blank_form_defaults_to_s3_with_every_field_empty() {
    let dir = tempfile::tempdir().unwrap();
    let wizard = start(&dir.path().join("gitquarry.toml"), None).await;
    let html = wizard.get().await;
    assert!(html.contains(r#"name="name" size="40" required autofocus value="""#));
    assert!(html.contains(r#"<option value="s3" selected>"#));
    assert!(!html.contains(r#"value="my-bucket""#));
}

/// The wizard is served before any admin exists, to whoever reaches the
/// port — so the stored credentials themselves never go back out.
#[tokio::test]
async fn prefilled_form_carries_the_existing_config_but_never_a_secret_or_an_admin() {
    let dir = tempfile::tempdir().unwrap();
    let wizard = start(&dir.path().join("gitquarry.toml"), Some(s3_prefill())).await;
    let html = wizard.get().await;
    for value in ["main", "my-bucket", "us-east-1", "AKIA"] {
        assert!(html.contains(&format!(r#"value="{value}""#)), "{value} not prefilled");
    }
    for secret in ["s3cret", "hunter2"] {
        assert!(!html.contains(secret), "{secret} rendered into the page");
    }
    assert!(html.contains(r#"<option value="s3" selected>"#));
    assert!(html.contains(r#"name="s3_path_style" value="1" checked"#));
    // Seeding an admin is the whole point of this run of the wizard.
    assert!(!html.contains(r#"name="admin_username" size="30" required value"#));
}

/// Leaving a secret blank keeps the stored one only while it would go to the
/// same place: pointing the form at another endpoint with the secret left
/// blank must not hand the stored secret to that endpoint.
#[tokio::test]
async fn a_blank_secret_is_reused_only_for_the_same_destination() {
    let dir = tempfile::tempdir().unwrap();
    let wizard = start(&dir.path().join("gitquarry.toml"), Some(s3_prefill())).await;
    let s3 = |endpoint: &'static str| {
        [
            ("driver", "s3"),
            ("s3_bucket", "my-bucket"),
            ("s3_region", "us-east-1"),
            ("s3_endpoint_url", endpoint),
            ("s3_access_key_id", "AKIA"),
            ("s3_path_style", "1"),
        ]
    };
    let mut elsewhere = with_admin(&s3("https://attacker.example"));
    elsewhere.retain(|(k, _)| *k != "passphrase");
    let (status, html) = wizard.post(&elsewhere).await;
    assert_eq!(status, 400);
    assert!(html.contains("s3: secret access key is empty"), "{html}");

    // Same destination: the blanks pass validation and the stored values are
    // used for the real round trip (which then fails here, having no network
    // S3 to reach — but never on a missing secret or passphrase).
    let mut same = with_admin(&s3(""));
    same.retain(|(k, _)| *k != "passphrase");
    let (status, html) = wizard.post(&same).await;
    assert_eq!(status, 400);
    assert!(
        !html.contains("is empty") && !html.contains("can&#39;t be empty"),
        "{html}"
    );
}

#[tokio::test]
async fn a_prefilled_local_resubmit_with_a_blank_passphrase_keeps_the_stored_one() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gitquarry.toml");
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let bucket_str = bucket.to_string_lossy().to_string();
    let prefill = Prefill {
        name: "main".to_string(),
        bucket_cfg: BucketConfig {
            backend: BackendConfig::Local(LocalParams {
                bucket: bucket_str.clone(),
                engine: LocalEngine::Redb,
            }),
            passphrase: "stored passphrase".to_string(),
            resilience: Default::default(),
        },
    };
    let wizard = start(&config, Some(prefill)).await;

    let mut fields = with_admin(&[("driver", "local"), ("local_bucket", &bucket_str)]);
    fields.retain(|(k, _)| *k != "passphrase");
    let (status, _) = wizard.post(&fields).await;
    assert_eq!(status, 200);
    let outcome = wizard.handle.await.unwrap().unwrap();
    assert_eq!(outcome.bucket_cfg.passphrase, "stored passphrase");
}

#[tokio::test]
async fn invalid_submissions_re_render_with_the_reason_and_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gitquarry.toml");
    let wizard = start(&config, None).await;

    let cases: [(&[(&str, &str)], &str); 7] = [
        (
            &[
                ("driver", "s3"),
                ("s3_access_key_id", "k"),
                ("s3_secret_access_key", "s"),
            ],
            "s3: bucket name can&#39;t be empty",
        ),
        (
            &[("driver", "s3"), ("s3_bucket", "b"), ("s3_secret_access_key", "s")],
            "s3: access key id is empty",
        ),
        (
            &[
                ("driver", "r2"),
                ("r2_bucket", "b"),
                ("r2_access_key_id", "k"),
                ("r2_secret_access_key", "s"),
            ],
            "r2: account id is empty",
        ),
        (
            &[
                ("driver", "azure"),
                ("azure_bucket", "c"),
                ("azure_account_name", "a"),
                ("azure_client_id", "i"),
                ("azure_client_secret", "s"),
            ],
            "azure: tenant id is empty",
        ),
        (
            &[("driver", "gcs"), ("gcs_bucket", "b")],
            "gcs: service account JSON path is empty",
        ),
        (&[("driver", "local")], "local: directory can&#39;t be empty"),
        (&[("driver", "ftp")], "unknown driver: ftp"),
    ];
    for (driver_fields, reason) in cases {
        let (status, html) = wizard.post(&with_admin(driver_fields)).await;
        assert_eq!(status, 400, "{reason}");
        assert!(html.contains(reason), "expected {reason:?} in the re-rendered form");
    }

    let mut mismatch = with_admin(&[("driver", "local"), ("local_bucket", "/unused")]);
    mismatch.retain(|(k, _)| *k != "admin_password_confirm");
    mismatch.push(("admin_password_confirm", "other"));
    let (status, html) = wizard.post(&mismatch).await;
    assert_eq!(status, 400);
    assert!(html.contains("admin password and confirmation don&#39;t match"));

    assert!(!config.exists());
    assert!(!wizard.handle.is_finished());
}

#[tokio::test]
async fn a_local_submit_writes_the_config_seeds_the_admin_and_starts_on_redb() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gitquarry.toml");
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let wizard = start(&config, None).await;

    let bucket_str = bucket.to_string_lossy();
    let (status, _) = wizard
        .post(&with_admin(&[("driver", "local"), ("local_bucket", &bucket_str)]))
        .await;
    assert_eq!(status, 200);

    let outcome = wizard.handle.await.unwrap().unwrap();
    assert_eq!(outcome.bucket, "main");
    assert!(matches!(
        &outcome.bucket_cfg.backend,
        BackendConfig::Local(LocalParams {
            engine: LocalEngine::Redb,
            ..
        })
    ));
    assert!(config.exists());
    assert_eq!(
        gitquarry_core::auth::access::list_usernames(outcome.backend.as_ref())
            .await
            .unwrap(),
        ["admin"]
    );
}

#[tokio::test]
async fn a_prefilled_local_resubmit_keeps_the_engine_already_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gitquarry.toml");
    let bucket = dir.path().join("bucket");
    std::fs::create_dir(&bucket).unwrap();
    let bucket_str = bucket.to_string_lossy().to_string();
    #[allow(deprecated)] // the Files engine is exactly the legacy layout this must not silently flip
    let files = LocalEngine::Files;
    let prefill = Prefill {
        name: "main".to_string(),
        bucket_cfg: BucketConfig {
            backend: BackendConfig::Local(LocalParams {
                bucket: bucket_str.clone(),
                engine: files,
            }),
            passphrase: "hunter2".to_string(),
            resilience: Default::default(),
        },
    };
    let wizard = start(&config, Some(prefill)).await;

    let (status, _) = wizard
        .post(&with_admin(&[("driver", "local"), ("local_bucket", &bucket_str)]))
        .await;
    assert_eq!(status, 200);

    let outcome = wizard.handle.await.unwrap().unwrap();
    #[allow(deprecated)]
    let kept_files = matches!(
        &outcome.bucket_cfg.backend,
        BackendConfig::Local(LocalParams {
            engine: LocalEngine::Files,
            ..
        })
    );
    assert!(kept_files);
    // The prefill path's config already exists by definition; it's never rewritten.
    assert!(!config.exists());
}
