// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Global-kill test helpers: poll PD / TiKV / TiDB HTTP status endpoints.
//! Mirrors Go `tests/globalkilltest/util.go`.

// 本文件对应 `tests/globalkilltest/util.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use crate::stubs::{Error, Result};
use crate::stubs::{decode_pd_health, errors, log, server, util};
use std::time::{Duration, Instant};

// `TIMEOUT_CHECK_PD_STATUS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const TIMEOUT_CHECK_PD_STATUS: Duration = Duration::from_secs(10);
// `TIMEOUT_CHECK_TIKV_STATUS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const TIMEOUT_CHECK_TIKV_STATUS: Duration = Duration::from_secs(30);
/// First start up of TiDB would take a long time.
// `TIMEOUT_CHECK_TIDB_STATUS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const TIMEOUT_CHECK_TIDB_STATUS: Duration = Duration::from_secs(60);
// `RETRY_INTERVAL` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const RETRY_INTERVAL: Duration = Duration::from_millis(500);

// `pd_timeout` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn pd_timeout() -> Duration {
    util::timeout_overrides()
        .pd
        .unwrap_or(TIMEOUT_CHECK_PD_STATUS)
}

// `tikv_timeout` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn tikv_timeout() -> Duration {
    util::timeout_overrides()
        .tikv
        .unwrap_or(TIMEOUT_CHECK_TIKV_STATUS)
}

// `tidb_timeout` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn tidb_timeout() -> Duration {
    util::timeout_overrides()
        .tidb
        .unwrap_or(TIMEOUT_CHECK_TIDB_STATUS)
}

// `retry_interval` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn retry_interval() -> Duration {
    util::timeout_overrides()
        .retry_interval
        .unwrap_or(RETRY_INTERVAL)
}

/// Go `withRetry[T any](fn, timeout)` — retry until success or timeout; return last error.
// `with_retry` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn with_retry<T, F>(mut f: F, timeout: Duration) -> Result<T>
where
    T: Default,
    F: FnMut() -> Result<T>,
{
    let start_time = Instant::now();
    let mut retry = 0i32;
    let mut last_err: Option<Error> = None;

    while start_time.elapsed() < timeout {
        retry += 1;
        match f() {
            Ok(resp) => return Ok(resp),
            Err(err) => {
                log::Debug("withRetry", Some(&err), retry);
                last_err = Some(err);
                std::thread::sleep(retry_interval());
            }
        }
    }

    match last_err {
        Some(err) => Err(errors::Trace(err)),
        None => Ok(T::default()),
    }
}

/// PD `/health` JSON body (`health` string field).
#[derive(Clone, Debug, PartialEq, Eq)]
// `PdHealth` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct PdHealth {
    pub health: String,
}

/// Go `checkPDHealth(host)` — HTTP 200 and `health == "true"`.
// `check_pd_health` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn check_pd_health(host: &str) -> Result<()> {
    let url = util::ComposeURL(host, "/health");

    let request = || -> Result<()> {
        let resp = util::InternalHTTPClient()
            .Get(&url)
            .map_err(errors::Trace)?;

        if resp.StatusCode != util::StatusOK {
            return Err(errors::Errorf(format!(
                "PD health status code {}",
                resp.StatusCode
            )));
        }
        // Go: defer resp.Body.Close()
        let _body_guard = BodyGuard(&resp.Body);

        let health_val = decode_pd_health(resp.Body.as_slice()).map_err(errors::Trace)?;
        let health = PdHealth { health: health_val };

        log::Info("PD health", &format!("{health:?}"));
        if health.health != "true" {
            return Err(errors::Errorf(format!("PD not healthy {}", health.health)));
        }
        Ok(())
    };

    with_retry(request, pd_timeout()).map_err(errors::Trace)
}

/// Go `checkTiKVStatus()` — fixed `127.0.0.1:20180/status`, HTTP 200 only.
// `check_tikv_status` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn check_tikv_status() -> Result<()> {
    let url = util::ComposeURL("127.0.0.1:20180", "/status");

    let request = || -> Result<()> {
        let resp = util::InternalHTTPClient()
            .Get(&url)
            .map_err(errors::Trace)?;

        log::Info("TiKV status", &format!("{}", resp.StatusCode));
        if resp.StatusCode != util::StatusOK {
            return Err(errors::Errorf(format!(
                "TiKV status code {}",
                resp.StatusCode
            )));
        }
        // Go: resp.Body.Close()
        resp.Body.Close();
        Ok(())
    };

    with_retry(request, tikv_timeout()).map_err(errors::Trace)
}

/// Go `checkTiDBStatus(statusPort)` — decode `server.Status` from `/status`.
// `check_tidb_status` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn check_tidb_status(status_port: i32) -> Result<()> {
    let host = format!("127.0.0.1:{status_port}");
    let url = util::ComposeURL(&host, "/status");

    let request = || -> Result<()> {
        let resp = util::InternalHTTPClient()
            .Get(&url)
            .map_err(errors::Trace)?;

        if resp.StatusCode != util::StatusOK {
            return Err(errors::Errorf(format!(
                "TiDB status code {}",
                resp.StatusCode
            )));
        }
        // Go: defer resp.Body.Close()
        let _body_guard = BodyGuard(&resp.Body);

        let status = server::decode_status(resp.Body.as_slice()).map_err(errors::Trace)?;

        log::Info("TiDB status", &format!("{status:?}"));
        Ok(())
    };

    with_retry(request, tidb_timeout()).map_err(errors::Trace)
}

/// Ensures response body is closed on drop (Go `defer resp.Body.Close()`).
// `BodyGuard` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct BodyGuard<'a>(&'a util::Body);

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Drop for BodyGuard<'_> {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn drop(&mut self) {
        self.0.Close();
    }
}

#[cfg(test)]
// `default_timeouts` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
pub(crate) fn default_timeouts() -> (Duration, Duration, Duration, Duration) {
    (
        TIMEOUT_CHECK_PD_STATUS,
        TIMEOUT_CHECK_TIKV_STATUS,
        TIMEOUT_CHECK_TIDB_STATUS,
        RETRY_INTERVAL,
    )
}
