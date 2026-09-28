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

// 连接 IP 审计示例插件（`conn_ip_example`）。
//
// 演示如何实现 Audit 插件：校验/初始化/关闭钩子、注册示例系统变量，
// 以及在连接与语句通用事件中打印会话与连接信息。对应 Go 示例插件包。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use astersql_plugin::{
    AuditManifest, ConnectionEvent, ConnectionInfo as PluginConnectionInfo, Context, GeneralEvent,
    Kind, Manifest, PluginError, RejectReasonContextKey, SessionVars as PluginSessionVars,
};

/// 示例插件注册的系统变量名。
pub const SYSTEM_VARIABLE_NAME: &str = "conn_ip_example_key";
/// 全局作用域位标志。
pub const SCOPE_GLOBAL: u8 = 1;
/// 会话作用域位标志。
pub const SCOPE_SESSION: u8 = 2;

/// 累计收到的连接事件次数（示例计数器）。
static CONNECTIONS: AtomicI32 = AtomicI32::new(0);

/// 系统变量校验回调：规范化取值。
pub type ValidationCallback =
    Arc<dyn Fn(&SessionVars, &str, &str, u8) -> Result<String, PluginError> + Send + Sync>;
/// 设置会话级变量时的回调。
pub type SetSessionCallback =
    Arc<dyn Fn(&SessionVars, &str) -> Result<(), PluginError> + Send + Sync>;
/// 设置全局变量时的回调。
pub type SetGlobalCallback =
    Arc<dyn Fn(&Context, &SessionVars, &str) -> Result<(), PluginError> + Send + Sync>;

/// 示例用系统变量描述（名称、作用域、值与钩子）。
#[derive(Clone)]
pub struct SystemVariable {
    pub name: String,
    pub scope: u8,
    pub value: String,
    pub validation: Option<ValidationCallback>,
    pub set_session: Option<SetSessionCallback>,
    pub set_global: Option<SetGlobalCallback>,
}

/// 进程内系统变量注册表。
fn variables() -> &'static RwLock<HashMap<String, SystemVariable>> {
    static VARIABLES: OnceLock<RwLock<HashMap<String, SystemVariable>>> = OnceLock::new();
    VARIABLES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 按名称注册（覆盖）系统变量。
pub fn register_system_variable(variable: SystemVariable) {
    if let Ok(mut registry) = variables().write() {
        registry.insert(variable.name.clone(), variable);
    }
}

/// 按名称查找已注册系统变量。
pub fn get_system_variable(name: &str) -> Option<SystemVariable> {
    variables().read().ok()?.get(name).cloned()
}

/// 示例会话中的语句上下文（原文 SQL、摘要、涉及表）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatementContext {
    pub original_sql: String,
    pub digest: String,
    pub tables: Vec<String>,
}

/// 示例插件本地会话视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionVars {
    pub status: u16,
    pub statement: StatementContext,
    pub user: String,
}

/// 示例插件本地连接信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub user: String,
    pub host: String,
    pub database: String,
    pub connection_type: String,
}

/// 插件校验钩子：示例仅打印日志。
pub fn validate(_context: &Context, _manifest: &Manifest) -> Result<(), PluginError> {
    println!("## conn_ip_example Validate called ##");
    Ok(())
}

/// 初始化：注册示例系统变量并将连接计数清零。
pub fn on_init(context: &Context, _manifest: &Manifest) -> Result<(), PluginError> {
    println!("## conn_ip_example OnInit called ##");
    let variable = SystemVariable {
        name: SYSTEM_VARIABLE_NAME.into(),
        scope: SCOPE_GLOBAL | SCOPE_SESSION,
        value: "v1".into(),
        validation: Some(Arc::new(|_, normalized, _, _| {
            println!("The validation function was called");
            Ok(normalized.to_ascii_lowercase())
        })),
        set_session: Some(Arc::new(|_, _| {
            println!("The set session function was called");
            Ok(())
        })),
        set_global: Some(Arc::new(|_, _, _| {
            println!("The set global function was called");
            Ok(())
        })),
    };
    register_system_variable(variable);
    // 读回刚注册的配置，演示 init 阶段可读。
    if let Some(variable) = get_system_variable(SYSTEM_VARIABLE_NAME) {
        println!(
            "---- read cfg in init [key: {SYSTEM_VARIABLE_NAME}, value: {}]",
            variable.value
        );
    }
    let _ = context;
    CONNECTIONS.swap(0, Ordering::SeqCst);
    Ok(())
}

