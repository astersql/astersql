// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{RegisterSysVar, SysVar, VariableError, VariableErrorKind, vardef};
use std::sync::Arc;

pub const EMBEDDING_API_KEYS: [&str; 6] = [
    "tidb_exp_embed_jina_ai_api_key",
    "tidb_exp_embed_openai_api_key",
    "tidb_exp_embed_cohere_api_key",
    "tidb_exp_embed_huggingface_api_key",
    "tidb_exp_embed_nvidia_nim_api_key",
    "tidb_exp_embed_gemini_api_key",
];
pub const EMBEDDING_API_BASE: &str = "tidb_exp_embed_openai_api_base";
pub const DEFAULT_EMBEDDING_API_BASE: &str = "https://api.openai.com/v1";
pub const OpenAIEndpointWhitelistErrMsg: &str = "For security reasons currently only OpenAI, Azure OpenAI, or Alibaba Cloud DashScope Endpoint is allowed";
fn config_holder(name: &str) -> &vardef::AtomicStringValue {
    match name {
        "tidb_exp_embed_jina_ai_api_key" => &vardef::EmbedJinaAPIKey,
        "tidb_exp_embed_openai_api_key" => &vardef::EmbedOpenAIAPIKey,
        "tidb_exp_embed_openai_api_base" => &vardef::EmbedOpenAIAPIBase,
        "tidb_exp_embed_cohere_api_key" => &vardef::EmbedCohereAPIKey,
        "tidb_exp_embed_huggingface_api_key" => &vardef::EmbedHuggingFaceAPIKey,
        "tidb_exp_embed_nvidia_nim_api_key" => &vardef::EmbedNvidiaNIMAPIKey,
        "tidb_exp_embed_gemini_api_key" => &vardef::EmbedGeminiAPIKey,
        _ => panic!("unknown embedding variable"),
    }
}
pub fn embedding_config_version() -> u64 {
    vardef::EmbeddingConfigVersion.Load()
}
pub fn embedding_api_key(name: &str) -> String {
    config_holder(name).Load()
}
pub fn GetOpenAIEmbeddingBaseURL() -> String {
    resolve_base(&embedding_api_key(EMBEDDING_API_BASE)).to_owned()
}
fn resolve_base(value: &str) -> &str {
    if value.is_empty() {
        DEFAULT_EMBEDDING_API_BASE
    } else {
        value
    }
}
pub fn is_embedding_api_key(name: &str) -> bool {
    EMBEDDING_API_KEYS
        .iter()
        .any(|key| key.eq_ignore_ascii_case(name))
}
pub fn mask_embedding_api_key(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else if value.len() <= 6 {
        "******".into()
    } else {
        format!(
            "******{}",
            String::from_utf8_lossy(&value.as_bytes()[value.len() - 4..])
        )
    }
}
pub fn NormalizeOpenAIEmbeddingAPIBase(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let invalid = |reason: &str| format!("invalid value for {EMBEDDING_API_BASE}: {reason}");
    let url = url::Url::parse(trimmed).map_err(|error| {
        invalid(if error == url::ParseError::RelativeUrlWithoutBase {
            "absolute https URL is required"
        } else {
            "invalid URL"
        })
    })?;
    if url.host_str().is_none() {
        return Err(invalid("absolute https URL is required"));
    }
    if url.scheme() != "https" {
        return Err(invalid("only https scheme is supported"));
    }
    if url.query().is_some_and(|query| !query.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return Err(invalid("query parameters and fragments are not allowed"));
    }
    let host = url.host_str().unwrap().to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "api.openai.com"
            | "dashscope.aliyuncs.com"
            | "dashscope-intl.aliyuncs.com"
            | "dashscope-us.aliyuncs.com"
    ) && !host.ends_with(".openai.azure.com")
    {
        return Err(OpenAIEndpointWhitelistErrMsg.into());
    }
    // Preserve the original host spelling and explicit port like Go's URL.Host;
    // URL normalization intentionally drops userinfo and query/fragment.
    let authority = trimmed
        .split_once("://")
        .unwrap()
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap()
        .rsplit('@')
        .next()
        .unwrap();
    // Go URL.Path decodes escapes without cleaning dot segments.
    let raw_path = trimmed.split_once("://").unwrap().1;
    let raw_path = raw_path
        .find('/')
        .map(|start| &raw_path[start..])
        .unwrap_or("");
    let raw_path = raw_path.split(['?', '#']).next().unwrap();
    let mut bytes = Vec::with_capacity(raw_path.len());
    let mut input = raw_path.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input.next().and_then(|b| (b as char).to_digit(16));
            let low = input.next().and_then(|b| (b as char).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(invalid("invalid URL escape"));
            };
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| invalid("invalid URL path"))?;
    let decoded_path = decoded.strip_suffix('/').unwrap_or(&decoded);
    let path = decoded_path
        .strip_suffix("/embeddings")
        .unwrap_or(decoded_path);
    Ok(format!("https://{authority}{path}"))
}
fn set_config(name: &str, value: &str) {
    let old = config_holder(name).Swap(value);
    let changed = if name == EMBEDDING_API_BASE {
        resolve_base(&old) != resolve_base(value)
    } else {
        old != value
    };
    if changed {
        vardef::EmbeddingConfigVersion.Inc();
    }
}
pub(crate) fn register_embedding_vars() {
    for name in EMBEDDING_API_KEYS.into_iter().chain([EMBEDDING_API_BASE]) {
        let mut var = SysVar {
            Name: name.into(),
            Scope: vardef::ScopeGlobal,
            Value: if name == EMBEDDING_API_BASE {
                DEFAULT_EMBEDDING_API_BASE.into()
            } else {
                String::new()
            },
            Type: vardef::TypeStr,
            AllowEmptyAll: true,
            ..SysVar::default()
        };
        var.SetGlobal = Some(Arc::new(move |_, _, value| {
            let value = if name == EMBEDDING_API_BASE {
                NormalizeOpenAIEmbeddingAPIBase(value)
                    .map_err(|error| VariableError::new(VariableErrorKind::WrongValue, error))?
            } else {
                value.into()
            };
            set_config(name, &value);
            Ok(())
        }));
        var.GetGlobal = Some(Arc::new(move |_, _| {
            Ok(if name == EMBEDDING_API_BASE {
                GetOpenAIEmbeddingBaseURL()
            } else {
                mask_embedding_api_key(&embedding_api_key(name))
            })
        }));
        RegisterSysVar(var);
    }
}
