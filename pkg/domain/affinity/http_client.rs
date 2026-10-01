// Copyright 2026 AsterSQL.
//! PD HTTP affinity API transport; manager retains compatibility fallback.
use crate::{AffinityError, AffinityGroupKeyRange, AffinityGroupState, Context, PdClient};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
const API: &str = "/pd/api/v2/affinity-groups";
pub struct HttpClient {
    client: reqwest::blocking::Client,
    endpoints: Vec<String>,
}
impl HttpClient {
    pub fn new(
        endpoints: Vec<String>,
        tls: Option<(&[u8], &[u8], &[u8])>,
    ) -> Result<Self, AffinityError> {
        if endpoints.is_empty() {
            return Err(AffinityError::new("PD endpoints are empty"));
        }
        let mut builder = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30));
        if let Some((ca, cert, key)) = tls {
            builder =
                builder.add_root_certificate(reqwest::Certificate::from_pem(ca).map_err(err)?);
            if !cert.is_empty() || !key.is_empty() {
                let mut identity = cert.to_vec();
                identity.extend_from_slice(key);
                builder = builder.identity(reqwest::Identity::from_pem(&identity).map_err(err)?);
            }
        }
        let scheme = if tls.is_some() { "https" } else { "http" };
        let endpoints = endpoints
            .into_iter()
            .map(|e| {
                if e.contains("://") {
                    e.trim_end_matches('/').to_owned()
                } else {
                    format!("{scheme}://{}", e.trim_end_matches('/'))
                }
            })
            .collect();
        Ok(Self {
            client: builder.build().map_err(err)?,
            endpoints,
        })
    }
    pub fn request_json(
        &self,
        ctx: &dyn Context,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, AffinityError> {
        let mut last = AffinityError::new("PD request failed");
        for endpoint in &self.endpoints {
            if ctx.is_cancelled() {
                return Err(AffinityError::new("context cancelled"));
            }
            let mut request = self
                .client
                .request(method.clone(), format!("{endpoint}{path}"));
            if let Some(deadline) = ctx.deadline() {
                let timeout = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or_else(|| AffinityError::new("context deadline exceeded"))?;
                request = request.timeout(timeout);
            }
            if let Some(v) = &body {
                request = request.json(v);
            }
            match request.send() {
                Ok(response) => {
                    let status = response.status();
                    let bytes = response.bytes().map_err(err)?;
                    if !status.is_success() {
                        return Err(AffinityError::http_status(
                            status.as_u16(),
                            String::from_utf8_lossy(&bytes),
                        ));
                    }
                    return if bytes.is_empty() {
                        Ok(Value::Null)
                    } else {
                        Ok(serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                            Value::String(String::from_utf8_lossy(&bytes).into_owned())
                        }))
                    };
                }
                Err(e) => last = AffinityError::http_service(e.to_string()),
            }
        }
        Err(last)
    }
    fn states(value: Value) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let Some(groups) = value.get("affinity_groups").and_then(Value::as_object) else {
            return Err(AffinityError::new(
                "PD affinity response has no affinity_groups",
            ));
        };
        groups
            .iter()
            .map(|(id, v)| {
                let count = v
                    .get("range_count")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| AffinityError::new("PD affinity response has no range_count"))?;
                Ok((
                    id.clone(),
                    AffinityGroupState::new(
                        v.get("id").and_then(Value::as_str).unwrap_or(id),
                        count as usize,
                    ),
                ))
            })
            .collect()
    }
}
fn err(e: impl ToString) -> AffinityError {
    AffinityError::new(e.to_string())
}
impl PdClient for HttpClient {
    fn create_affinity_groups(
        &self,
        ctx: &dyn Context,
        groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
        skip: bool,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let input: HashMap<_, _> = groups
            .iter()
            .map(|(id, ranges)| {
                let ranges: Vec<_> = ranges
                    .iter()
                    .map(|r| {
                        json!({
                            "start_key": STANDARD.encode(&r.start_key),
                            "end_key": STANDARD.encode(&r.end_key),
                        })
                    })
                    .collect();
                (id, json!({"ranges": ranges}))
            })
            .collect();
        Self::states(self.request_json(
            ctx,
            reqwest::Method::POST,
            &format!("{API}{}", if skip { "?skip_exist_check=true" } else { "" }),
            Some(json!({"affinity_groups":input})),
        )?)
    }
    fn batch_delete_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
        force: bool,
    ) -> Result<(), AffinityError> {
        self.request_json(
            ctx,
            reqwest::Method::POST,
            &format!("{API}?delete"),
            Some(json!({"ids":ids,"force":force})),
        )?;
        Ok(())
    }
    fn get_affinity_groups(
        &self,
        ctx: &dyn Context,
        ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(ids.iter().map(|id| ("ids", id)))
            .finish();
        Self::states(self.request_json(
            ctx,
            reqwest::Method::GET,
            &format!(
                "{API}{}",
                if query.is_empty() {
                    String::new()
                } else {
                    format!("?{query}")
                }
            ),
            None,
        )?)
    }
    fn get_all_affinity_groups(
        &self,
        ctx: &dyn Context,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        self.get_affinity_groups(ctx, &[])
    }
}

#[cfg(test)]
#[path = "http_client_test.rs"]
mod tests;
