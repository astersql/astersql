// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `SET CONFIG` 语句执行器。
//
// 对应 Go 的 `SetConfigExec`：在线修改集群组件（TiKV / PD / TiFlash 等）的配置项。
// 通过 HTTP POST JSON 下发到各节点 status 地址；TiDB / TSO / scheduling 不支持在线改配。
// `SetConfigBackend` 抽象集群发现与 HTTP 调用，便于测试注入。

#![allow(non_snake_case)]

use std::net::ToSocketAddrs;

use astersql_util_chunk::Chunk;

/// 测试用：注入假集群节点信息的 context key。
pub const TestSetConfigServerInfoKey: &str = "TestSetConfigServerInfoKey";
/// 测试用：注入假 HTTP 处理器的 context key。
pub const TestSetConfigHTTPHandlerKey: &str = "TestSetConfigHTTPHandlerKey";

/// 配置项取值的类型化表示（对应表达式求值结果）。
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    /// SQL NULL，不能写入配置。
    Null,
    /// 字符串配置值。
    String(String),
    /// 整数配置值。
    Int(i64),
    /// 布尔配置值。
    Boolean(bool),
    /// 浮点配置值。
    Real(f64),
    /// 十进制字符串形式的数值。
    Decimal(String),
    /// 当前不支持的类型。
    Unsupported,
}

/// `SET CONFIG` 的计划节点：目标节点类型、实例、配置名与取值。
#[derive(Clone, Debug, PartialEq)]
pub struct SetConfigPlan {
    /// 节点类型：`tikv` / `pd` / `tiflash` 等；空表示全部类型。
    pub node_type: String,
    /// 目标实例的 status 地址（host:port）；空表示该类型下全部实例。
    pub instance: String,
    /// 配置项名称（会转小写；TiFlash 需带 `raftstore-proxy.` 前缀）。
    pub name: String,
    /// 要写入的配置值。
    pub value: ConfigValue,
}

/// 集群中一个可接收配置变更的服务端点。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigServerInfo {
    /// 服务类型（与 `node_type` 对齐）。
    pub server_type: String,
    /// status / HTTP 监听地址。
    pub status_address: String,
}

/// 配置下发 HTTP 响应摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigHttpResponse {
    /// HTTP 状态码。
    pub status_code: u16,
    /// 状态文本。
    pub status: String,
    /// 响应体（错误信息常在此）。
    pub body: Vec<u8>,
}

/// 生产边界：集群节点发现、内部 HTTP 调用与警告收集。
pub trait SetConfigBackend {
    /// 后端错误类型。
    type Error;

    /// 列出当前集群中可配置的服务端点。
    fn cluster_servers(&mut self) -> Result<Vec<ConfigServerInfo>, Self::Error>;
    /// 内部 HTTP 协议方案（如 `http` / `https`）。
    fn internal_http_scheme(&self) -> &str;
    /// 向节点 POST JSON 配置体。
    fn post_json(&mut self, url: &str, body: &str) -> Result<ConfigHttpResponse, Self::Error>;
    /// 将单节点失败记为会话警告（不中断其余节点）。
    fn append_warning(&mut self, error: Self::Error);
    /// 构造带消息的后端错误。
    fn error(&self, message: String) -> Self::Error;
}

/// `SET CONFIG` 执行器状态：持有后端、计划与预序列化的 JSON 请求体。
pub struct SetConfigExec<B: SetConfigBackend> {
    /// 集群与 HTTP 后端。
    pub backend: B,
    /// 解析后的配置变更计划。
    pub plan: SetConfigPlan,
    /// Open 阶段生成的 JSON 请求体，Next 中复用。
    pub json_body: String,
}

