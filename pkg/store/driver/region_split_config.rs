// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use astersql_kv as kv;
use futures::FutureExt;
use futures::future::{Either, select};

use super::adapter_error;

fn resolve(address: &str) -> Option<SocketAddr> {
    let addresses = address.to_socket_addrs().ok()?.collect::<Vec<_>>();
    addresses
        .iter()
        .find(|a| a.is_ipv4())
        .copied()
        .or_else(|| addresses.first().copied())
}

fn status_address(address: &str, status: &str) -> String {
    let (Some(address), Some(mut status_address)) = (resolve(address), resolve(status)) else {
        return status.to_owned();
    };
    let local = |a: SocketAddr| a.ip().is_loopback() || a.ip().is_unspecified();
    if !local(address) && local(status_address) {
        status_address.set_ip(address.ip());
        status_address.to_string()
    } else {
        status.to_owned()
    }
}

async fn json(
    client: &reqwest::Client,
    context: &kv::Context,
    url: String,
) -> Result<serde_json::Value, kv::errors::SharedError> {
    if context.is_cancelled() {
        return Err(adapter_error("context canceled"));
    }
    let request = async {
        client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await
    };
    match select(request.boxed(), context.cancelled().boxed()).await {
        Either::Left((result, _)) => result.map_err(adapter_error),
        Either::Right(_) => Err(adapter_error("context canceled")),
    }
}

pub(super) async fn get_region_split_config(
    context: &kv::Context,
    pd_addresses: &[String],
    tls: Option<crate::tikv_driver::TlsConfig>,
) -> Result<(i64, i64), kv::errors::SharedError> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10));
    let scheme = if let Some(tls) = tls {
        let ca = std::fs::read(&tls.ca_path).map_err(adapter_error)?;
        let mut identity = std::fs::read(&tls.cert_path).map_err(adapter_error)?;
        identity.extend_from_slice(&std::fs::read(&tls.key_path).map_err(adapter_error)?);
        builder = builder
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).map_err(adapter_error)?)
            .identity(reqwest::Identity::from_pem(&identity).map_err(adapter_error)?);
        "https"
    } else {
        "http"
    };
    let client = builder.build().map_err(adapter_error)?;
    let mut stores = None;
    let mut last_error = adapter_error("no PD addresses configured");
    for address in pd_addresses {
        let base = if address.contains("://") {
            address.trim_end_matches('/').to_owned()
        } else {
            format!("{scheme}://{address}")
        };
        match json(&client, context, format!("{base}/pd/api/v1/stores")).await {
            Ok(response) => {
                if let Some(entries) = response["stores"].as_array() {
                    stores = Some(entries.clone());
                    break;
                }
                last_error = adapter_error("invalid PD stores response");
            }
            Err(error) => last_error = error,
        }
        if context.is_cancelled() {
            return Err(adapter_error("context canceled"));
        }
    }
    let stores = stores.ok_or(last_error)?;
    for entry in stores {
        let store = &entry["store"];
        if store["state"] == 2
            || store["state_name"] == "Tombstone"
            || store["labels"].as_array().is_some_and(|labels| {
                labels
                    .iter()
                    .any(|l| l["key"] == "engine" && l["value"] == "tiflash")
            })
        {
            continue;
        }
        let status = store["status_address"].as_str().unwrap_or_default();
        if status.is_empty() {
            continue;
        }
        let status = status_address(store["address"].as_str().unwrap_or_default(), status);
        let result = async {
            let response = json(&client, context, format!("{scheme}://{status}/config")).await?;
            let config = &response["coprocessor"];
            let size = match &config["region-split-size"] {
                serde_json::Value::String(size) => size.as_str(),
                serde_json::Value::Null => "",
                _ => return Err(adapter_error("invalid region-split-size type")),
            };
            let keys = match &config["region-split-keys"] {
                serde_json::Value::Null => 0,
                value => value
                    .as_i64()
                    .ok_or_else(|| adapter_error("invalid region-split-keys type"))?,
            };
            let size =
                astersql_config_configtypes::ParseGoSize(size, false).map_err(adapter_error)?;
            Ok((size, keys))
        }
        .await;
        match result {
            Ok(config) => return Ok(config),
            Err(error) => {
                if context.is_cancelled() {
                    return Err(adapter_error("context canceled"));
                }
                eprintln!("get region split size and keys failed: store={status}: {error}");
            }
        }
    }
    Err(adapter_error("get region split size and keys failed"))
}

#[cfg(test)]
#[path = "region_split_config_test.rs"]
mod tests;
