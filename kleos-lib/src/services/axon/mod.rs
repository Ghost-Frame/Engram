//! Axon event bus service.
//!
//! Pub/sub event bus with channels, subscriptions, cursor-based consumption,
//! webhook fan-out, and event retention. Submodules split core CRUD from
//! delivery and maintenance concerns.

mod core;

/// Webhook fan-out -- delivers events to subscriber webhook URLs.
pub mod fanout;

/// Event retention -- prunes expired events per channel's retain_hours.
pub mod retention;

pub use core::*;
