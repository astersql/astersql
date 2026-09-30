// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};
use crate::openai::decode_indexed_base64_embeddings;

const DEFAULT_API_ENDPOINT: &str = "https://api.jina.ai/v1/embeddings";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Jina AI embedding provider using its indexed base64 response format.
pub struct JinaEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    endpoint: Arc<dyn Fn() -> String + Send + Sync>,
}

impl JinaEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        endpoint: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct Jina embedding HTTP client"),
            api_key: Arc::new(api_key),
            endpoint: Arc::new(endpoint),
        }
    }
}

impl Embedder for JinaEmbedder {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if model.is_empty() {
            return Err("model name is required".into());
        }
        if opts.get("return_multivector") == Some(&Value::Bool(true)) {
            return Err("JinaAI option return_multivector=true is not supported".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let api_key = (self.api_key)();
        if api_key.is_empty() {
            return Err("JinaAI API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_JINA_AI_API_KEY='<API_KEY>'".into());
        }
        let configured = (self.endpoint)();
        let endpoint = if configured.trim().is_empty() {
            DEFAULT_API_ENDPOINT
        } else {
            configured.trim()
        };
        let endpoint = reqwest::Url::parse(endpoint)
            .map_err(|error| format!("invalid Jina AI API base URL: {error}"))?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host().is_none() {
            return Err("Jina AI API base URL must be HTTP or HTTPS".into());
        }
        let mut payload = Map::new();
        payload.extend(
            opts.iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        payload.insert("model".into(), Value::String(model.into()));
        payload.insert("input".into(), serde_json::json!(texts));
        payload.insert("embedding_type".into(), Value::String("base64".into()));
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .map_err(|error| format!("JinaAI embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("JinaAI response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        if status.as_u16() == 401 {
            return Err("JinaAI returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_JINA_AI_API_KEY='<API_KEY>'".into());
        }
        if !status.is_success() {
            return Err(format!("JinaAI: status code {}", status.as_u16()));
        }
        decode_indexed_base64_embeddings(&body, texts.len())
    }
}
