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

use crate::Options;
use regex::Regex;
use std::sync::LazyLock;

pub fn decode_float32_array_bytes(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.is_empty() {
        return Err("embedding data is empty".into());
    }
    if bytes.len() % 4 != 0 {
        return Err("invalid embedding data".into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

pub fn json_fields_with_options(fields: Options, opts: &Options) -> Options {
    let mut merged = opts.clone();
    merged.extend(fields);
    merged
}

pub fn sanitize_error_text(text: &str, secrets: &[&str]) -> String {
    static JSON: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)("(?:authorization|api[_-]?key|token|access[_-]?token|credentials)"\s*:\s*")([^"]*)(")"#).unwrap()
    });
    static BEARER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)Bearer\s+[A-Za-z0-9._~+/=-]+").unwrap());
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}\b").unwrap());
    let mut value = text.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            value = value.replace(secret, "[REDACTED]");
        }
    }
    value = JSON.replace_all(&value, "${1}[REDACTED]${3}").into_owned();
    value = BEARER.replace_all(&value, "Bearer [REDACTED]").into_owned();
    value = KEY.replace_all(&value, "[REDACTED]").into_owned();
    if value.len() > 4096 {
        let mut end = 4096;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push_str("...[truncated]");
    }
    value
}

pub const DEFAULT_MAX_RESPONSE_BYTES: i64 = 32 * 1024 * 1024;
pub const DEFAULT_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A safe display message with an independently inspectable transport or caller cause.
#[derive(Clone, Debug)]
pub struct ProviderError {
    message: String,
    cause: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>,
}

impl ProviderError {
    pub fn redacted(
        message: impl Into<String>,
        cause: std::sync::Arc<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self {
            message: message.into(),
            cause: Some(cause),
        }
    }
    pub fn has_cause(&self, cause: &std::sync::Arc<dyn std::error::Error + Send + Sync>) -> bool {
        self.cause
            .as_ref()
            .is_some_and(|stored| std::sync::Arc::ptr_eq(stored, cause))
    }
}
impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}
impl From<String> for ProviderError {
    fn from(message: String) -> Self {
        Self {
            message,
            cause: None,
        }
    }
}
impl From<&str> for ProviderError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// The provider observes both cancellation and its caller's original cause.
/// Existing AtomicBool callers continue using the legacy trait method.
pub struct ProviderContext<'a> {
    pub cancel: &'a std::sync::atomic::AtomicBool,
    pub cancellation_cause: Option<&'a (dyn Fn() -> Option<ProviderError> + Send + Sync)>,
}
impl<'a> ProviderContext<'a> {
    pub fn new(cancel: &'a std::sync::atomic::AtomicBool) -> Self {
        Self {
            cancel,
            cancellation_cause: None,
        }
    }
    pub fn cause(&self) -> Option<ProviderError> {
        self.cancellation_cause
            .and_then(|cause| cause())
            .or_else(|| {
                self.cancel
                    .load(std::sync::atomic::Ordering::Acquire)
                    .then(|| "request canceled".into())
            })
    }
}

#[derive(Clone, Default)]
pub struct APIKeyProviderConfig {
    pub api_key: Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>>,
    pub base_url: Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>>,
    pub missing_key_error: Option<ProviderError>,
    pub unauthorized_error: Option<ProviderError>,
    pub max_response_bytes: i64,
}
impl APIKeyProviderConfig {
    pub fn with_defaults(mut self) -> Self {
        if self.max_response_bytes <= 0 {
            self.max_response_bytes = DEFAULT_MAX_RESPONSE_BYTES;
        }
        self
    }
    pub fn resolve_api_key(&self, fallback: &str) -> Result<String, ProviderError> {
        self.resolve_api_key_error(Some(fallback.into()))
    }
    pub fn resolve_api_key_error(
        &self,
        fallback: Option<ProviderError>,
    ) -> Result<String, ProviderError> {
        let key = self
            .api_key
            .as_ref()
            .map(|getter| getter())
            .unwrap_or_default();
        if !key.is_empty() {
            return Ok(key);
        }
        Err(self
            .missing_key_error
            .clone()
            .or(fallback)
            .unwrap_or_else(|| "API key is not configured".into()))
    }
    pub fn configured_base_url(&self) -> String {
        self.base_url
            .as_ref()
            .map(|getter| getter())
            .unwrap_or_default()
    }
    pub fn unauthorized_error(&self, provider: &str, status: u16) -> ProviderError {
        self.unauthorized_error.clone().unwrap_or_else(|| {
            format!(
                "{provider} returns status {}, check API key",
                reqwest::StatusCode::from_u16(status)
                    .ok()
                    .and_then(|code| code.canonical_reason())
                    .unwrap_or("")
                    .to_lowercase()
            )
            .into()
        })
    }
}

