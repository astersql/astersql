// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use reqwest::Client;
use serde_json::Value;

use crate::base::{decode_float32_array_bytes, json_fields_with_options, sanitize_error_text};
use crate::embed_fn::{Embedder, Options};
use logutil::log::{BgLogger, LogField, LogLevel};

const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// OpenAI-compatible embedding provider. The getters read current global
/// configuration for each request, matching Go's dynamic sysvar behavior.
pub struct OpenAIEmbedder {
    pub(crate) client: Client,
    api_key: Arc<dyn Fn() -> String + Send + Sync>,
    base_url: Arc<dyn Fn() -> String + Send + Sync>,
    max_response_bytes: u64,
    missing_key_error: Option<String>,
    unauthorized_error: Option<String>,
}

/// Provider foundation configuration; absent getters and non-positive limits
/// preserve Go defaults. Runtime callers can supply deployment guidance.
#[derive(Default)]
pub struct OpenAIConfig {
    pub api_key: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub base_url: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub missing_key_error: Option<String>,
    pub unauthorized_error: Option<String>,
    pub max_response_bytes: i64,
}

impl OpenAIEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(OpenAIConfig {
            api_key: Some(Arc::new(api_key)),
            base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("OpenAI API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_OPENAI_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("OpenAI returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_OPENAI_API_KEY='<API_KEY>'".into()),
            ..OpenAIConfig::default()
        })
    }

    pub fn with_config(config: OpenAIConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("construct OpenAI embedding HTTP client"),
            api_key: config.api_key.unwrap_or_else(|| Arc::new(String::new)),
            base_url: config.base_url.unwrap_or_else(|| Arc::new(String::new)),
            max_response_bytes: if config.max_response_bytes > 0 {
                config.max_response_bytes as u64
            } else {
                MAX_RESPONSE_BYTES
            },
            missing_key_error: config.missing_key_error,
            unauthorized_error: config.unauthorized_error,
        }
    }

    pub(crate) fn endpoint(&self) -> Result<reqwest::Url, String> {
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
        let path = if path.ends_with("/embeddings") {
            path.to_owned()
        } else {
            format!("{path}/embeddings")
        };
        url.set_path(&path);
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
            return Err(self
                .missing_key_error
                .clone()
                .unwrap_or_else(|| "API key is not configured for OpenAI".into()));
        }
        let endpoint = self.endpoint()?;
        let payload = json_fields_with_options(
            Options::from([
                ("model".into(), Value::String(model.into())),
                ("input".into(), serde_json::json!(texts)),
                ("encoding_format".into(), Value::String("base64".into())),
            ]),
            opts,
        );
        // Dropping the in-flight future cancels socket IO, preserving Go's
        // request-context cancellation without detached HTTP worker threads.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("OpenAI request runtime failed: {error}"))?;
        let (status, body) = runtime.block_on(async {
            let request = async {
                let mut response = self
                    .client
                    .post(endpoint)
                    .bearer_auth(&api_key)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|error| format!("OpenAI embedding request failed: {error}"))?;
                let status = response.status();
                let mut body = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|error| format!("OpenAI response read failed: {error}"))?
                {
                    // Retain only limit+1 bytes even if a server sends a large chunk.
                    let remaining =
                        (self.max_response_bytes + 1).saturating_sub(body.len() as u64) as usize;
                    body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    if body.len() as u64 > self.max_response_bytes {
                        return Err(format!(
                            "response body exceeds maximum size of {} bytes",
                            self.max_response_bytes
                        ));
                    }
                }
                Ok((status, body))
            };
            let cancelled = async {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            use std::future::Future;
            let mut request = std::pin::pin!(request);
            let mut cancelled = std::pin::pin!(cancelled);
            std::future::poll_fn(|context| {
                if cancelled.as_mut().poll(context).is_ready() {
                    return std::task::Poll::Ready(Err("request canceled".to_owned()));
                }
                request.as_mut().poll(context)
            })
            .await
        })?;
        if cancel.load(Ordering::Acquire) {
            return Err("request canceled".into());
        }
        if status.as_u16() != 200 {
            let parsed = serde_json::from_slice::<Value>(&body);
            let message = parsed
                .as_ref()
                .ok()
                .and_then(|response| response["error"]["message"].as_str())
                .filter(|message| !message.is_empty())
                .map(|message| sanitize_error_text(message, &[&api_key]));
            let mut fields = vec![LogField::I64("status".into(), status.as_u16() as i64)];
            if let Some(message) = &message {
                fields.push(LogField::String("message".into(), message.clone()));
            }
            if let Err(error) = parsed {
                fields.push(LogField::String(
                    "parse_error".into(),
                    sanitize_error_text(&error.to_string(), &[&api_key]),
                ));
            }
            BgLogger().log(LogLevel::Error, "OpenAI API request failed", fields);
            if status.as_u16() == 401 {
                return Err(self.unauthorized_error.clone().unwrap_or_else(|| {
                    "OpenAI returns status unauthorized, check API key".into()
                }));
            }
            return Err(format!(
                "OpenAI: status code {}, message: {}",
                status.as_u16(),
                message.unwrap_or_else(|| status.canonical_reason().unwrap_or("").into())
            ));
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
    let empty = Vec::new();
    let items = if response["data"].is_null() {
        &empty
    } else {
        response["data"]
            .as_array()
            .ok_or_else(|| "unexpected unmarshal response error: data is not an array".to_owned())?
    };
    if items.len() != expected_count {
        return Err(format!(
            "response data length {} does not match input texts length {}",
            items.len(),
            expected_count
        ));
    }
    let mut embeddings = vec![None; expected_count];
    for item in items {
        let index = if item["index"].is_null() {
            0
        } else {
            item["index"].as_i64().ok_or_else(|| {
                "unexpected unmarshal response error: index is not an integer".to_owned()
            })?
        };
        if index < 0 || index as usize >= expected_count {
            return Err(format!(
                "response data index {index} is out of range [0, {expected_count})"
            ));
        }
        let index = index as usize;
        if embeddings[index].is_some() {
            return Err(format!("response data contains duplicate index {index}"));
        }
        let bytes = match &item["embedding"] {
            Value::Null => Vec::new(),
            Value::String(encoded) => {
                let encoded = encoded.replace(['\r', '\n'], "");
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|error| {
                        format!("failed to decode embedding for index {index}: {error}")
                    })?
            }
            Value::Array(bytes) => bytes
                .iter()
                .map(|value| {
                    if value.is_null() {
                        Ok(0)
                    } else {
                        value
                            .as_u64()
                            .and_then(|n| u8::try_from(n).ok())
                            .ok_or_else(|| {
                                "unexpected unmarshal response error: invalid byte".to_owned()
                            })
                    }
                })
                .collect::<Result<Vec<u8>, String>>()?,
            _ => return Err("unexpected unmarshal response error: invalid embedding".into()),
        };
        let values = decode_float32_array_bytes(&bytes)
            .map_err(|error| format!("failed to decode embedding for index {index}: {error}"))?;
        embeddings[index] = Some(values);
    }
    Ok(embeddings.into_iter().map(Option::unwrap).collect())
}
