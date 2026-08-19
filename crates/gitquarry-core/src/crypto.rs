//! Encryption at rest lives in `s3db` (bucket bootstrap needs `Backend`, so
//! that part stays there); the pure `Cipher` primitive itself lives in
//! `s3db-crypto`, depended on directly here rather than through `s3db` --
//! `auth::web::login`'s cookie sealing has nothing to do with bucket storage, and
//! shouldn't need to pull in `s3db`'s driver dependencies just to reach
//! `Cipher`. Re-exported here so every existing `crate::crypto::...` call
//! site keeps working unchanged.
pub use s3db::bootstrap;
pub use s3db_crypto::{Cipher, CompressionStrategy};

/// Only ever needed to build a throwaway `Cipher::derive` salt in tests
/// across the tree — real ciphers derive from the bucket's own stored salt,
/// never a hardcoded length constant. `feature = "test-util"` for the same
/// cross-crate reason as `storage::local`.
#[cfg(any(test, feature = "test-util"))]
pub use s3db_crypto::SALT_LEN;
