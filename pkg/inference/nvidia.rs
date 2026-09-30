// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};
use crate::openai::decode_indexed_base64_embeddings;

const DEFAULT_ENDPOINT: &str = "https://integrate.api.nvidia.com/v1/embeddings";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

pub struct NvidiaEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    endpoint: Arc<dyn Fn() -> String + Send + Sync>,
}

impl NvidiaEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        endpoint: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct NVIDIA NIM embedding HTTP client"),
            api_key: Arc::new(api_key),
            endpoint: Arc::new(endpoint),
        }
    }
}

impl Embedder for NvidiaEmbedder {
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
        if let Some(kind) = opts.get("embedding_type")
            && kind != "float"
        {
            return Err("NVIDIA NIM embedding_type must be \"float\"".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let key = (self.api_key)();
        if key.is_empty() {
            return Err("NVIDIA NIM API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_NVIDIA_NIM_API_KEY='<API_KEY>'".into());
        }
        let configured = (self.endpoint)();
        let endpoint = if configured.trim().is_empty() {
            DEFAULT_ENDPOINT
        } else {
            configured.trim()
        };
        let endpoint = reqwest::Url::parse(endpoint)
            .map_err(|error| format!("invalid NVIDIA NIM API base URL: {error}"))?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host().is_none() {
            return Err("NVIDIA NIM API base URL must be HTTP or HTTPS".into());
        }
        let mut payload = Map::new();
        payload.extend(
            opts.iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        payload.insert("model".into(), Value::String(model.into()));
        payload.insert("input".into(), serde_json::json!(texts));
        payload.insert("encoding_format".into(), Value::String("base64".into()));
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(key)
            .json(&payload)
            .send()
            .map_err(|error| format!("NVIDIA NIM embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("NVIDIA NIM response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        match status.as_u16() {
            401 | 403 => return Err("NVIDIA NIM returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_NVIDIA_NIM_API_KEY='<API_KEY>'".into()),
            404 => return Err(format!("NVIDIA NIM model '{model}' does not exist or is not available")),
            _ => {}
        }
        if !status.is_success() {
            let detail = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| {
                    ["detail", "message", "error"]
                        .into_iter()
                        .find_map(|key| value[key].as_str().map(str::to_owned))
                })
                .unwrap_or_default();
            return Err(format!(
                "NVIDIA NIM: status code {}: {detail}",
                status.as_u16()
            ));
        }
        decode_indexed_base64_embeddings(&body, texts.len())
    }
}
