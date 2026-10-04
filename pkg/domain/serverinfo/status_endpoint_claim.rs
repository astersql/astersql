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

//! Best-effort ownership of an advertised status endpoint under the server-info lease.
use crate::{Context, EtcdClient, KeyOpDefaultTimeout, ServerInfo, SyncError};
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use base64::Engine;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointClaimState {
    Skipped,
    Acquired,
    Conflict,
    CheckFailed,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObservedStatusEndpointClaim {
    pub id: String,
    pub lease: i64,
    pub mod_revision: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusEndpointClaimResult {
    pub state: EndpointClaimState,
    pub endpoint: String,
    pub claim_key: String,
    pub local_id: String,
    pub existing_id: String,
    pub existing_lease: i64,
    pub error: Option<SyncError>,
}
impl StatusEndpointClaimResult {
    fn apply_create(
        &mut self,
        outcome: Result<(bool, ObservedStatusEndpointClaim), SyncError>,
    ) -> Option<ObservedStatusEndpointClaim> {
        match outcome {
            Err(error) => {
                self.state = EndpointClaimState::CheckFailed;
                self.error = Some(error);
                None
            }
            Ok((true, _)) => {
                self.state = EndpointClaimState::Acquired;
                None
            }
            Ok((false, observed)) => {
                self.existing_id = observed.id.clone();
                self.existing_lease = observed.lease;
                if observed.id != self.local_id {
                    self.state = EndpointClaimState::Conflict;
                    None
                } else {
                    Some(observed)
                }
            }
        }
    }
    pub fn diagnostic(&self, keyspace: &str) -> Option<(&'static str, Vec<LogField>)> {
        let mut fields = vec![
            LogField::String("advertised-status-endpoint".into(), self.endpoint.clone()),
            LogField::String("claim-key".into(), self.claim_key.clone()),
            LogField::String("local-server-info-id".into(), self.local_id.clone()),
        ];
        if !keyspace.is_empty() {
            fields.push(LogField::String("keyspace".into(), keyspace.into()));
        }
        match self.state {
            EndpointClaimState::Conflict => {
                fields.extend([
                    LogField::String("existing-server-info-id".into(), self.existing_id.clone()),
                    LogField::String("existing-lease-id".into(), format!("{:016x}", self.existing_lease)),
                    LogField::String("action".into(), "check for duplicate advertise-address and status-port settings, copied startup configuration, or a TiDB instance outside the intended topology".into()),
                ]);
                Some((
                    "advertised status endpoint already has an active claim",
                    fields,
                ))
            }
            EndpointClaimState::CheckFailed => {
                fields.extend([
                    LogField::String("action".into(), "check etcd connectivity and whether the advertised status endpoint claim can be read or updated".into()),
                    LogField::String("error".into(), self.error.as_ref().map(ToString::to_string).unwrap_or_default()),
                ]);
                Some(("failed to check advertised status endpoint claim", fields))
            }
            _ => None,
        }
    }
    pub fn report_result(&self, keyspace: &str) {
        if let Some((message, fields)) = self.diagnostic(keyspace) {
            BgLogger().log(LogLevel::Warn, message, fields);
        }
    }
    pub fn warning(&self, keyspace: &str) -> Option<String> {
        self.diagnostic(keyspace).map(|(message, fields)| {
            format!(
                "{message}: {}",
                fields
                    .into_iter()
                    .map(|field| match field {
                        LogField::String(key, value) => format!("{key}={value}"),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
    }
}

pub fn build_status_endpoint_claim(info: &ServerInfo, enabled: bool) -> (String, String) {
    if !enabled || info.StaticInfo.IsAssumed() {
        return Default::default();
    }
    let raw = info.StaticInfo.IP.trim();
    if raw.is_empty() {
        return Default::default();
    }
    let host = raw
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| {
            if let Some((ip, zone)) = raw.split_once('%') {
                if !zone.is_empty() {
                    if let Ok(ip) = ip.parse::<std::net::Ipv6Addr>() {
                        return format!("{ip}%{zone}");
                    }
                }
            }
            let lower = raw.to_lowercase();
            lower.strip_suffix('.').unwrap_or(&lower).to_owned()
        });
    if host.is_empty() {
        return Default::default();
    }
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let port = if info.StaticInfo.StatusPort == 0 {
        astersql_config::DEF_STATUS_PORT as u32
    } else {
        info.StaticInfo.StatusPort
    };
    let endpoint = format!("{host}:{port}");
    let key = format!(
        "/tidb/server/status_addr/{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(endpoint.as_bytes())
    );
    (endpoint, key)
}
pub struct StatusEndpointClaim<'a> {
    pub client: &'a dyn EtcdClient,
    pub endpoint: String,
    pub key: String,
    pub local_id: String,
}
impl<'a> StatusEndpointClaim<'a> {
    pub fn from_key(client: &'a dyn EtcdClient, info: &ServerInfo, key: String) -> Self {
        let endpoint = key
            .strip_prefix("/tidb/server/status_addr/")
            .and_then(|s| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(s)
                    .ok()
            })
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default();
        Self {
            client,
            endpoint,
            key,
            local_id: info.StaticInfo.ID.clone(),
        }
    }
    pub fn cleanup_fields(&self, lease: i64) -> Vec<LogField> {
        vec![
            LogField::String("advertised-status-endpoint".into(), self.endpoint.clone()),
            LogField::String("claim-key".into(), self.key.clone()),
            LogField::String("local-server-info-id".into(), self.local_id.clone()),
            LogField::String("lease-id".into(), format!("{lease:016x}")),
        ]
    }
    pub fn acquire(&self, context: &Context, lease: i64) -> StatusEndpointClaimResult {
        let mut result = StatusEndpointClaimResult {
            state: EndpointClaimState::Skipped,
            endpoint: self.endpoint.clone(),
            claim_key: self.key.clone(),
            local_id: self.local_id.clone(),
            existing_id: String::new(),
            existing_lease: 0,
            error: None,
        };
        if self.key.is_empty() {
            return result;
        }
        let context = context.WithTimeout(KeyOpDefaultTimeout);
        let Some(observed) = result.apply_create(self.client.TryCreateClaim(
            &context,
            &self.key,
            &self.local_id,
            lease,
        )) else {
            return result;
        };
        match self
            .client
            .ReattachClaim(&context, &self.key, &observed, lease)
        {
            Err(error) => {
                result.state = EndpointClaimState::CheckFailed;
                result.error = Some(error);
                return result;
            }
            Ok(true) => {
                result.state = EndpointClaimState::Acquired;
                return result;
            }
            Ok(false) => {}
        }
        if result
            .apply_create(
                self.client
                    .TryCreateClaim(&context, &self.key, &self.local_id, lease),
            )
            .is_some()
        {
            result.state = EndpointClaimState::CheckFailed;
            result.error = Some(SyncError("advertised status endpoint claim changed while reattaching the same server info ID".into()));
        }
        result
    }
    pub fn try_acquire_and_report(
        &self,
        context: &Context,
        lease: i64,
        report: impl FnOnce(&StatusEndpointClaimResult),
    ) -> Option<StatusEndpointClaimResult> {
        let result = self.acquire(context, lease);
        if context.Done() {
            return None;
        }
        report(&result);
        Some(result)
    }
}
