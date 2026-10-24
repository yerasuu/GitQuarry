//! `PublishMirror` storage and the save edge: everything a form submits is
//! validated before it is stored, and the host allowlist applies exactly as
//! it does to an external mirror.

use gitquarry_core::app::AppContext;
use gitquarry_core::storage::Cas;
use gitquarry_mirror_external::publish::{self, FilterConfig, IdentityMapping, PublishMirror, SecretAllow};
use gitquarry_test_support::ctx;

fn sample() -> PublishMirror {
    PublishMirror {
        label: "public".to_string(),
        remote_url: "https://github.com/acme/public.git".to_string(),
        filter: FilterConfig {
            include: vec!["src/**".to_string(), "LICENSE".to_string()],
            exclude: vec!["*.env".to_string()],
            ..Default::default()
        },
        publish_branches: ["main".to_string()].into(),
        publish_tags: ["v*".to_string()].into(),
        ..Default::default()
    }
}

async fn repo(ctx: &AppContext) -> gitquarry_core::storage::uid::Uid {
    ctx.create_repo("alice/private").await.unwrap().unwrap().uid
}

#[tokio::test]
async fn create_get_update_list_delete_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx(root.path());
    let uid = repo(&ctx).await;

    let created = publish::create(
        &ctx,
        &uid,
        PublishMirror {
            id: "ignored".to_string(),
            ..sample()
        },
    )
    .await
    .unwrap();
    assert_ne!(created.id, "ignored", "create mints its own id");

    let (mut stored, etag) = publish::get(&ctx, &uid, &created.id).await.unwrap().unwrap();
    assert_eq!(stored.filter.include, ["src/**", "LICENSE"]);
    assert!(stored.filter.exclude_submodules && stored.filter.strip_empty_dirs);

    stored.label = "renamed".to_string();
    assert!(matches!(
        publish::update(&ctx, &uid, &stored, &etag).await.unwrap(),
        Cas::Ok
    ));
    assert!(
        matches!(
            publish::update(&ctx, &uid, &stored, &etag).await.unwrap(),
            Cas::Conflict
        ),
        "a stale etag loses the race"
    );

    let listed = publish::list_with_etags(&ctx, &uid).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0.label, "renamed");

    publish::delete(&ctx, &uid, &created.id).await.unwrap();
    assert!(publish::get(&ctx, &uid, &created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_host_outside_the_allowlist_is_refused_at_save() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx(root.path());
    let uid = repo(&ctx).await;
    let err = publish::create(
        &ctx,
        &uid,
        PublishMirror {
            remote_url: "https://evil.example/acme/public.git".to_string(),
            ..sample()
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("evil.example"), "{err}");
    assert!(publish::list_with_etags(&ctx, &uid).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_ref_pattern_aimed_outside_heads_and_tags_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx(root.path());
    let uid = repo(&ctx).await;
    for pattern in [
        "refs/merge-requests/*",
        "refs/notes/commits",
        "refs/gitquarry/x",
        "-x",
        "a..b",
    ] {
        let mirror = PublishMirror {
            publish_branches: [pattern.to_string()].into(),
            ..sample()
        };
        assert!(
            publish::create(&ctx, &uid, mirror).await.is_err(),
            "{pattern} accepted as a branch pattern"
        );
    }
}

#[tokio::test]
async fn untrusted_fields_are_validated_before_storing() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx(root.path());
    let uid = repo(&ctx).await;
    let bad = [
        PublishMirror {
            filter: FilterConfig {
                exclude: vec!["!deploy/**".to_string()],
                ..Default::default()
            },
            ..sample()
        },
        PublishMirror {
            filter: FilterConfig {
                include: vec!["../outside".to_string()],
                ..Default::default()
            },
            ..sample()
        },
        PublishMirror {
            identities: vec![IdentityMapping {
                private: "ana@internal.corp".to_string(),
                public: "ana>\ncommitter x".to_string(),
                name: None,
            }],
            ..sample()
        },
        PublishMirror {
            spdx: Some("MIT\nEvil".to_string()),
            ..sample()
        },
        PublishMirror {
            secret_allowlist: [SecretAllow {
                path: "a".to_string(),
                sha: "not-a-sha".to_string(),
            }]
            .into(),
            ..sample()
        },
        PublishMirror {
            remote_url: "https://user:pw@github.com/acme/public.git".to_string(),
            ..sample()
        },
    ];
    for mirror in bad {
        assert!(
            publish::create(&ctx, &uid, mirror.clone()).await.is_err(),
            "{mirror:?} accepted"
        );
    }
}
