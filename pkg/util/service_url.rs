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

use anyhow::{Result, anyhow};
use std::fmt;
use url::{ParseError, Url};

pub const URLSchemeHTTP: &str = "http";
pub const URLSchemeHTTPS: &str = "https";
pub const URLSchemeUnix: &str = "unix";
pub const URLSchemeUnixs: &str = "unixs";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceURL {
    scheme: String,
    address: String,
}

fn supported(scheme: &str) -> bool {
    matches!(
        scheme,
        URLSchemeHTTP | URLSchemeHTTPS | URLSchemeUnix | URLSchemeUnixs
    )
}

// Go net.SplitHostPort validates syntax without requiring a numeric port.
fn split_host_port(address: &str) -> Option<(&str, &str)> {
    if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        if host.contains(['[', ']']) || port.contains(':') {
            return None;
        }
        return Some((host, port));
    }
    let (host, port) = address.rsplit_once(':')?;
    if host.contains([':', '[', ']']) {
        return None;
    }
    Some((host, port))
}

pub fn ParseServiceURL(raw: &str) -> Result<ServiceURL> {
    parse_service_url(raw, "")
}

fn parse_service_url(raw: &str, default_scheme: &str) -> Result<ServiceURL> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(anyhow!("URL must not be empty"));
    }
    if !raw.contains("://") {
        if !supported(default_scheme) {
            return Err(anyhow!(
                "URL scheme must be http, https, unix, or unixs: {}",
                if default_scheme.is_empty() {
                    raw
                } else {
                    default_scheme
                }
            ));
        }
        if split_host_port(raw).is_none() {
            return Err(anyhow!(
                "URL address does not have the form \"host:port\": {raw}"
            ));
        }
        return Ok(ServiceURL {
            scheme: default_scheme.to_owned(),
            address: raw.to_owned(),
        });
    }
    for scheme in [URLSchemeUnix, URLSchemeUnixs] {
        if let Some(address) = raw.strip_prefix(&format!("{scheme}://")) {
            if address.is_empty() {
                return Err(anyhow!("URL address must not be empty: {raw}"));
            }
            return Ok(ServiceURL {
                scheme: scheme.to_owned(),
                address: address.to_owned(),
            });
        }
    }
    let (scheme, rest) = raw.split_once("://").expect("checked above");
    if let Err(error) = Url::parse(raw) {
        // Go net/url accepts service-name ports; WHATWG Url rejects them.
        if error != ParseError::InvalidPort {
            return Err(anyhow!("parse url {raw} failed {error}"));
        }
    }
    if !supported(scheme) {
        return Err(anyhow!(
            "URL scheme must be http, https, unix, or unixs: {raw}"
        ));
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let Some((host, port)) = split_host_port(host_port) else {
        return Err(anyhow!(
            "URL address does not have the form \"host:port\": {raw}"
        ));
    };
    if host.is_empty() || port.is_empty() {
        return Err(anyhow!(
            "URL address does not have the form \"host:port\": {raw}"
        ));
    }
    if rest[authority_end..].starts_with('/') {
        return Err(anyhow!("URL must not contain a path: {raw}"));
    }
    Ok(ServiceURL {
        scheme: scheme.to_owned(),
        address: host_port.to_owned(),
    })
}

pub fn NormalizeServiceURL(raw: &str, default_scheme: &str) -> Result<String> {
    Ok(parse_service_url(raw, default_scheme)?.to_string())
}

impl ServiceURL {
    pub fn SchemePrefix(&self) -> String {
        format!("{}://", self.scheme)
    }
    pub fn Address(&self) -> &str {
        &self.address
    }
    pub fn Endpoint(&self, with_scheme: bool) -> String {
        if with_scheme || self.IsUnixFamily() {
            self.to_string()
        } else {
            self.address.clone()
        }
    }
    pub fn IsUnixFamily(&self) -> bool {
        matches!(self.scheme.as_str(), URLSchemeUnix | URLSchemeUnixs)
    }
}

impl fmt::Display for ServiceURL {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.address)
    }
}
