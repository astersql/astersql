// Copyright 2015 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 状态变量（status variable）注册与聚合，对应 Go `statusvar.go`。
//
// 状态变量供 `SHOW STATUS` 等接口读取运行时指标；本模块维护提供者列表，
// 聚合 Scope（作用域：Global/Session）与值，并实现默认 SSL / Performance Schema /
// `tidb_keys_examined` 等内置状态。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::sync::{Arc, LazyLock, RwLock};

use crate::{tlsutil, vardef};

/// 默认状态变量作用域：同时可见于 Global 与 Session。
pub static DefaultStatusVarScopeFlag: LazyLock<vardef::ScopeFlag> =
    LazyLock::new(|| vardef::ScopeGlobal | vardef::ScopeSession);

/// 存入 `StatusValue` 的类型擦除后端：支持 Any 下转型与 Debug/Display。
trait StoredStatusValue: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn fmt_debug(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn fmt_display(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl<T> StoredStatusValue for T
where
    T: Any + Debug + Display + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_debug(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, formatter)
    }

    fn fmt_display(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, formatter)
    }
}

/// Rust's type-safe counterpart of the scalar values stored in Go's `any`.
///
/// Go `any` 的类型安全对应：用 `Arc<dyn StoredStatusValue>` 擦除具体标量类型。
#[derive(Clone)]
pub struct StatusValue(Arc<dyn StoredStatusValue>);

impl StatusValue {
    /// 包装任意可 Debug+Display 的值。
    pub fn new<T>(value: T) -> Self
    where
        T: Any + Debug + Display + Send + Sync,
    {
        Self(Arc::new(value))
    }

    /// 尝试下转型为具体类型 `T`。
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref::<T>()
    }
}

impl Debug for StatusValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt_debug(formatter)
    }
}

impl Display for StatusValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt_display(formatter)
    }
}

/// StatusVal is the value and scope of one status variable.
///
/// 单个状态变量的作用域与取值。
#[derive(Clone, Debug)]
pub struct StatusVal {
    /// 变量作用域（Global / Session 等位图）。
    pub Scope: vardef::ScopeFlag,
    /// 当前取值。
    pub Value: StatusValue,
}

/// Only the fields read by this Go source file are carried through the focused
/// file-group boundary. The package integration task can map the full session
/// object to this view without changing status-variable behavior.
///
/// 本文件所需的 TLS 连接状态精简视图（密码套件与协议版本）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TLSConnectionState {
    /// TLS CipherSuite 数字编号。
    pub CipherSuite: u16,
    /// TLS 协议版本编号。
    pub Version: u16,
}

/// 本模块聚合状态时读取的会话侧字段子集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionVars {
    /// 本会话已检查的 key 数量（对应 `tidb_keys_examined`）。
    pub KeysExamined: u64,
    /// 可选的 TLS 握手结果，用于填充 Ssl_* 状态。
    pub TLSConnectionState: Option<TLSConnectionState>,
}

/// 统计提供者返回的错误类型别名。
pub type StatisticsError = Box<dyn Error + Send + Sync + 'static>;

/// Statistics supplies a group of status variables, matching the Go interface.
///
/// 一组状态变量的提供者接口，与 Go `Statistics` 对齐。
pub trait Statistics: Send + Sync {
    /// 返回指定状态名的作用域。
    fn GetScope(&self, status: &str) -> vardef::ScopeFlag;
    /// 收集当前状态名→值映射；`vars` 为可选会话上下文。
    fn Stats(
        &self,
        vars: Option<&SessionVars>,
    ) -> Result<HashMap<String, StatusValue>, StatisticsError>;
}

/// 已注册统计提供者的共享句柄。
pub type StatisticsHandle = Arc<dyn Statistics>;

/// 全局注册表：默认包含 `DefaultStatusStat`。
static STATISTICS_LIST: LazyLock<RwLock<Vec<StatisticsHandle>>> =
    LazyLock::new(|| RwLock::new(vec![Arc::new(DefaultStatusStat) as StatisticsHandle]));

/// RegisterStatistics appends a provider in registration order.
///
/// 按注册顺序追加一个统计提供者。
pub fn RegisterStatistics(statistics: StatisticsHandle) {
    STATISTICS_LIST
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(statistics);
}

/// UnregisterStatistics removes the last matching registration and fills its
/// slot with the last element, exactly like the Go implementation.
///
/// 移除最后一次匹配的注册项，并用末尾元素填补空位（与 Go `swap` 删除一致）。
pub fn UnregisterStatistics(statistics: &StatisticsHandle) {
    let mut registered = STATISTICS_LIST
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(index) = registered
        .iter()
        .rposition(|candidate| Arc::ptr_eq(candidate, statistics))
    {
        registered.swap_remove(index);
    }
}

