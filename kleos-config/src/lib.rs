//! Shared Kleos configuration crate.
//!
//! This crate is the single source of truth for the runtime [`config::Config`]
//! schema and the [`env::kleos_env`] environment-variable resolver. It was
//! extracted from `kleos-lib` so that the installer (`kleos-install-core`) can
//! build and serialize a real [`config::Config`] without pulling in the heavy
//! server dependencies (ONNX, Lance, tokenizers, SQLite). Sharing one schema
//! guarantees the config the installer writes round-trips into the config the
//! server reads -- the two can no longer drift.
//!
//! `kleos-lib` re-exports everything here at its historical paths
//! (`kleos_lib::config`, `kleos_lib::kleos_env`) so existing call sites are
//! unaffected.

pub mod config;
pub mod env;

pub use config::*;
pub use env::kleos_env;
