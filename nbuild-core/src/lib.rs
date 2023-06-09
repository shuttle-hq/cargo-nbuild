#![doc = include_str!("../README.md")]

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