/// GetStatusVars aggregates registered providers under the registry read lock.
/// A provider error stops iteration and is returned without a partial result.
///
/// 在读锁下聚合全部提供者；任一提供者出错则中止并返回错误（无部分结果）。
pub fn GetStatusVars(
    vars: Option<&SessionVars>,
) -> Result<HashMap<String, StatusVal>, StatisticsError> {
    let mut status_vars = HashMap::new();
    let registered = STATISTICS_LIST
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    for statistics in registered.iter() {
        for (name, value) in statistics.Stats(vars)? {
            let scope = statistics.GetScope(&name);
            status_vars.insert(
                name,
                StatusVal {
                    Scope: scope,
                    Value: value,
                },
            );
        }
    }

    Ok(status_vars)
}

// Taken from Go's crypto/tls constants, in statusvar.go declaration order.
// 取自 Go crypto/tls 常量，顺序与 statusvar.go 声明一致。
const TLS_CIPHERS: [u16; 25] = [
    0x0005, 0x000a, 0x002f, 0x0035, 0x003c, 0x009c, 0x009d, 0xc007, 0xc009, 0xc00a, 0xc011, 0xc012,
    0xc013, 0xc014, 0xc023, 0xc027, 0xc02f, 0xc02b, 0xc030, 0xc02c, 0xcca8, 0xcca9, 0x1301, 0x1302,
    0x1303,
];

/// 支持的 TLS 密码套件名称列表（以 `:` 拼接，末尾保留冒号）。
pub static TLS_SUPPORTED_CIPHERS: LazyLock<String> = LazyLock::new(|| {
    let mut supported = String::new();
    for cipher in TLS_CIPHERS {
        supported.push_str(&tlsutil::CipherSuiteName(cipher));
        supported.push(':');
    }
    supported
});

/// 默认内置状态变量的静态初值表。
static DEFAULT_STATUS: LazyLock<HashMap<&'static str, StatusVal>> = LazyLock::new(|| {
    HashMap::from([
        (
            "Ssl_cipher",
            StatusVal {
                Scope: *DefaultStatusVarScopeFlag,
                Value: StatusValue::new(""),
            },
        ),
        (
            "Ssl_cipher_list",
            StatusVal {
                Scope: *DefaultStatusVarScopeFlag,
                Value: StatusValue::new(""),
            },
        ),
        (
            "Ssl_verify_mode",
            StatusVal {
                Scope: *DefaultStatusVarScopeFlag,
                Value: StatusValue::new(0_i32),
            },
        ),
        (
            "Ssl_version",
            StatusVal {
                Scope: *DefaultStatusVarScopeFlag,
                Value: StatusValue::new(""),
            },
        ),
        (
            "Performance_schema_session_connect_attrs_longest_seen",
            StatusVal {
                Scope: vardef::ScopeGlobal,
                Value: StatusValue::new(0_i64),
            },
        ),
        (
            "Performance_schema_session_connect_attrs_lost",
            StatusVal {
                Scope: vardef::ScopeGlobal,
                Value: StatusValue::new(0_i64),
            },
        ),
        (
            "tidb_keys_examined",
            StatusVal {
                Scope: vardef::ScopeSession,
                Value: StatusValue::new(0_u64),
            },
        ),
    ])
});

/// 默认统计提供者：填充内置状态，并按会话注入实时计数与 TLS 字段。
struct DefaultStatusStat;

impl Statistics for DefaultStatusStat {
    fn GetScope(&self, status: &str) -> vardef::ScopeFlag {
        DEFAULT_STATUS
            .get(status)
            .unwrap_or_else(|| panic!("unknown default status variable: {status}"))
            .Scope
    }

    fn Stats(
        &self,
        vars: Option<&SessionVars>,
    ) -> Result<HashMap<String, StatusValue>, StatisticsError> {
        // 先克隆默认初值，再覆盖实时计数器与会话相关字段。
        let mut status_vars: HashMap<String, StatusValue> = DEFAULT_STATUS
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.Value.clone()))
            .collect();

        status_vars.insert(
            "Performance_schema_session_connect_attrs_longest_seen".to_owned(),
            StatusValue::new(vardef::ConnectAttrsLongestSeen.Load()),
        );
        status_vars.insert(
            "Performance_schema_session_connect_attrs_lost".to_owned(),
            StatusValue::new(vardef::ConnectAttrsLost.Load()),
        );

        if let Some(vars) = vars {
            status_vars.insert(
                "tidb_keys_examined".to_owned(),
                StatusValue::new(vars.KeysExamined),
            );
            if let Some(tls_state) = vars.TLSConnectionState {
                status_vars.insert(
                    "Ssl_cipher".to_owned(),
                    StatusValue::new(tlsutil::CipherSuiteName(tls_state.CipherSuite)),
                );
                status_vars.insert(
                    "Ssl_cipher_list".to_owned(),
                    StatusValue::new(TLS_SUPPORTED_CIPHERS.clone()),
                );
                // tls.VerifyClientCertIfGiven == SSL_VERIFY_PEER | SSL_VERIFY_CLIENT_ONCE.
                status_vars.insert("Ssl_verify_mode".to_owned(), StatusValue::new(0x01 | 0x04));
                status_vars.insert(
                    "Ssl_version".to_owned(),
                    StatusValue::new(tlsutil::VersionName(tls_state.Version)),
                );
            }
        }

        Ok(status_vars)
    }
}
