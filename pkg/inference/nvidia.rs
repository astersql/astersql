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

const DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com/v1/embeddings";
pub struct NvidiaEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: APIKeyProviderConfig,
}
impl NvidiaEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(APIKeyProviderConfig {
            api_key: Some(Arc::new(api_key)), base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("NVIDIA NIM API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_NVIDIA_NIM_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("NVIDIA NIM returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_NVIDIA_NIM_API_KEY='<API_KEY>'".into()),
            ..Default::default()
        })
    }
    pub fn with_config(cfg: APIKeyProviderConfig) -> Self {
        Self {
            client: base::http_client("NVIDIA NIM"),
            cfg: cfg.with_defaults(),
        }
    }
    pub(crate) fn endpoint(&self, model: &str) -> Result<reqwest::Url, ProviderError> {
        let configured = self.cfg.configured_base_url();
        let configured = configured.trim();
        let url = base::parse_http_url(
            if configured.is_empty() {
                DEFAULT_BASE_URL
            } else {
                configured
            },
            "NVIDIA NIM API base URL",
        )?;
        let _ = model;
        Ok(url)
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
        if let Some(kind) = opts.get("embedding_type")
            && kind != "float"
        {
            return Err(r#"NVIDIA NIM embedding_type must be "float""#.into());
        }
        let key = self
            .cfg
            .resolve_api_key("API key is not configured for NVIDIA NIM")?;
        let endpoint = self.endpoint(model)?;
        let fields: Options = serde_json::from_value(
            serde_json::json!({"model":model,"input":texts,"encoding_format":"base64"}),
        )
        .expect("request fields are an object");
        let payload = serde_json::to_value(base::json_fields_with_options(fields, opts))
            .expect("JSON fields");
        base::execute_json_embedding_call(
            context,
            &self.client,
            "NVIDIA NIM",
            endpoint,
            &payload,
            base::provider_auth_headers(context, "NVIDIA NIM", &key, false)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| {
                let detail = base::string_field(&value["detail"])?;
                let message = base::string_field(&value["message"])?;
                let error = base::string_field(&value["error"])?;
                Ok(if !detail.is_empty() {
                    detail
                } else if !message.is_empty() {
                    message
                } else {
                    error
                })
            }),
            |status| match status {
                401 | 403 => Some(self.cfg.unauthorized_error("NVIDIA NIM", status)),
                404 => Some(
                    format!("NVIDIA NIM model '{model}' does not exist or is not available").into(),
                ),
                _ => None,
            },
            Some(decode_embeddings),
        )
    }
}
fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    base::ensure_json_object(&value)?;
    base::string_field(&value["object"])?;
    base::ensure_json_object(&value["usage"])?;
    for key in ["prompt_tokens", "total_tokens"] {
        if !value["usage"][key].is_null() && value["usage"][key].as_i64().is_none() {
            return Err("unexpected unmarshal integer field error".into());
        }
    }
    crate::openai::decode_indexed_base64_embeddings(body, expected)
}
