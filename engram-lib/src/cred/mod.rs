pub mod client;
pub mod pattern;
pub mod proxy;

pub use client::{CreddClient, SecretAccessMode};
pub use pattern::{find_secret_patterns, has_secret_patterns, SecretPattern, SecretPatternKind};
pub use proxy::{ProxyRequest, ProxyResponse};

#[cfg(test)]
mod tests;
