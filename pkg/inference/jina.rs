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

const DEFAULT_BASE_URL: &str = "https://api.jina.ai/v1/embeddings";
pub struct JinaEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: APIKeyProviderConfig,
}
impl JinaEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(APIKeyProviderConfig {
            api_key: Some(Arc::new(api_key)), base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("JinaAI API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_JINA_AI_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("JinaAI returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_JINA_AI_API_KEY='<API_KEY>'".into()),
            ..Default::default()
        })
    }
    pub fn with_config(cfg: APIKeyProviderConfig) -> Self {
        Self {
            client: base::http_client("JinaAI"),
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
            "Jina AI API base URL",
        )?;
        let _ = model;
        Ok(url)
    }
}
impl Embedder for JinaEmbedder {
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
        if opts.get("return_multivector") == Some(&Value::Bool(true)) {
            return Err("JinaAI option return_multivector=true is not supported".into());
        }
        let key = self
            .cfg
            .resolve_api_key("API key is not configured for JinaAI")?;
        let endpoint = self.endpoint(model)?;
        let fields: Options = serde_json::from_value(
            serde_json::json!({"model":model,"input":texts,"embedding_type":"base64"}),
        )
        .expect("request fields are an object");
        let payload = serde_json::to_value(base::json_fields_with_options(fields, opts))
            .expect("JSON fields");
        base::execute_json_embedding_call(
            context,
            &self.client,
            "JinaAI",
            endpoint,
            &payload,
            base::provider_auth_headers(context, "JinaAI", &key, false)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| base::string_field(&value["detail"])),
            |status| match status {
                401 => Some(self.cfg.unauthorized_error("JinaAI", status)),
                _ => None,
            },
            Some(decode_embeddings),
        )
    }
}
fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    crate::openai::decode_indexed_base64_embeddings(body, expected)
}
