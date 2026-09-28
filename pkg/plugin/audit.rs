// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 审计（Audit）插件的清单、事件类型与回调签名。
//
// 审计插件在连接、语句生命周期、全局变量变更、SQL 解析等时机被框架回调，
// 用于记录或拦截操作。本文件定义事件枚举、会话视图、Manifest 扩展及 Context 键。

use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use crate::{Context, ContextKey, ExportManifest, Manifest, PluginError, PluginErrorKind};

/// Corresponds to Go `plugin.GeneralEvent`.
/// 语句级通用审计事件：开始、完成、错误。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneralEvent {
    /// 语句开始执行。
    Starting = 0,
    /// 语句执行完成。
    Completed = 1,
    /// 语句执行出错。
    Error = 2,
}

impl GeneralEvent {
    /// Corresponds to Go `GeneralEventCount`.
    /// 合法 GeneralEvent 取值个数。
    pub const COUNT: u8 = 3;

    /// 返回与 Go 一致的大写事件名字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Completed => "COMPLETED",
            Self::Error => "ERROR",
        }
    }

    /// 由数值还原事件；非法值返回 None。
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Starting),
            1 => Some(Self::Completed),
            2 => Some(Self::Error),
            _ => None,
        }
    }
}

/// Corresponds to Go `GeneralEventFromString`.
/// 解析字符串形式的 GeneralEvent（大小写不敏感）。
pub fn general_event_from_string(value: &str) -> Result<GeneralEvent, PluginError> {
    value.parse()
}

impl FromStr for GeneralEvent {
    type Err = PluginError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_uppercase().as_str() {
            "STARTING" => Ok(Self::Starting),
            "COMPLETED" => Ok(Self::Completed),
            "ERROR" => Ok(Self::Error),
            _ => Err(PluginError::new(
                PluginErrorKind::Backend,
                format!("Invalid general event: {value}"),
            )),
        }
    }
}

impl std::fmt::Display for GeneralEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Corresponds to Go `plugin.ConnectionEvent` (`type ConnectionEvent byte`).
/// 连接生命周期事件（连接、断开、切换用户、鉴权前、拒绝等）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionEvent(pub u8);

impl ConnectionEvent {
    /// 客户端已建立连接。
    #[allow(non_upper_case_globals)]
    pub const Connected: Self = Self(0);
    /// 连接断开。
    #[allow(non_upper_case_globals)]
    pub const Disconnect: Self = Self(1);
    /// COM_CHANGE_USER 切换用户。
    #[allow(non_upper_case_globals)]
    pub const ChangeUser: Self = Self(2);
    /// 鉴权之前的钩子。
    #[allow(non_upper_case_globals)]
    pub const PreAuth: Self = Self(3);
    /// 连接/登录被拒绝。
    #[allow(non_upper_case_globals)]
    pub const Reject: Self = Self(4);

    /// 返回可读事件名；未知取值返回空串。
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            0 => "Connected",
            1 => "Disconnect",
            2 => "ChangeUser",
            3 => "PreAuth",
            4 => "Reject",
            _ => "",
        }
    }
}

impl std::fmt::Display for ConnectionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// SQL 解析前后事件（PreParse / PostParse）。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseEvent {
    PreParse = 1,
    PostParse,
}

/// 连接侧展示给审计回调的用户/主机/库等信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub user: String,
    pub host: String,
    pub database: String,
    pub connection_type: String,
}

/// 语句涉及的库表条目。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableEntry {
    pub db: String,
    pub table: String,
}

/// Minimal session view exposed to audit plugins. Mirrors the fields Go's audit
/// callbacks read from `*variable.SessionVars` / `StmtCtx`.
/// 暴露给审计插件的最小会话视图，字段对齐 Go SessionVars / StmtCtx。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionVars {
    pub connection_id: u64,
    pub original_sql: String,
    pub stmt_type: String,
    pub affected_rows: u64,
    pub tables: Vec<TableEntry>,
    pub status: u16,
    pub user: String,
}

