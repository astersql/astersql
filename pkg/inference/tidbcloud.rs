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

use crate::base::{self, ProviderContext, ProviderError};
use crate::{Embedder, Options};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Default)]
pub struct TiDBCloudConfig {
    pub billing_id: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub api_key: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub base_url: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub max_response_bytes: i64,
}
pub struct TiDBCloudFreeEmbedder {
    pub(crate) client: reqwest::Client,
    cfg: TiDBCloudConfig,
}
impl TiDBCloudFreeEmbedder {
    pub fn new(
        billing_id: impl Fn() -> String + Send + Sync + 'static,
        api_key: impl Fn() -> String + Send + Sync + 'static,
        base_url: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self::with_config(TiDBCloudConfig {
            billing_id: Some(Arc::new(billing_id)),
            api_key: Some(Arc::new(api_key)),
            base_url: Some(Arc::new(base_url)),
            ..Default::default()
        })
    }
    pub fn with_config(mut cfg: TiDBCloudConfig) -> Self {
        if cfg.max_response_bytes <= 0 {
            cfg.max_response_bytes = base::DEFAULT_MAX_RESPONSE_BYTES;
        }
        Self {
            client: base::http_client("TiDB Cloud Inference"),
            cfg,
        }
    }
    pub(crate) fn endpoint(&self) -> Result<base::ProviderEndpoint, ProviderError> {
        let configured = self
            .cfg
            .base_url
            .as_ref()
            .map(|getter| getter())
            .unwrap_or_default();
        if configured.is_empty() {
            return Err("base URL is not configured for TiDB Cloud Inference".into());
        }
        let url = base::parse_http_url(&configured, "TiDB Cloud Inference base URL")?;
        let billing = self
            .cfg
            .billing_id
            .as_ref()
            .map(|getter| getter())
            .unwrap_or_default();
        let billing = if billing.is_empty() {
            "default_billing_id"
        } else {
            &billing
        };
        let path = format!(
            "{}/api/v1/inference/embeddings/{}",
            url.path().trim_end_matches('/'),
            base::escape_url_path_segment(billing)
        );
        Ok(base::ProviderEndpoint::with_path(url, path))
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
            .api_key
            .as_ref()
            .map(|getter| getter())
            .unwrap_or_default();
        let fields = Options::from([
            ("model".into(), Value::String(model.into())),
            ("texts".into(), serde_json::json!(texts)),
        ]);
        let payload = serde_json::to_value(base::json_fields_with_options(fields, opts))
            .expect("JSON fields");
        base::execute_json_embedding_call(
            context,
            &self.client,
            "TiDB Cloud Inference",
            self.endpoint()?,
            &payload,
            base::provider_auth_headers(context, "TiDB Cloud Inference", &key, false)?,
            self.cfg.max_response_bytes,
            &[&key],
            texts.len(),
            Some(|value: &serde_json::Value| base::string_field(&value["error"])),
            |_| None,
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
        .enumerate()
        .map(|(index, item)| {
            let wrapped = serde_json::json!({"data":[{"index":0,"embedding":item}]});
            crate::openai::decode_indexed_base64_embeddings(wrapped.to_string().as_bytes(), 1)
                .map(|mut values| values.remove(0))
                .map_err(|error| error.replace("for index 0:", &format!("for index {index}:")))
        })
        .collect()
}
