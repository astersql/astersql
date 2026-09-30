// Copyright 2026 AsterSQL.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use reqwest::blocking::Client;
use serde_json::{Map, Value};

use crate::embed_fn::{Embedder, Options};

const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// OpenAI-compatible embedding provider. The getters read current global
/// configuration for each request, matching Go's dynamic sysvar behavior.
pub struct OpenAIEmbedder {
    client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    base_url: Arc<dyn Fn() -> String + Send + Sync>,
}

impl OpenAIEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct OpenAI embedding HTTP client"),
            api_key: Arc::new(api_key),
            base_url: Arc::new(base_url),
        }
    }

    fn endpoint(&self) -> Result<reqwest::Url, String> {
        let configured = (self.base_url)();
        let base = if configured.trim().is_empty() {
            DEFAULT_API_BASE_URL
        } else {
            configured.trim()
        };
        let mut url = reqwest::Url::parse(base)
            .map_err(|error| format!("invalid OpenAI API base URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
            return Err("OpenAI API base URL must be HTTP or HTTPS".into());
        }
        let path = url.path().trim_end_matches('/');
        if !path.ends_with("/embeddings") {
            let path = format!("{path}/embeddings");
            url.set_path(&path);
        }
        Ok(url)
    }
}

impl Embedder for OpenAIEmbedder {
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
        let api_key = (self.api_key)();
        if api_key.is_empty() {
            return Err("OpenAI API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_OPENAI_API_KEY='<API_KEY>'".into());
        }
        let endpoint = self.endpoint()?;
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
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .map_err(|error| format!("OpenAI embedding request failed: {error}"))?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| format!("OpenAI response read failed: {error}"))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(format!(
                "response body exceeds maximum size of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        if status.as_u16() == 401 {
            return Err("OpenAI returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_OPENAI_API_KEY='<API_KEY>'".into());
        }
        if !status.is_success() {
            return Err(format!("OpenAI: status code {}", status.as_u16()));
        }
        decode_indexed_base64_embeddings(&body, texts.len())
    }
}

/// Decode the indexed base64 wire format shared by OpenAI-compatible and Jina APIs.
pub(crate) fn decode_indexed_base64_embeddings(
    body: &[u8],
    expected_count: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    let items = response["data"]
        .as_array()
        .ok_or_else(|| "OpenAI response data is missing".to_owned())?;
    if items.len() != expected_count {
        return Err(format!(
            "response data length {} does not match input texts length {}",
            items.len(),
            expected_count
        ));
    }
    let mut embeddings = vec![None; expected_count];
    for item in items {
        let index = item["index"]
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| "response data index is invalid".to_owned())?;
        if index >= expected_count {
            return Err(format!(
                "response data index {index} is out of range [0, {})",
                expected_count
            ));
        }
        if embeddings[index].is_some() {
            return Err(format!("response data contains duplicate index {index}"));
        }
        let encoded = item["embedding"]
            .as_str()
            .ok_or_else(|| format!("missing embedding for index {index}"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("failed to decode embedding for index {index}: {error}"))?;
        if bytes.is_empty() || bytes.len() % 4 != 0 {
            return Err(format!("invalid embedding data for index {index}"));
        }
        let values = bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
            .collect();
        embeddings[index] = Some(values);
    }
    Ok(embeddings.into_iter().map(Option::unwrap).collect())
}
