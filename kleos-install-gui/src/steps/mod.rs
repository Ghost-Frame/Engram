//! Step modules for the Kleos installer wizard.
//!
//! Each sub-module renders one wizard step and exposes a single `draw_*`
//! function that accepts a mutable reference to the shared [`InstallerApp`]
//! state. Steps are stateless renderers -- they read and write app state but
//! do not own it.

pub mod advanced;
pub mod components;
pub mod embeddings;
pub mod security;
pub mod server;
pub mod summary;
pub mod system;