pub fn read_response_body(
    reader: impl std::io::Read,
    max_bytes: i64,
) -> Result<Vec<u8>, ProviderError> {
    use std::io::Read;
    if max_bytes < 0 {
        return Err("maximum response body size must not be negative".into());
    }
    let mut body = Vec::new();
    reader
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| ProviderError::redacted(error.to_string(), std::sync::Arc::new(error)))?;
    if body.len() as u64 > max_bytes as u64 {
        return Err(format!("response body exceeds maximum size of {max_bytes} bytes").into());
    }
    Ok(body)
}

pub fn parse_http_url(raw: &str, description: &str) -> Result<reqwest::Url, ProviderError> {
    // WHATWG URLs otherwise accept malformed percent escapes that Go url.Parse rejects.
    let bytes = raw.trim().as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit())
        {
            return Err(format!("invalid {description}").into());
        }
    }
    if !raw.trim().contains(':') {
        return Err(format!("invalid {description}: absolute HTTP(S) URL is required").into());
    }
    let url = reqwest::Url::parse(raw.trim()).map_err(|error| {
        ProviderError::redacted(format!("invalid {description}"), std::sync::Arc::new(error))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(format!("invalid {description}: absolute HTTP(S) URL is required").into());
    }
    Ok(url)
}

/// Match Go url.PathEscape, including complete dot segments.
pub fn escape_url_path_segment(segment: &str) -> String {
    if segment == "." {
        return "%2E".into();
    }
    if segment == ".." {
        return "%2E%2E".into();
    }
    let mut escaped = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~$&+:=@".contains(&byte) {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

/// Retain the original escaped path independently of WHATWG normalization.
pub(crate) struct ProviderEndpoint {
    pub(crate) url: reqwest::Url,
    escaped_path: Option<String>,
}
impl From<reqwest::Url> for ProviderEndpoint {
    fn from(url: reqwest::Url) -> Self {
        Self {
            url,
            escaped_path: None,
        }
    }
}
impl ProviderEndpoint {
    pub(crate) fn with_path(mut url: reqwest::Url, escaped_path: String) -> Self {
        url.set_path(&escaped_path);
        Self {
            url,
            escaped_path: Some(escaped_path),
        }
    }
    pub(crate) fn raw_url(&self) -> Option<String> {
        let path = self.escaped_path.as_ref()?;
        if path == self.url.path() {
            return None;
        }
        let mut endpoint = format!(
            "{}{path}",
            &self.url.as_str()[..self.url.as_str().len()
                - self.url.path().len()
                - self.url.query().map_or(0, |query| query.len() + 1)
                - self.url.fragment().map_or(0, |fragment| fragment.len() + 1)]
        );
        if let Some(query) = self.url.query() {
            endpoint.push('?');
            endpoint.push_str(query);
        }
        Some(endpoint)
    }
}

pub(crate) fn http_client(provider: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_HTTP_TIMEOUT)
        .build()
        .unwrap_or_else(|error| panic!("construct {provider} embedding HTTP client: {error}"))
}

