pub mod db;
pub mod validation;
pub mod xpub;

// Umbrel packaging layer — dashboard API endpoints. Server-only (needs Actix).
// Kept separate from upstream src/ so future syncs from Gitea stay trivial.
#[cfg(feature = "server")]
pub mod umbrel_api;
