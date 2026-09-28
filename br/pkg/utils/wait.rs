// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Polling wait helper ported from `br/pkg/utils/wait.go`.
//!
//! 轮询等待条件成立：间隔检查、上下文取消与总超时三路退出。
//! 超时错误优先于继续 sleep；取消与 Go `context.Canceled` 对齐。

use std::time::{Duration, Instant};

use crate::stubs::context::Context;
use astersql_errors::SharedError;

/// Polls `condition` every `check_interval` until it returns true, the parent token is cancelled,
/// or `max_timeout` elapses.
///
/// 入口先同步探测一次，已满足则零等待返回。
/// 超时瞬间若上下文也取消，优先返回 Canceled（对齐 Go 选择）。
pub fn WaitUntil(
    ctx: &Context,
    mut condition: impl FnMut() -> bool,
    check_interval: Duration,
    max_timeout: Duration,
) -> Result<(), SharedError> {
    // 快路径：避免无谓进入循环与 sleep。
    if condition() {
        return Ok(());
    }

    assert!(
        !check_interval.is_zero(),
        "non-positive interval for NewTicker"
    );

    let started = Instant::now();
    let deadline = started + max_timeout;
    let mut next_tick = started + check_interval;
    let mut buffered_tick = false;
    loop {
        if ctx.is_cancelled() {
            return Err(SharedError::new(astersql_br_pkg_errors::Canceled));
        }
        if Instant::now() >= deadline {
            // 超时边界再读一次取消，避免竞态下误报 TimedOut。
            if ctx.is_cancelled() {
                return Err(SharedError::new(astersql_br_pkg_errors::Canceled));
            }
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("waitUntil timed out after waiting for {max_timeout:?}"),
            )));
        }
        let wake_at = if buffered_tick {
            Instant::now()
        } else {
            next_tick
        }
        .min(deadline);
        let wait = wake_at.saturating_duration_since(Instant::now());
        if ctx.wait_cancelled_timeout(wait) {
            return Err(SharedError::new(astersql_br_pkg_errors::Canceled));
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("waitUntil timed out after waiting for {max_timeout:?}"),
            )));
        }
        if buffered_tick || now >= next_tick {
            if buffered_tick {
                buffered_tick = false;
            } else {
                next_tick += check_interval;
            }
            if condition() {
                return Ok(());
            }

            // time.Ticker keeps at most one unread tick and drops the rest. Preserve one
            // immediate check, then resume at the original fixed-rate cadence.
            let after_check = Instant::now();
            if after_check >= next_tick {
                buffered_tick = true;
                let overdue = after_check.duration_since(next_tick);
                let remainder_nanos = overdue.as_nanos() % check_interval.as_nanos();
                let remainder = Duration::new(
                    (remainder_nanos / 1_000_000_000) as u64,
                    (remainder_nanos % 1_000_000_000) as u32,
                );
                let until_next = if remainder.is_zero() {
                    check_interval
                } else {
                    check_interval - remainder
                };
                next_tick = after_check + until_next;
            }
        }
    }
}
