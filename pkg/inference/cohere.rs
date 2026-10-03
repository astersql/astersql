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

const DEFAULT_BASE_URL: &str = "https://api.cohere.com/v1/embed";
pub struct CohereEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: APIKeyProviderConfig,
}
impl CohereEmbedder {
    pub fn new(
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(APIKeyProviderConfig {
            api_key: Some(Arc::new(api_key)), base_url: Some(Arc::new(base_url)),
            missing_key_error: Some("Cohere API key is not configured, to configure the API key: SET @@GLOBAL.TIDB_EXP_EMBED_COHERE_API_KEY='<API_KEY>'".into()),
            unauthorized_error: Some("Cohere returns status unauthorized, check your API key. To reconfigure a new API key: SET @@GLOBAL.TIDB_EXP_EMBED_COHERE_API_KEY='<API_KEY>'".into()),
            ..Default::default()
        })
    }
    pub fn with_config(cfg: APIKeyProviderConfig) -> Self {
        Self {
            client: base::http_client("Cohere"),
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
            "Cohere API base URL",
        )?;
        let _ = model;
        Ok(url)
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
        if let Some(types) = opts.get("embedding_types")
            && types != &serde_json::json!(["float"])
        {
            return Err(r#"Cohere embedding_types must be exactly ["float"]"#.into());
        }
        let key = self
            .cfg
            .resolve_api_key("API key is not configured for cohere")?;
        let endpoint = self.endpoint(model)?;
        let fields: Options =
            serde_json::from_value(serde_json::json!({"model":model,"texts":texts}))
                .expect("request fields are an object");
        let payload = serde_json::to_value(base::json_fields_with_options(fields, opts))
            .expect("JSON fields");
        base::execute_json_embedding_call(
            context,
            &self.client,
            "Cohere",
            endpoint,
            &payload,
            base::provider_auth_headers(context, "Cohere", &key, false)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| base::string_field(&value["message"])),
            |status| match status {
                401 => Some(self.cfg.unauthorized_error("cohere", status)),
                _ => None,
            },
            Some(decode_embeddings),
        )
    }
}
fn decode_embeddings(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected unmarshal response error: {error}"))?;
    let raw = response
        .get("embeddings")
        .ok_or("Cohere response does not contain embeddings")?;
    let raw = if raw.is_array() {
        raw
    } else if raw.is_object() {
        raw.get("float")
            .filter(|value| !value.is_null())
            .ok_or("Cohere response does not contain float embeddings")?
    } else {
        return Err("unexpected Cohere embeddings response format".into());
    };
    let embeddings = base::decode_float_rows(raw)?;
    if embeddings.len() != expected {
        return Err(format!(
            "response embeddings length {} does not match input texts length {expected}",
            embeddings.len()
        ));
    }
    Ok(embeddings)
}
