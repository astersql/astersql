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

//! Warning-only, one-shot verification of a server's advertised `/info` identity.

use astersql_domain_serverinfo::StaticInfo;
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use std::net::SocketAddr;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::watch;

pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
pub const BODY_LIMIT: usize = 1 << 20;
pub const WARNING_MESSAGE: &str = "failed to verify advertised status endpoint identity";

#[derive(Clone, Debug, Default)]
pub struct TlsOptions {
    pub ca: Option<String>,
    pub certificate: Option<String>,
    pub key: Option<String>,
}

impl TlsOptions {
    pub fn scheme(&self) -> &'static str {
        if self.ca.is_some() || self.certificate.is_some() || self.key.is_some() {
            "https"
        } else {
            "http"
        }
    }

    // Rebuild the internal transport with the same trust and client identity;
    // reqwest clients cannot clone a transport while changing proxy/redirect policy.
    pub fn builder(&self) -> Result<reqwest::ClientBuilder, String> {
        let mut builder = reqwest::Client::builder();
        if let Some(ca) = &self.ca {
            let bytes = std::fs::read(ca).map_err(|e| e.to_string())?;
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(&bytes).map_err(|e| e.to_string())?,
            );
        }
        if let (Some(certificate), Some(key)) = (&self.certificate, &self.key) {
            let mut bytes = std::fs::read(certificate).map_err(|e| e.to_string())?;
            bytes.extend(std::fs::read(key).map_err(|e| e.to_string())?);
            builder =
                builder.identity(reqwest::Identity::from_pem(&bytes).map_err(|e| e.to_string())?);
        }
        Ok(builder)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub report_status: bool,
    pub status_address: Option<SocketAddr>,
    pub advertise_address: String,
    pub local_id: String,
    pub tls: TlsOptions,
}

impl Options {
    pub fn endpoint(&self) -> Option<String> {
        if !self.report_status || self.advertise_address.is_empty() || self.local_id.is_empty() {
            return None;
        }
        let port = self.status_address?.port();
        let host = if self.advertise_address.contains(':') {
            format!("[{}]", self.advertise_address)
        } else {
            self.advertise_address.clone()
        };
        Some(format!("{}://{host}:{port}/info", self.tls.scheme()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckResult {
    pub reason: &'static str,
    pub remote_id: String,
    pub status: String,
    pub error: Option<String>,
}

impl CheckResult {
    fn request_failed(error: reqwest::Error) -> Self {
        let mut message = error.to_string();
        let mut cause = std::error::Error::source(&error);
        while let Some(error) = cause {
            message.push_str(": ");
            message.push_str(&error.to_string());
            cause = error.source();
        }
        Self::failed(message)
    }

    fn failed(error: impl ToString) -> Self {
        Self {
            reason: "request-failed",
            error: Some(error.to_string()),
            ..Self::default()
        }
    }
}

pub fn new_http_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, reqwest::Error> {
    builder
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(CHECK_TIMEOUT)
        .build()
}

pub async fn check_endpoint(
    client: &reqwest::Client,
    endpoint: &str,
    expected_id: &str,
    mut cancelled: watch::Receiver<bool>,
) -> CheckResult {
    if *cancelled.borrow() {
        return CheckResult::failed("context canceled");
    }
    tokio::select! {
        biased;
        _ = cancelled.changed() => CheckResult::failed("context canceled"),
        result = check_response(client, endpoint, expected_id) => result,
    }
}

async fn check_response(
    client: &reqwest::Client,
    endpoint: &str,
    expected_id: &str,
) -> CheckResult {
    let mut response = match client.get(endpoint).send().await {
        Ok(response) => response,
        Err(error) => return CheckResult::request_failed(error),
    };
    let status = response.status();
    let reason_phrase = response
        .extensions()
        .get::<hyper::ext::ReasonPhrase>()
        .map(|phrase| String::from_utf8_lossy(phrase.as_bytes()).into_owned())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("").to_owned());
    let mut result = CheckResult {
        status: format!("{} {}", status.as_u16(), reason_phrase),
        ..CheckResult::default()
    };
    if !status.is_success() {
        result.reason = "unexpected-status";
        return result;
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk[..chunk.len().min(BODY_LIMIT + 1 - body.len())]);
                if body.len() > BODY_LIMIT {
                    result.reason = "invalid-response";
                    result.error = Some(format!("response body exceeds {BODY_LIMIT}-byte limit"));
                    return result;
                }
            }
            Ok(None) => break,
            Err(error) => {
                result.reason = "request-failed";
                result.error = CheckResult::request_failed(error).error;
                return result;
            }
        }
    }
    let mut info = StaticInfo::default();
    if let Err(error) = info.Unmarshal(&body) {
        result.reason = "invalid-response";
        result.error = Some(error.to_string());
        return result;
    }
    if info.ID.is_empty() {
        result.reason = "missing-identity";
        result.error = Some("response does not contain ddl_id".into());
        return result;
    }
    result.remote_id = info.ID;
    if result.remote_id != expected_id {
        result.reason = "identity-mismatch";
    }
    result
}

