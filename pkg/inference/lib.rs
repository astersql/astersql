// Copyright 2026 AsterSQL.

pub mod base;
pub mod cohere;
pub mod embed_fn;
pub mod gemini;
pub mod huggingface;
pub mod jina;
pub mod mock;
pub mod nvidia;
pub mod openai;
pub mod tidbcloud;
pub use embed_fn::{EmbedFn, Embedder, Options};
pub use mock::MockEmbedder;

#[cfg(test)]
mod base_test;
#[cfg(test)]
mod cohere_test;
#[cfg(test)]
mod embed_fn_test;
#[cfg(test)]
mod gemini_test;
#[cfg(test)]
mod huggingface_test;
#[cfg(test)]
mod jina_test;
#[cfg(test)]
mod nvidia_test;
#[cfg(test)]
mod openai_test;
#[cfg(test)]
mod tidbcloud_test;
