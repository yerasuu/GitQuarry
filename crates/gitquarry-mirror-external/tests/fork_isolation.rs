//! A duplicated repo must not inherit its source's external mirrors — the
//! only place in a repo's own storage that holds a live, reusable
//! third-party credential. Lives here rather than in `gitquarry-core`'s own
//! tests because writing an `ExternalMirror` needs this crate.

use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_core::storage::uid::Uid;
use gitquarry_mirror_external::{access, domain};
use gitquarry_test_support::ctx;

#[tokio::test]
async fn a_fork_doesnt_inherit_the_sources_mirror_credential() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let source = gitquarry_core::domain::repo::create(&ctx, "alice/mirrored", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    domain::create(
        &ctx,
        &source.uid,
        access::ExternalMirror {
            remote_url: "https://github.com/alice/mirrored.git".to_string(),
            token: "ghp_alices_secret".to_string(),
            enabled: true,
            label: "github".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let fork = gitquarry_core::domain::repo::fork(
        &ctx,
        &source,
        "bob/mirrored",
        None,
        OwnerRef::User(Uid::from("u-bob".to_string())),
        gitquarry_i18n::Locale::DEFAULT,
    )
    .await
    .unwrap()
    .unwrap();

    let inherited = domain::list(&ctx, &fork.uid).await.unwrap();
    assert!(
        inherited.is_empty(),
        "the fork inherited {} mirror(s), credential included",
        inherited.len()
    );
    assert_eq!(domain::list(&ctx, &source.uid).await.unwrap().len(), 1);
}
