// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};

const DEFAULT_ENDPOINT: &str = "https://api.cohere.com/v1/embed";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Cohere's embedding endpoint accepts `texts` and returns either untyped
/// arrays or the `embeddings.float` object when `embedding_types` is set.
pub struct CohereEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    endpoint: Arc<dyn Fn() -> String + Send + Sync>,
}

impl CohereEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        endpoint: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct Cohere embedding HTTP client"),
            api_key: Arc::new(api_key),
            endpoint: Arc::new(endpoint),
        }
    }
}

impl Embedder for CohereEmbedder {
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
        if let Some(types) = opts.get("embedding_types")
            && types != &serde_json::json!(["float"])
        {
            return Err("Cohere embedding_types must be exactly [\"float\"]".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let key = (self.api_key)();
        if key.is_empty() {
            return Err("Cohere API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_COHERE_API_KEY='<API_KEY>'".into());
        }
        let configured = (self.endpoint)();
        let endpoint = if configured.trim().is_empty() {
            DEFAULT_ENDPOINT
        } else {
            configured.trim()
        };
        let endpoint = reqwest::Url::parse(endpoint)
            .map_err(|error| format!("invalid Cohere API base URL: {error}"))?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host().is_none() {
            return Err("Cohere API base URL must be HTTP or HTTPS".into());
        }
        let mut payload = Map::new();
        payload.extend(
            opts.iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        payload.insert("model".into(), Value::String(model.into()));
        payload.insert("texts".into(), serde_json::json!(texts));
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(key)
            .json(&payload)
            .send()
            .map_err(|error| format!("Cohere embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("Cohere response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        if status.as_u16() == 401 {
            return Err("Cohere returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_COHERE_API_KEY='<API_KEY>'".into());
        }
        if !status.is_success() {
            let detail = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| value["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(format!("Cohere: status code {}: {detail}", status.as_u16()));
        }
        decode_embeddings(&body, texts.len())
    }
}

fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    let embeddings = &response["embeddings"];
    let embeddings = if embeddings.is_array() {
        embeddings
    } else {
        &embeddings["float"]
    };
    let embeddings = embeddings
        .as_array()
        .ok_or_else(|| "Cohere response does not contain float embeddings".to_owned())?;
    if embeddings.len() != expected {
        return Err(format!(
            "response embeddings length {} does not match input texts length {expected}",
            embeddings.len()
        ));
    }
    embeddings
        .iter()
        .map(|embedding| {
            let values = embedding
                .as_array()
                .ok_or_else(|| "Cohere embedding must be an array".to_owned())?;
            values
                .iter()
                .map(|value| {
                    value
                        .as_f64()
                        .map(|value| value as f32)
                        .ok_or_else(|| "Cohere embedding value must be numeric".to_owned())
                })
                .collect()
        })
        .collect()
}