impl<B: SetConfigBackend> SetConfigExec<B> {
    /// 打开执行器：校验节点类型/实例，规范化配置名，并预生成 JSON。
    pub fn Open<C>(&mut self, _ctx: C) -> Result<(), B::Error> {
        // 有指定节点类型时先规范化并校验白名单。
        if !self.plan.node_type.is_empty() {
            self.plan.node_type.make_ascii_lowercase();
            if !matches!(
                self.plan.node_type.as_str(),
                "tikv" | "tidb" | "pd" | "tiflash" | "tso" | "scheduling"
            ) {
                return Err(self
                    .backend
                    .error(format!("unknown type {}", self.plan.node_type)));
            }
            // TiDB / 微服务组件不支持在线改配，在 Open 阶段直接失败。
            match self.plan.node_type.as_str() {
                "tidb" => {
                    return Err(self.backend.error(
                        "TiDB doesn't support to change configs online, please use SQL variables"
                            .to_owned(),
                    ));
                }
                "tso" | "scheduling" => {
                    return Err(self.backend.error(format!(
                        "{} doesn't support to change configs online",
                        self.plan.node_type
                    )));
                }
                _ => {}
            }
        }
        // 指定实例时校验 host:port 可解析。
        if !self.plan.instance.is_empty() {
            self.plan.instance.make_ascii_lowercase();
            if !isValidInstance(&self.plan.instance) {
                return Err(self
                    .backend
                    .error(format!("invalid instance {}", self.plan.instance)));
            }
        }
        self.plan.name.make_ascii_lowercase();
        // TiFlash 仅允许修改 raftstore-proxy 前缀下的配置项。
        if self.plan.node_type == "tiflash" {
            const PREFIX: &str = "raftstore-proxy.";
            if !self.plan.name.starts_with(PREFIX) {
                return Err(self.backend.error("This command can only change config items begin with 'raftstore-proxy'. For other TiFlash config items, please update the config file directly. Your change to the config file will take effect immediately without a restart.".to_owned()));
            }
            self.plan.name = self.plan.name[PREFIX.len()..].to_owned();
        }
        self.json_body = ConvertConfigItem2JSON(&self.plan.name, &self.plan.value)
            .map_err(|message| self.backend.error(message))?;
        Ok(())
    }

    /// 执行配置下发：按类型/实例过滤节点，逐个 POST；单节点失败记警告。
    pub fn Next<C>(&mut self, _ctx: C, request: &mut Chunk) -> Result<(), B::Error> {
        request.Reset();
        let mut servers = self.backend.cluster_servers()?;
        if !self.plan.node_type.is_empty() {
            servers.retain(|server| server.server_type == self.plan.node_type);
        }
        if !self.plan.instance.is_empty() {
            servers.retain(|server| server.status_address == self.plan.instance);
            if servers.is_empty() {
                return Err(self.backend.error(format!(
                    "instance {} is not found in this cluster",
                    self.plan.instance
                )));
            }
        }

        // 按组件选择不同的配置 HTTP 路径。
        for server in servers {
            let path = match server.server_type.as_str() {
                "pd" => "/pd/api/v1/config",
                "tikv" | "tiflash" => "/config",
                "tidb" => {
                    return Err(self.backend.error(
                        "TiDB doesn't support to change configs online, please use SQL variables"
                            .to_owned(),
                    ));
                }
                unknown => {
                    return Err(self.backend.error(format!("Unknown server type {unknown}")));
                }
            };
            let url = format!(
                "{}://{}{}",
                self.backend.internal_http_scheme(),
                server.status_address,
                path
            );
            if let Err(error) = self.doRequest(&url) {
                self.backend.append_warning(error);
            }
        }
        Ok(())
    }

    /// 向单个 URL 发送 JSON，并按状态码映射成功/错误。
    pub fn doRequest(&mut self, url: &str) -> Result<(), B::Error> {
        let response = self.backend.post_json(url, &self.json_body)?;
        match response.status_code {
            200 => Ok(()),
            400..=599 => Err(self.backend.error(format!(
                "bad request to {}: {}",
                url,
                String::from_utf8_lossy(&response.body)
            ))),
            _ => Err(self
                .backend
                .error(format!("request {} failed: {}", url, response.status))),
        }
    }
}

/// 判断实例字符串是否为可解析的 `host:port` 地址。
pub fn isValidInstance(instance: &str) -> bool {
    let host = if let Some(bracketed) = instance.strip_prefix('[') {
        let Some((host, port)) = bracketed.split_once("]:") else {
            return false;
        };
        if port.is_empty() || port.contains(':') {
            return false;
        }
        host
    } else {
        let Some((host, port)) = instance.rsplit_once(':') else {
            return false;
        };
        if host.contains(':') || port.is_empty() {
            return false;
        }
        host
    };

    !host.is_empty()
        && (host, 0)
            .to_socket_addrs()
            .is_ok_and(|mut addresses| addresses.next().is_some())
}

/// 将配置键值转为单字段 JSON 对象字符串（键用 Debug 引号转义）。
pub fn ConvertConfigItem2JSON(key: &str, value: &ConfigValue) -> Result<String, String> {
    let value = match value {
        ConfigValue::Null => return Err("cannot set config to null".to_owned()),
        ConfigValue::String(value) => format!("{value:?}"),
        ConfigValue::Int(value) => value.to_string(),
        ConfigValue::Boolean(value) => value.to_string(),
        ConfigValue::Real(value) => value.to_string(),
        ConfigValue::Decimal(value) => value.clone(),
        ConfigValue::Unsupported => return Err("unsupported config value type".to_owned()),
    };
    Ok(format!("{{{key:?}:{value}}}"))
}
