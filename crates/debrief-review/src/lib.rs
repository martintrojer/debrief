//! Review comments on files and diffs, batched per repo root and handed back to an agent.

mod comment;
mod export;
pub mod handback;
mod store;

pub use comment::{Comment, Kind, Rev, Side, builtin_kinds};
pub use store::{Review, cache_dir, repo_root};