/// 连接事件回调：可返回错误以拒绝连接等。
pub type ConnectionEventCallback = Arc<
    dyn Fn(&Context, ConnectionEvent, &ConnectionInfo) -> Result<(), PluginError> + Send + Sync,
>;
/// 语句通用事件回调（通常只记录，不返回错误）。
pub type GeneralEventCallback =
    Arc<dyn Fn(&Context, Option<&SessionVars>, GeneralEvent, &str) + Send + Sync>;
/// 全局变量变更事件回调。
pub type GlobalVariableEventCallback =
    Arc<dyn Fn(&Context, Option<&SessionVars>, &str, &str) + Send + Sync>;
/// 解析事件回调：可返回错误中止解析。
pub type ParseEventCallback = Arc<
    dyn Fn(&Context, Option<&SessionVars>, ParseEvent) -> Result<(), PluginError> + Send + Sync,
>;

/// 审计插件清单：基础 Manifest 加上可选的四类事件回调。
#[derive(Clone)]
pub struct AuditManifest {
    pub manifest: Manifest,
    pub on_connection_event: Option<ConnectionEventCallback>,
    pub on_general_event: Option<GeneralEventCallback>,
    pub on_global_variable_event: Option<GlobalVariableEventCallback>,
    pub on_parse_event: Option<ParseEventCallback>,
}

impl Default for AuditManifest {
    fn default() -> Self {
        Self {
            manifest: Manifest::default(),
            on_connection_event: None,
            on_general_event: None,
            on_global_variable_event: None,
            on_parse_event: None,
        }
    }
}

/// 挂到 Manifest.extension 上的回调集合，供运行时 downcast 取用。
#[derive(Clone)]
pub struct AuditCallbacks {
    pub on_connection_event: Option<ConnectionEventCallback>,
    pub on_general_event: Option<GeneralEventCallback>,
    pub on_global_variable_event: Option<GlobalVariableEventCallback>,
    pub on_parse_event: Option<ParseEventCallback>,
}

impl ExportManifest for AuditManifest {
    fn export_manifest(&self) -> Manifest {
        // 将四类回调打包进 extension，供插件框架统一加载。
        let mut manifest = self.manifest.clone();
        manifest.extension = Some(Arc::new(AuditCallbacks {
            on_connection_event: self.on_connection_event.clone(),
            on_general_event: self.on_general_event.clone(),
            on_global_variable_event: self.on_global_variable_event.clone(),
            on_parse_event: self.on_parse_event.clone(),
        }));
        manifest
    }
}

/// 从已加载 Manifest 中取出审计回调扩展。
pub fn audit_callbacks(manifest: &Manifest) -> Option<&AuditCallbacks> {
    manifest
        .extension
        .as_ref()?
        .downcast_ref::<AuditCallbacks>()
}

/// Context 键：拒绝连接时的原因字符串。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RejectReasonContextKey;
impl ContextKey for RejectReasonContextKey {
    type Value = String;
}
/// Context 键：语句开始执行时间。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExecStartTimeContextKey;
impl ContextKey for ExecStartTimeContextKey {
    type Value = SystemTime;
}
/// Context 键：Prepare 语句 ID。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrepareStatementIdContextKey;
impl ContextKey for PrepareStatementIdContextKey {
    type Value = u32;
}
/// Context 键：当前是否处于重试执行。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IsRetryingContextKey;
impl ContextKey for IsRetryingContextKey {
    type Value = bool;
}

/// 执行开始时间 Context 键单例。
pub const EXEC_START_TIME_CONTEXT_KEY: ExecStartTimeContextKey = ExecStartTimeContextKey;
/// Prepare 语句 ID Context 键单例。
pub const PREPARE_STATEMENT_ID_CONTEXT_KEY: PrepareStatementIdContextKey =
    PrepareStatementIdContextKey;
/// 是否重试 Context 键单例。
pub const IS_RETRYING_CONTEXT_KEY: IsRetryingContextKey = IsRetryingContextKey;
