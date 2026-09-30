// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

pub struct GeminiEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    base_url: Arc<dyn Fn() -> String + Send + Sync>,
}

impl GeminiEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct Gemini embedding HTTP client"),
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
            .map_err(|error| format!("invalid Gemini API base URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
            return Err("Gemini API base URL must be HTTP or HTTPS".into());
        }
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| "Gemini API base URL cannot be a base".to_owned())?;
            path.pop_if_empty()
                .push(&format!("{model}:batchEmbedContents"));
        }
        Ok(url)
    }
}

impl Embedder for GeminiEmbedder {
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
            return Err("Gemini API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_GEMINI_API_KEY='<API_KEY>'".into());
        }
        let endpoint = self.endpoint(model)?;
        let requests = texts
            .iter()
            .map(|text| {
                let mut request = Map::new();
                request.extend(
                    opts.iter()
                        .map(|(name, value)| (name.clone(), value.clone())),
                );
                request.insert("model".into(), Value::String(format!("models/{model}")));
                request.insert(
                    "content".into(),
                    serde_json::json!({"parts":[{"text":text}]}),
                );
                Value::Object(request)
            })
            .collect::<Vec<_>>();
        let response = self
            .client
            .post(endpoint)
            .header("x-goog-api-key", key)
            .json(&serde_json::json!({"requests":requests}))
            .send()
            .map_err(|error| format!("Gemini embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("Gemini response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let data: Value = serde_json::from_slice(&body)
            .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
        if !status.is_success() {
            return Err(format!(
                "Gemini: status code {}: {}",
                status.as_u16(),
                data["error"]["message"].as_str().unwrap_or_default()
            ));
        }
        let embeddings = data["embeddings"]
            .as_array()
            .ok_or_else(|| "Gemini response embeddings are missing".to_owned())?;
        if embeddings.len() != texts.len() {
            return Err(format!(
                "response embeddings length {} does not match input texts length {}",
                embeddings.len(),
                texts.len()
            ));
        }
        embeddings
            .iter()
            .map(|embedding| {
                embedding["values"]
                    .as_array()
                    .ok_or_else(|| "Gemini embedding values are missing".to_owned())?
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .map(|value| value as f32)
                            .ok_or_else(|| "Gemini embedding value must be numeric".to_owned())
                    })
                    .collect()
            })
            .collect()
    }
}
