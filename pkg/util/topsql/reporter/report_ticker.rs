// Copyright 2026 PingCAP, Inc.
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

// TopSQL 上报周期 ticker。
//
// 封装 crossbeam `tick`，默认间隔取自 `DefTiDBTopSQLReportIntervalSeconds`；
// 测试可通过全局锁临时覆盖间隔并在闭包中还原。

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, tick};

/// 默认上报间隔（秒），与 topsql 全局默认一致。
const DEFAULT_REPORT_TICKER_INTERVAL: Duration =
    Duration::from_secs(crate::topsql_state::DefTiDBTopSQLReportIntervalSeconds as u64);

/// 进程内可覆盖的上报间隔；测试用 `set_report_ticker_interval_seconds_for_test` 修改。
static REPORT_TICKER_INTERVAL: LazyLock<Mutex<Duration>> =
    LazyLock::new(|| Mutex::new(DEFAULT_REPORT_TICKER_INTERVAL));

/// 带固定间隔的接收端 ticker，供 reporter 周期触发上报。
pub struct ReportTicker {
    interval: Duration,
    receiver: Receiver<Instant>,
}

impl ReportTicker {
    /// 当前 ticker 间隔。
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// 阻塞等待下一次 tick。
    pub fn recv(&self) -> Result<Instant, crossbeam_channel::RecvError> {
        self.receiver.recv()
    }

    /// 带超时等待下一次 tick。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Instant, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

/// 按当前全局间隔创建新的 ReportTicker。
pub fn new_report_ticker() -> ReportTicker {
    let interval = *REPORT_TICKER_INTERVAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    ReportTicker {
        interval,
        receiver: tick(interval),
    }
}

/// 测试辅助：临时设置间隔秒数，返回还原闭包；`seconds <= 0` 时恢复默认。
pub fn set_report_ticker_interval_seconds_for_test(
    seconds: i64,
) -> Box<dyn FnOnce() + Send + 'static> {
    let interval = if seconds > 0 {
        Duration::from_secs(seconds as u64)
    } else {
        DEFAULT_REPORT_TICKER_INTERVAL
    };
    // 替换全局间隔并捕获旧值，供还原闭包写回。
    let previous = {
        let mut current = REPORT_TICKER_INTERVAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        std::mem::replace(&mut *current, interval)
    };
    Box::new(move || {
        *REPORT_TICKER_INTERVAL
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = previous;
    })
}

/// Go 风格导出名，委托给 `set_report_ticker_interval_seconds_for_test`。
#[allow(non_snake_case)]
pub fn SetReportTickerIntervalSecondsForTest(seconds: i64) -> Box<dyn FnOnce() + Send + 'static> {
    set_report_ticker_interval_seconds_for_test(seconds)
}