/// JSON POST lifecycle shared by the adapters: bounded read, cancellation,
/// exact 200 success, sanitized logging, then provider-specific status overrides.
pub(crate) fn execute_json_embedding_call<E: Into<ProviderError>>(
    context: &ProviderContext<'_>,
    client: &reqwest::Client,
    provider: &str,
    endpoint: impl Into<ProviderEndpoint>,
    payload: &serde_json::Value,
    headers: reqwest::header::HeaderMap,
    max_bytes: i64,
    secrets: &[&str],
    expected: usize,
    decode_message: Option<impl Fn(&serde_json::Value) -> Result<String, String>>,
    status_error: impl Fn(u16) -> Option<ProviderError>,
    decode: Option<impl Fn(&[u8], usize) -> Result<Vec<Vec<f32>>, E>>,
) -> Result<Vec<Vec<f32>>, ProviderError> {
    use logutil::log::{BgLogger, LogField, LogLevel};
    let decode_message = decode_message.ok_or_else(|| {
        ProviderError::from(format!(
            "{provider} error response decoder is not configured"
        ))
    })?;
    let decode = decode.ok_or_else(|| {
        ProviderError::from(format!(
            "{provider} success response decoder is not configured"
        ))
    })?;
    let endpoint = endpoint.into();
    use std::future::Future;
    if let Some(cause) = context.cause() {
        return Err(cause);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            ProviderError::redacted(
                format!("{provider} request failed"),
                std::sync::Arc::new(error),
            )
        })?;
    let (status, body) = runtime.block_on(async {
        let request = async {
            if let Some(raw_url) = endpoint.raw_url() {
                return crate::raw_http::post_json(&raw_url, payload, headers, max_bytes, provider)
                    .await
                    .map_err(|error| context.cause().unwrap_or(error));
            }
            let mut headers = headers;
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            );
            let mut response = client
                .post(endpoint.url)
                .headers(headers)
                .json(payload)
                .send()
                .await
                .map_err(|error| {
                    context.cause().unwrap_or_else(|| {
                        ProviderError::redacted(
                            format!("{provider} request failed"),
                            std::sync::Arc::new(error.without_url()),
                        )
                    })
                })?;
            let status = response.status();
            if max_bytes < 0 {
                return Err("maximum response body size must not be negative".into());
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                let error = error.without_url();
                ProviderError::redacted(error.to_string(), std::sync::Arc::new(error))
            })? {
                let remaining =
                    (max_bytes.saturating_add(1) as u64).saturating_sub(body.len() as u64) as usize;
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if body.len() as u64 > max_bytes as u64 {
                    return Err(
                        format!("response body exceeds maximum size of {max_bytes} bytes").into(),
                    );
                }
            }
            Ok((status, body))
        };
        let cancelled = async {
            loop {
                if let Some(cause) = context.cause() {
                    return cause;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        };
        let mut request = std::pin::pin!(request);
        let mut cancelled = std::pin::pin!(cancelled);
        std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(cause) = cancelled.as_mut().poll(cx) {
                return std::task::Poll::Ready(Err(cause));
            }
            request.as_mut().poll(cx)
        })
        .await
    })?;
    if status.as_u16() == 200 {
        return decode(&body, expected).map_err(Into::into);
    }
    let parsed =
        serde_json::from_slice::<serde_json::Value>(&body).map_err(|error| error.to_string());
    let message = parsed.and_then(|value| decode_message(&value));
    let mut fields = vec![LogField::I64("status".into(), status.as_u16() as i64)];
    let message = match message {
        Ok(message) => sanitize_error_text(&message, secrets),
        Err(error) => {
            fields.push(LogField::String(
                "parse_error".into(),
                sanitize_error_text(&error, secrets),
            ));
            String::new()
        }
    };
    if !message.is_empty() {
        fields.push(LogField::String("message".into(), message.clone()));
    }
    BgLogger().log(
        LogLevel::Error,
        format!("{provider} API request failed"),
        fields,
    );
    if let Some(error) = status_error(status.as_u16()) {
        return Err(error);
    }
    let message = if message.is_empty() {
        go_http_status_text(status.as_u16())
    } else {
        &message
    };
    Err(format!(
        "{provider}: status code {}, message: {message}",
        status.as_u16()
    )
    .into())
}

pub(crate) fn string_field(value: &serde_json::Value) -> Result<String, String> {
    if value.is_null() {
        return Ok(String::new());
    }
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "unexpected unmarshal string field error".into())
}
pub(crate) fn auth_headers(
    key: &str,
    google: bool,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    let mut headers = reqwest::header::HeaderMap::new();
    if !key.is_empty() {
        let (name, value) = if google {
            ("x-goog-api-key", key.to_owned())
        } else {
            ("authorization", format!("Bearer {key}"))
        };
        headers.insert(
            reqwest::header::HeaderName::from_static(name),
            reqwest::header::HeaderValue::from_str(&value).map_err(|error| {
                ProviderError::redacted(
                    "invalid provider authentication header",
                    std::sync::Arc::new(error),
                )
            })?,
        );
    }
    Ok(headers)
}

pub(crate) fn decode_float_row(value: &serde_json::Value) -> Result<Vec<f32>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or("unexpected unmarshal response error: embedding must be an array")?
        .iter()
        .map(|value| {
            if value.is_null() {
                return Ok(0.0);
            }
            let number = value
                .as_f64()
                .ok_or("unexpected unmarshal response error: embedding value must be numeric")?;
            if !(number as f32).is_finite() {
                return Err("unexpected unmarshal response error: float32 overflow".into());
            }
            Ok(number as f32)
        })
        .collect()
}
pub(crate) fn decode_float_rows(value: &serde_json::Value) -> Result<Vec<Vec<f32>>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or("unexpected unmarshal response error: embeddings must be an array")?
        .iter()
        .map(decode_float_row)
        .collect()
}

pub(crate) fn ensure_json_object(value: &serde_json::Value) -> Result<(), String> {
    if value.is_null() || value.is_object() {
        Ok(())
    } else {
        Err("unexpected unmarshal response error: expected an object".into())
    }
}

pub(crate) fn provider_auth_headers(
    context: &ProviderContext<'_>,
    provider: &str,
    key: &str,
    google: bool,
) -> Result<reqwest::header::HeaderMap, ProviderError> {
    auth_headers(key, google).map_err(|error| {
        context.cause().unwrap_or_else(|| {
            let cause = error
                .cause
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(error));
            ProviderError::redacted(format!("{provider} request failed"), cause)
        })
    })
}

pub(crate) fn go_http_status_text(status: u16) -> &'static str {
    match status {
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        416 => "Requested Range Not Satisfiable",
        _ => reqwest::StatusCode::from_u16(status)
            .ok()
            .and_then(|code| code.canonical_reason())
            .unwrap_or(""),
    }
}
