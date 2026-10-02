//! Entity CRUD for the federation-owned entities — [`peer`] (linked
//! instances, layer 1), [`token`] (fine-grained access tokens, layer 4),
//! [`peer_credential`] (a local user's own credential toward a linked peer,
//! layer 4), [`fork`] (requesting a fork from a linked peer, layer 2).

pub mod fork;
pub mod peer;
pub mod peer_credential;
pub mod token;
