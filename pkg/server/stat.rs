// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Server 状态变量：SSL 证书有效期与进程 Uptime。
//
// 对应 SHOW STATUS 等接口所需的服务端指标；TLS 元数据不可用时保持
// 空字符串，Domain 时间不可用时 Uptime 为 0，不阻塞其余变量返回。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::server::Server;

/// SSL 服务器证书“不晚于”时间状态名。
pub const SSL_SERVER_NOT_AFTER: &str = "Ssl_server_not_after";
/// SSL 服务器证书“不早于”时间状态名。
pub const SSL_SERVER_NOT_BEFORE: &str = "Ssl_server_not_before";
/// 服务器已运行秒数状态名。
pub const UPTIME: &str = "Uptime";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 状态变量可见范围：全局、会话或两者。
pub enum StatusScope {
    Global,
    Session,
    GlobalAndSession,
}

#[derive(Clone, Debug, PartialEq)]
/// 状态变量取值：字符串或整数。
pub enum StatusValue {
    String(String),
    Integer(i64),
}

impl Server {
    /// 与 Go `GetScope` 一致，所有状态名均返回默认的全局与会话作用域。
    pub fn status_scope(&self, _name: &str) -> StatusScope {
        StatusScope::GlobalAndSession
    }

    /// Server status variables preserve the Go failure policy: unavailable TLS
    /// metadata remains an empty string and unavailable domain time produces
    /// uptime zero; neither prevents the remaining variables being returned.
    /// 汇总服务器状态变量（TLS 日期与 Uptime）。
    pub fn statistics(&self) -> HashMap<String, StatusValue> {
        // 缺省空串/0，与 Go 失败策略一致，后续按需覆盖。
        let mut values = HashMap::from([
            (
                SSL_SERVER_NOT_AFTER.into(),
                StatusValue::String(String::new()),
            ),
            (
                SSL_SERVER_NOT_BEFORE.into(),
                StatusValue::String(String::new()),
            ),
            (UPTIME.into(), StatusValue::Integer(0)),
        ]);

        // 有 TLS 配置时填入证书有效期；旧的 unix 字段保留为兼容回退。
        if let Some(tls) = self.tls_config() {
            if let Some(not_after) = tls.not_after.as_ref() {
                values.insert(
                    SSL_SERVER_NOT_AFTER.into(),
                    StatusValue::String(not_after.clone()),
                );
            } else if let Some(not_after) = tls.not_after_unix {
                values.insert(
                    SSL_SERVER_NOT_AFTER.into(),
                    StatusValue::String(not_after.to_string()),
                );
            }
            if let Some(not_before) = tls.not_before.as_ref() {
                values.insert(
                    SSL_SERVER_NOT_BEFORE.into(),
                    StatusValue::String(not_before.clone()),
                );
            } else if let Some(not_before) = tls.not_before_unix {
                values.insert(
                    SSL_SERVER_NOT_BEFORE.into(),
                    StatusValue::String(not_before.to_string()),
                );
            }
        }

        // Uptime = 当前 unix 秒 − Domain 启动时间戳。
        if let Some(domain) = self.domain() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs() as i64);
            values.insert(
                UPTIME.into(),
                StatusValue::Integer(now.saturating_sub(domain.start_timestamp())),
            );
        }
        values
    }
}
