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

const DEFAULT_BASE_URL: &str = "https://router.huggingface.co/hf-inference";
pub struct HuggingFaceEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: APIKeyProviderConfig,
}
impl HuggingFaceEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(APIKeyProviderConfig {
            api_key: Some(Arc::new(api_key)), base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("HuggingFace API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_HUGGINGFACE_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("HuggingFace returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_HUGGINGFACE_API_KEY='<API_KEY>'".into()),
            ..Default::default()
        })
    }
    pub fn with_config(cfg: APIKeyProviderConfig) -> Self {
        Self {
            client: base::http_client("HuggingFace"),
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
            "HuggingFace API base URL",
        )?;
        let url = url;
        let model = model
            .split('/')
            .map(base::escape_url_path_segment)
            .collect::<Vec<_>>()
            .join("/");
        let path = format!(
            "{}/models/{model}/pipeline/feature-extraction",
            url.path().trim_end_matches('/')
        );
        Ok(base::ProviderEndpoint::with_path(url, path))
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
            .resolve_api_key("API key is not configured for HuggingFace")?;
        let endpoint = self.endpoint(model)?;
        let fields: Options = serde_json::from_value(serde_json::json!({"inputs":texts}))
            .expect("request fields are an object");
        let payload = serde_json::to_value(base::json_fields_with_options(fields, opts))
            .expect("JSON fields");
        base::execute_json_embedding_call(
            context,
            &self.client,
            "HuggingFace",
            endpoint,
            &payload,
            base::provider_auth_headers(context, "HuggingFace", &key, false)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| base::string_field(&value["error"])),
            |status| match status {
                401 => Some(self.cfg.unauthorized_error("HuggingFace", status)),
                404 => Some(
                    format!("HuggingFace model '{model}' does not exist or is not available")
                        .into(),
                ),
                _ => None,
            },
            Some(decode_embeddings),
        )
    }
}
fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    let embeddings = base::decode_float_rows(&response)?;
    if embeddings.len() != expected {
        return Err(format!(
            "response data length {} does not match input texts length {expected}",
            embeddings.len()
        ));
    }
    Ok(embeddings)
}
