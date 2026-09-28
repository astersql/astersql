// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 主机端口地址列表解析。
//
// 由 `urls.go` 迁移。`ParseHostPortAddr` 接受逗号分隔的 `host:port` 或
// `http(s)|unix(s)://` URL，校验 scheme / host:port 形态且禁止 path。

use anyhow::{Result, anyhow};
use url::{ParseError, Url};

/// 判断字符串是否已是 `host:port`（含 IPv6 `[host]:port`）形态。
fn split_host_port(address: &str) -> bool {
    // IPv6：只要求存在 `]:` 分隔。Go net.SplitHostPort 允许空 host 或空 port。
    if let Some(rest) = address.strip_prefix('[') {
        return rest
            .split_once("]:")
            .is_some_and(|(host, port)| !host.contains(['[', ']']) && !port.contains(':'));
    }
    // IPv4 / 主机名：最后一个 `:` 右侧为 port，左侧不得再含未括起的 `:`。
    // host/port 内容可为空，和 net.SplitHostPort(":") 一致。
    address
        .rsplit_once(':')
        .is_some_and(|(host, _port)| !host.contains([':', '[', ']']))
}

/// 解析逗号分隔的地址列表，返回修剪后的原始地址字符串向量。
pub fn ParseHostPortAddr(input: &str) -> Result<Vec<String>> {
    let mut addresses = Vec::with_capacity(input.split(',').count());
    for address in input.split(',').map(str::trim) {
        // 已是 host:port 则直接收录，无需 URL 解析。
        if split_host_port(address) {
            addresses.push(address.to_owned());
            continue;
        }

        let parsed = Url::parse(address);
        let (raw_scheme, remainder) = address
            .split_once(':')
            .ok_or_else(|| anyhow!("parse url {address} failed relative URL without a base"))?;
        let scheme = match &parsed {
            Ok(parsed) => parsed.scheme(),
            // WHATWG URL parsing restricts ports to u16 numbers, while Go's
            // net/url leaves validation to net.SplitHostPort, which accepts
            // service names and larger decimal strings.
            Err(ParseError::InvalidPort) => raw_scheme,
            Err(error) => return Err(anyhow!("parse url {address} failed {error}")),
        };
        // 仅允许 http/https/unix/unixs（unix 套接字常用于本地 TiDB 通信）。
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "unix" | "unixs"
        ) {
            return Err(anyhow!(
                "URL scheme must be http, https, unix, or unixs: {address}"
            ));
        }
        let Some(authority_and_tail) = remainder.strip_prefix("//") else {
            return Err(anyhow!(
                "URL address does not have the form \"host:port\": {address}"
            ));
        };
        let authority_end = authority_and_tail
            .find(['/', '?', '#'])
            .unwrap_or(authority_and_tail.len());
        let authority = &authority_and_tail[..authority_end];
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if !split_host_port(host_port) {
            return Err(anyhow!(
                "URL address does not have the form \"host:port\": {address}"
            ));
        }
        // 只检查 URL path；query / fragment 中的斜杠不属于 path。
        if authority_and_tail[authority_end..].starts_with('/') {
            return Err(anyhow!("URL must not contain a path: {address}"));
        }
        // Go's net/url lower-cases the parsed scheme in URL.String().
        addresses.push(format!("{}:{remainder}", scheme.to_ascii_lowercase()));
    }
    Ok(addresses)
}
