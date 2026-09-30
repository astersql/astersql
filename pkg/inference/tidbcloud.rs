// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};

const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// TiDB Cloud Starter embedding service. An empty API key is valid and leaves
/// the Authorization header unset, matching the hosted service protocol.
pub struct TiDBCloudFreeEmbedder {
    client: Client,
    billing_id: Arc<dyn Fn() -> String + Send + Sync>,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    base_url: Arc<dyn Fn() -> String + Send + Sync>,
}

impl TiDBCloudFreeEmbedder {
    pub fn new(
        billing_id: impl Fn() -> String + Send + Sync + 'static,
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct TiDB Cloud embedding HTTP client"),
            billing_id: Arc::new(billing_id),
            api_key: Arc::new(api_key),
            base_url: Arc::new(base_url),
        }
    }

    fn endpoint(&self) -> Result<reqwest::Url, String> {
        let base = (self.base_url)();
        if base.is_empty() {
            return Err("base URL is not configured for TiDB Cloud Inference".into());
        }
        let mut url = reqwest::Url::parse(&base)
            .map_err(|error| format!("invalid TiDB Cloud Inference base URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
            return Err("TiDB Cloud Inference base URL must be HTTP or HTTPS".into());
        }
        let configured = (self.billing_id)();
        let billing_id = if configured.is_empty() {
            "default_billing_id"
        } else {
            &configured
        };
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| "TiDB Cloud Inference base URL cannot be a base".to_owned())?;
            path.pop_if_empty()
                .push("api")
                .push("v1")
                .push("inference")
                .push("embeddings")
                .push(billing_id);
        }
        Ok(url)
    }
}

impl Embedder for TiDBCloudFreeEmbedder {
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
        let endpoint = self.endpoint()?;
        let key = (self.api_key)();
        let mut payload = Map::new();
        payload.extend(
            opts.iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        payload.insert("model".into(), Value::String(model.into()));
        payload.insert("texts".into(), serde_json::json!(texts));
        let mut request = self.client.post(endpoint).json(&payload);
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .map_err(|error| format!("TiDB Cloud Inference embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("TiDB Cloud Inference response read failed: {error}"))?;
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
                "TiDB Cloud Inference: status code {}: {}",
                status.as_u16(),
                data["error"].as_str().unwrap_or_default()
            ));
        }
        let embeddings = data["embeddings"]
            .as_array()
            .ok_or_else(|| "TiDB Cloud Inference response embeddings are missing".to_owned())?;
        if embeddings.len() != texts.len() {
            return Err(format!(
                "response embeddings length {} does not match input texts length {}",
                embeddings.len(),
                texts.len()
            ));
        }
        embeddings
            .iter()
            .enumerate()
            .map(|(index, embedding)| {
                let encoded = embedding
                    .as_str()
                    .ok_or_else(|| format!("embedding {index} is not base64"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|error| {
                        format!("failed to decode embedding for index {index}: {error}")
                    })?;
                if bytes.len() % 4 != 0 || bytes.is_empty() {
                    return Err(format!("invalid embedding data for index {index}"));
                }
                Ok(bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
                    .collect())
            })
            .collect()
    }
}
