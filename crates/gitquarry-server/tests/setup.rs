use gitquarry::setup::*;
use gitquarry_core::config::{BackendConfig, BucketConfig, LocalEngine, LocalParams};

/// Every field blank — each test only fills in what its driver actually
/// reads, same as a real submission where the other four drivers'
/// sections come through empty (see `render_form`'s doc note: no
/// JavaScript to hide them).
fn blank_form() -> WizardForm {
    WizardForm {
        name: String::new(),
        driver: String::new(),
        s3_bucket: String::new(),
        s3_region: String::new(),
        s3_endpoint_url: String::new(),
        s3_access_key_id: String::new(),
        s3_secret_access_key: String::new(),
        s3_path_style: String::new(),
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
        admin_username: String::new(),
        admin_email: String::new(),
        admin_password: String::new(),
        admin_password_confirm: String::new(),
    }
}

#[test]
fn builds_s3_config_and_ignores_other_drivers_fields() {
    let form = WizardForm {
        driver: "s3".to_string(),
        s3_bucket: "my-bucket".to_string(),
        s3_access_key_id: "AKIA".to_string(),
        s3_secret_access_key: "secret".to_string(),
        // Belongs to other drivers' sections — must be ignored.
        gcs_service_account_json_path: "/should/be/ignored".to_string(),
        ..blank_form()
    };
    let BackendConfig::S3(params) = build_backend_config(&form, None).unwrap() else {
        panic!("expected driver s3");
    };
    assert_eq!(params.bucket, "my-bucket");
    assert_eq!(params.access_key_id, "AKIA");
}

#[test]
fn s3_rejects_an_empty_bucket() {
    let form = WizardForm {
        driver: "s3".to_string(),
        ..blank_form()
    };
    assert!(build_backend_config(&form, None).is_err());
}

#[test]
fn builds_r2_config_deriving_from_account_id() {
    let form = WizardForm {
        driver: "r2".to_string(),
        r2_bucket: "my-bucket".to_string(),
        r2_account_id: "acct123".to_string(),
        r2_access_key_id: "AKIA".to_string(),
        r2_secret_access_key: "secret".to_string(),
        ..blank_form()
    };
    let BackendConfig::R2(params) = build_backend_config(&form, None).unwrap() else {
        panic!("expected driver r2");
    };
    assert_eq!(params.account_id, "acct123");
}

#[test]
fn builds_azure_config() {
    let form = WizardForm {
        driver: "azure".to_string(),
        azure_bucket: "my-container".to_string(),
        azure_account_name: "acct".to_string(),
        azure_tenant_id: "tenant".to_string(),
        azure_client_id: "client".to_string(),
        azure_client_secret: "secret".to_string(),
        ..blank_form()
    };
    let BackendConfig::Azure(params) = build_backend_config(&form, None).unwrap() else {
        panic!("expected driver azure");
    };
    assert_eq!(params.bucket, "my-container");
}

#[test]
fn builds_gcs_config() {
    let form = WizardForm {
        driver: "gcs".to_string(),
        gcs_bucket: "my-bucket".to_string(),
        gcs_service_account_json_path: "/etc/key.json".to_string(),
        ..blank_form()
    };
    let BackendConfig::Gcs(params) = build_backend_config(&form, None).unwrap() else {
        panic!("expected driver gcs");
    };
    assert_eq!(params.service_account_json_path, "/etc/key.json");
}

#[test]
fn builds_local_config_defaulting_to_the_redb_engine_for_a_brand_new_bucket() {
    let form = WizardForm {
        driver: "local".to_string(),
        local_bucket: "/data/bucket".to_string(),
        ..blank_form()
    };
    let BackendConfig::Local(params) = build_backend_config(&form, None).unwrap() else {
        panic!("expected driver local");
    };
    assert_eq!(params.bucket, "/data/bucket");
    assert_eq!(params.engine, LocalEngine::Redb);
}

/// Resubmitting an existing local bucket (the prefill path) must keep
/// whatever engine it's actually formatted as on disk, not silently
/// flip it to the new-bucket default -- that would write a config
/// claiming a format the directory was never migrated to.
#[test]
fn builds_local_config_preserving_the_existing_engine_on_resubmit() {
    let form = WizardForm {
        driver: "local".to_string(),
        local_bucket: "/data/bucket".to_string(),
        ..blank_form()
    };
    let existing = BackendConfig::Local(LocalParams {
        bucket: "/data/bucket".to_string(),
        engine: LocalEngine::Files,
    });
    let BackendConfig::Local(params) = build_backend_config(&form, Some(&existing)).unwrap() else {
        panic!("expected driver local");
    };
    assert_eq!(params.engine, LocalEngine::Files);
}

#[test]
fn rejects_an_unknown_driver() {
    let form = WizardForm {
        driver: "ftp".to_string(),
        ..blank_form()
    };
    assert!(build_backend_config(&form, None).is_err());
}

fn s3_prefill() -> Prefill {
    Prefill {
        name: "main".to_string(),
        bucket_cfg: BucketConfig {
            backend: BackendConfig::S3(gitquarry_core::config::S3Params {
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

#[test]
fn field_values_from_prefill_carries_every_s3_field_over() {
    let v = FieldValues::from(&s3_prefill());
    assert_eq!(v.driver, "s3");
    assert_eq!(v.name, "main");
    assert_eq!(v.s3_bucket, "my-bucket");
    assert_eq!(v.s3_region, "us-east-1");
    assert_eq!(v.s3_access_key_id, "AKIA");
    assert_eq!(v.s3_secret_access_key, "s3cret");
    assert!(v.s3_path_style);
    assert_eq!(v.passphrase, "hunter2");
    // Fields belonging to other drivers stay blank.
    assert_eq!(v.r2_account_id, "");
    assert_eq!(v.azure_tenant_id, "");
}

#[test]
fn render_form_embeds_prefilled_values_and_selects_the_right_driver() {
    let html = render_form(None, Some(&s3_prefill()));
    assert!(html.contains(r#"value="my-bucket""#));
    assert!(html.contains(r#"value="AKIA""#));
    assert!(html.contains(r#"value="hunter2""#));
    assert!(html.contains(r#"<option value="s3" selected>"#));
    // The admin user is never pre-filled -- seeding one is the whole
    // point of this run of the wizard.
    assert!(!html.contains(r#"name="admin_username" size="30" required value"#));
}

#[test]
fn render_form_without_prefill_leaves_every_field_blank() {
    let html = render_form(None, None);
    assert!(html.contains(r#"name="name" size="40" required autofocus value="""#));
    assert!(html.contains(r#"<option value="s3" selected>"#)); // blank() still defaults to s3
    assert!(!html.contains(r#"value="my-bucket""#));
}
