//! This crate is used to create a nix derivation file. The derivation uses [`buildRustCrate`][buildRustCrate] to build
//! and cache each dependency individually. This allows the cache to be shared between projects if the dependency is
//! the same version with the same features activated.
//!
//! [buildRustCrate]: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md#buildrustcrate-compiling-rust-crates-using-nix-instead-of-cargo-compiling-rust-crates-using-nix-instead-of-cargo

use thiserror::Error;

pub mod models;

/// Errors that can happen while reading cargo metadata
#[derive(Debug, Error)]
pub enum Error {
    #[error("target spec failed: {0}")]
    TargetSpec(#[from] target_spec::Error),

    #[error("failed to read cargo metadata: {0}")]
    Metadata(#[from] cargo_metadata::Error),

    #[error("failed to read cargo lock file: {0}")]
    LockFile(#[from] cargo_lock::Error),
}
