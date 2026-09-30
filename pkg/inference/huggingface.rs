// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};

const DEFAULT_BASE_URL: &str = "https://router.huggingface.co/hf-inference";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

pub struct HuggingFaceEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    base_url: Arc<dyn Fn() -> String + Send + Sync>,
}

impl HuggingFaceEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct HuggingFace embedding HTTP client"),
            api_key: Arc::new(api_key),
            base_url: Arc::new(base_url),
        }
    }

    fn endpoint(&self, model: &str) -> Result<reqwest::Url, String> {
        let configured = (self.base_url)();
        let base = if configured.trim().is_empty() {
            DEFAULT_BASE_URL
        } else {
            configured.trim()
        };
        let mut url = reqwest::Url::parse(base)
            .map_err(|error| format!("invalid HuggingFace API base URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
            return Err("HuggingFace API base URL must be HTTP or HTTPS".into());
        }
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| "HuggingFace API base URL cannot be a base".to_owned())?;
            path.pop_if_empty();
            path.push("models");
            for segment in model.split('/') {
                path.push(segment);
            }
            path.push("pipeline").push("feature-extraction");
        }
        Ok(url)
    }
}

impl Embedder for HuggingFaceEmbedder {
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
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let key = (self.api_key)();
        if key.is_empty() {
            return Err("HuggingFace API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_HUGGINGFACE_API_KEY='<API_KEY>'".into());
        }
        let endpoint = self.endpoint(model)?;
        let mut payload = Map::new();
        payload.extend(
            opts.iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        payload.insert("inputs".into(), serde_json::json!(texts));
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(key)
            .json(&payload)
            .send()
            .map_err(|error| format!("HuggingFace embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("HuggingFace response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        match status.as_u16() {
            401 => return Err("HuggingFace returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_HUGGINGFACE_API_KEY='<API_KEY>'".into()),
            404 => return Err(format!("HuggingFace model '{model}' does not exist or is not available")),
            _ => {}
        }
        if !status.is_success() {
            let detail = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| value["error"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(format!(
                "HuggingFace: status code {}: {detail}",
                status.as_u16()
            ));
        }
        let embeddings: Vec<Vec<f32>> = serde_json::from_slice(&body)
            .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
        if embeddings.len() != texts.len() {
            return Err(format!(
                "response data length {} does not match input texts length {}",
                embeddings.len(),
                texts.len()
            ));
        }
        Ok(embeddings)
    }
}