pub fn warning_action(reason: &str) -> &'static str {
    match reason {
        "request-failed" => {
            "check DNS, network, TLS, and whether this TiDB instance can complete a request to the advertised status endpoint"
        }
        "unexpected-status" | "invalid-response" | "missing-identity" => {
            "check that advertise-address and status-port serve a valid TiDB /info response"
        }
        "identity-mismatch" => {
            "check that advertise-address and status-port route directly to this TiDB instance and that no TiDB exists outside the intended topology"
        }
        _ => "inspect the error and advertised status endpoint",
    }
}

pub fn warning_fields(endpoint: &str, local_id: &str, result: &CheckResult) -> Vec<LogField> {
    let mut fields = vec![
        LogField::String("advertised-status-endpoint".into(), endpoint.into()),
        LogField::String("local-tidb-id".into(), local_id.into()),
        LogField::String("reason".into(), result.reason.into()),
        LogField::String("action".into(), warning_action(result.reason).into()),
    ];
    if !result.remote_id.is_empty() {
        fields.push(LogField::String(
            "remote-tidb-id".into(),
            result.remote_id.clone(),
        ));
    }
    if !result.status.is_empty() {
        fields.push(LogField::String(
            "http-status".into(),
            result.status.clone(),
        ));
    }
    if let Some(error) = &result.error {
        fields.push(LogField::String("error".into(), error.clone()));
    }
    fields
}

/// Owns cancellation and joins the request worker before releasing its resources.
pub struct CheckHandle {
    cancelled: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl CheckHandle {
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
}

impl Drop for CheckHandle {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn start(options: Options) -> Option<CheckHandle> {
    start_with_reporter(options, |endpoint, id, result| {
        BgLogger().log(
            LogLevel::Warn,
            WARNING_MESSAGE,
            warning_fields(endpoint, id, result),
        );
    })
}

pub fn start_with_reporter(
    options: Options,
    reporter: impl FnOnce(&str, &str, &CheckResult) + Send + 'static,
) -> Option<CheckHandle> {
    let tls = options.tls.clone();
    start_with_client_builder(options, move || tls.builder(), reporter)
}

pub(crate) fn start_with_client_builder(
    options: Options,
    builder: impl FnOnce() -> Result<reqwest::ClientBuilder, String> + Send + 'static,
    reporter: impl FnOnce(&str, &str, &CheckResult) + Send + 'static,
) -> Option<CheckHandle> {
    let endpoint = options.endpoint()?;
    let (cancelled, receiver) = watch::channel(false);
    let worker = thread::spawn(move || {
        // Panics in diagnostics must not terminate the server.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let result = (|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let result = runtime.block_on(async {
                    let client = new_http_client(builder()?).map_err(|e| e.to_string())?;
                    Ok::<_, String>(
                        check_endpoint(&client, &endpoint, &options.local_id, receiver.clone())
                            .await,
                    )
                });
                // OS DNS jobs are not abortable. Like Go context cancellation,
                // ending this Run must not wait for such a job to finish.
                runtime.shutdown_background();
                result
            })()
            .unwrap_or_else(CheckResult::failed);
            if !*receiver.borrow() && !result.reason.is_empty() {
                reporter(&endpoint, &options.local_id, &result);
            }
        }))
        .is_err()
        {
            BgLogger().warn("advertised status endpoint check panicked");
        }
    });
    Some(CheckHandle {
        cancelled,
        worker: Some(worker),
    })
}
