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

use crate::base::{self, APIKeyProviderConfig, ProviderContext, ProviderError};
use crate::{Embedder, Options};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
pub struct GeminiEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: APIKeyProviderConfig,
}
impl GeminiEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(APIKeyProviderConfig {
            api_key: Some(Arc::new(api_key)), base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("Gemini API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_GEMINI_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("Gemini returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_GEMINI_API_KEY='<API_KEY>'".into()),
            ..Default::default()
        })
    }
    pub fn with_config(cfg: APIKeyProviderConfig) -> Self {
        Self {
            client: base::http_client("Gemini"),
            cfg: cfg.with_defaults(),
        }
    }
    pub(crate) fn endpoint(&self, model: &str) -> Result<base::ProviderEndpoint, ProviderError> {
        let configured = self.cfg.configured_base_url();
        let configured = configured.trim();
        let url = base::parse_http_url(
            if configured.is_empty() {
                DEFAULT_BASE_URL
            } else {
                configured
            },
            "Gemini API base URL",
        )?;
        let url = url;
        let path = format!(
            "{}/{}:batchEmbedContents",
            url.path().trim_end_matches('/'),
            if model == "." || model == ".." {
                model.to_owned()
            } else {
                base::escape_url_path_segment(model)
            }
        );
        Ok(base::ProviderEndpoint::with_path(url, path))
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
        self.create_embeddings_with_context(&ProviderContext::new(cancel), model, texts, opts)
            .map_err(|error| error.to_string())
    }
    fn create_embeddings_with_context(
        &self,
        context: &ProviderContext<'_>,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, ProviderError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if model.is_empty() {
            return Err("model name is required".into());
        }
        let key = self
            .cfg
            .resolve_api_key("API key is not configured for Gemini")?;
        let endpoint = self.endpoint(model)?;
        let requests = texts
            .iter()
            .map(|text| {
                let fields = Options::from([
                    ("model".into(), Value::String(format!("models/{model}"))),
                    (
                        "content".into(),
                        serde_json::json!({"parts":[{"text":text}]}),
                    ),
                ]);
                base::json_fields_with_options(fields, opts)
            })
            .collect::<Vec<_>>();
        let payload = serde_json::json!({"requests":requests});
        base::execute_json_embedding_call(
            context,
            &self.client,
            "Gemini",
            endpoint,
            &payload,
            base::provider_auth_headers(context, "Gemini", &key, true)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| {
                base::ensure_json_object(&value["error"])?;
                base::string_field(&value["error"]["status"])?;
                if !value["error"]["code"].is_null() && value["error"]["code"].as_i64().is_none() {
                    return Err("unexpected unmarshal integer field error".into());
                }
                base::string_field(&value["error"]["message"])
            }),
            |status| match status {
                401 => Some(self.cfg.unauthorized_error("Gemini", status)),
                _ => None,
            },
            Some(decode_embeddings),
        )
    }
}
fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    base::ensure_json_object(&response)?;
    let empty = Vec::new();
    let items = if response["embeddings"].is_null() {
        &empty
    } else {
        response["embeddings"]
            .as_array()
            .ok_or("unexpected unmarshal response error: embeddings must be an array")?
    };
    if items.len() != expected {
        return Err(format!(
            "response embeddings length {} does not match input texts length {expected}",
            items.len()
        ));
    }
    items
        .iter()
        .map(|item| {
            base::ensure_json_object(item)?;
            base::decode_float_row(&item["values"])
        })
        .collect()
}
