//! Persistent code index and deterministic context retrieval facade.

mod extract;
mod index;
mod model;

pub use index::{resolve_repository_root, CodeIndex, IndexError};
pub use model::{
    CodeRelation, ContextPack, ContextQuery, ContextSnippet, IndexedSymbol, RefreshReport,
    RelationDirection, RelationQuery,
};
