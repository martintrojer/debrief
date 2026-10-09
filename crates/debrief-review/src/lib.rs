//! Review comments on files and diffs, batched per repo root and handed back to an agent.

mod comment;
mod store;

pub use comment::{Comment, Rev, Side};
pub use store::{Review, cache_dir, repo_root};