/// 关闭：打印当前变量值并清零连接计数。
pub fn on_shutdown(context: &Context, _manifest: &Manifest) -> Result<(), PluginError> {
    println!("## conn_ip_example OnShutdown called ##");
    if let Some(variable) = get_system_variable(SYSTEM_VARIABLE_NAME) {
        println!(
            "---- read cfg in shutdown [key: {SYSTEM_VARIABLE_NAME}, value: {}]",
            variable.value
        );
    }
    let _ = context;
    CONNECTIONS.swap(0, Ordering::SeqCst);
    Ok(())
}

/// 语句通用事件：打印会话状态、SQL、表与用户，以及 Starting/Completed/Error。
pub fn on_general_event(
    _context: &Context,
    session: Option<&SessionVars>,
    event: GeneralEvent,
    command: &str,
) {
    println!("## conn_ip_example OnGeneralEvent called ##");
    if let Some(session) = session {
        println!("---- session status: {}", session.status);
        println!(
            "---- statement sql: {}, digest: {}",
            session.statement.original_sql, session.statement.digest
        );
        if !session.statement.tables.is_empty() {
            println!("---- statement tables: {:?}", session.statement.tables);
        }
        println!("---- executed by user: {:?}", session.user);
    }
    match event {
        GeneralEvent::Starting => println!("---- event: Statement Starting"),
        GeneralEvent::Completed => println!("---- event: Statement Completed"),
        GeneralEvent::Error => println!("---- event: ERROR!"),
    }
    println!("---- cmd: {command}");
}

/// 连接事件：打印事件类型、拒绝原因与连接详情，并递增计数。
pub fn on_connection_event(
    context: &Context,
    event: ConnectionEvent,
    info: &ConnectionInfo,
) -> Result<(), PluginError> {
    let reject_reason = rejection_reason(context);
    println!("## conn_ip_example onConnectionEvent called ##");
    println!("---- conenct event: {event}, reason: [{reject_reason}]");
    println!("---- connection host: {}", info.host);
    println!(
        "---- connection details: {}@{}/{} type: {}",
        info.user, info.host, info.database, info.connection_type
    );
    CONNECTIONS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// 读取 Go `ctx.Value(plugin.RejectReasonCtxValue{})` 对应的拒绝原因。
pub(crate) fn rejection_reason(context: &Context) -> String {
    context
        .value(RejectReasonContextKey)
        .as_deref()
        .cloned()
        .unwrap_or_default()
}

/// 返回已观察到的连接事件次数。
pub fn connection_count() -> i32 {
    CONNECTIONS.load(Ordering::SeqCst)
}

/// Returns the real audit manifest used by the plugin framework.
/// 构造框架加载用的 AuditManifest，并适配插件 SessionVars / ConnectionInfo。
pub fn plugin_manifest() -> AuditManifest {
    let mut manifest = Manifest::new(Kind::Audit, "conn_ip_example", 1);
    manifest.validate = Some(Arc::new(validate));
    manifest.on_init = Some(Arc::new(on_init));
    manifest.on_shutdown = Some(Arc::new(on_shutdown));
    AuditManifest {
        manifest,
        on_general_event: Some(Arc::new(|context, session, event, command| {
            // 将框架 SessionVars 映射为示例本地视图（表名拼成 db.table）。
            let adapted = session.map(|s| SessionVars {
                status: s.status,
                statement: StatementContext {
                    original_sql: s.original_sql.clone(),
                    digest: String::new(),
                    tables: s
                        .tables
                        .iter()
                        .map(|t| format!("{}.{}", t.db, t.table))
                        .collect(),
                },
                user: if s.user.is_empty() {
                    format!("connection:{}", s.connection_id)
                } else {
                    s.user.clone()
                },
            });
            on_general_event(context, adapted.as_ref(), event, command);
        })),
        on_connection_event: Some(Arc::new(|context, event, info| {
            on_connection_event(
                context,
                event,
                &ConnectionInfo {
                    user: info.user.clone(),
                    host: info.host.clone(),
                    database: info.database.clone(),
                    connection_type: info.connection_type.clone(),
                },
            )
        })),
        on_global_variable_event: None,
        on_parse_event: None,
    }
}

/// 构造带指定 connection_id 的框架 SessionVars（测试辅助）。
pub fn plugin_session(connection_id: u64) -> PluginSessionVars {
    PluginSessionVars {
        connection_id,
        ..PluginSessionVars::default()
    }
}

/// 构造带 user/host 的框架 ConnectionInfo（测试辅助）。
pub fn plugin_connection(user: impl Into<String>, host: impl Into<String>) -> PluginConnectionInfo {
    PluginConnectionInfo {
        user: user.into(),
        host: host.into(),
        ..PluginConnectionInfo::default()
    }
}
