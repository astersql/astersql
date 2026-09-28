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

// etcd session 创建与取消上下文。
//
// 对应 Go `pkg/util/etcd`：在取消/截止期限下重试建立 session，供 owner 选举等组件使用。
// etcd 是分布式键值存储，常作元数据与租约（lease）协调。

#![allow(non_snake_case, non_upper_case_globals)]

use anyhow::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// 创建 session 失败后的休眠间隔。
pub const newSessionRetryInterval: Duration = Duration::from_millis(200);
/// 每隔多少次失败打一次 warn 日志，避免刷屏。
pub const logIntervalCnt: i32 = 15;
/// 默认重试次数。
pub const NewSessionDefaultRetryCnt: i32 = 3;
/// 表示不限制重试次数（用 i64::MAX 近似）。
pub const NewSessionRetryUnlimited: i64 = i64::MAX;

#[derive(Clone, Debug, Default)]
/// 可取消且可带截止期限的上下文，模拟 Go `context.Context` 的常用子集。
pub struct CancellationContext {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl CancellationContext {
    /// 无截止期限的可取消上下文。
    pub fn new() -> Self {
        Self::default()
    }

    /// 带绝对截止时间的上下文；到期后 `error` 返回 DeadlineExceeded。
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Some(deadline),
        }
    }

    /// 标记已取消；之后 `error` 返回 ContextCanceled。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 若已取消或超过截止期限则返回对应错误，否则 None。
    pub fn error(&self) -> Option<Error> {
        if self.cancelled.load(Ordering::Acquire) {
            return Some(Error::new(ContextCanceled));
        }
        self.deadline
            .filter(|deadline| Instant::now() >= *deadline)
            .map(|_| Error::new(DeadlineExceeded))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("context canceled")]
/// 上下文被取消。
pub struct ContextCanceled;

#[derive(Debug, thiserror::Error)]
#[error("context deadline exceeded")]
/// 上下文截止期限已过。
pub struct DeadlineExceeded;

#[derive(Debug, thiserror::Error)]
#[error("client connection is closing")]
/// etcd 客户端连接正在关闭，视为不可再重试的终止条件。
pub struct ClientConnectionClosing;

/// 创建 etcd session 的工厂抽象，便于测试注入。
pub trait SessionFactory {
    type Session;

    fn new_session(&mut self, ctx: &CancellationContext, ttl: i32) -> Result<Self::Session, Error>;
}

// NewSession retries exactly as the Go implementation: check context before
// every attempt, log periodically, sleep after each failed attempt, and return
// the last error when the retry budget is exhausted.
/// 按 Go 语义重试创建 etcd session：每次尝试前检查上下文，周期性打日志，失败后休眠，耗尽预算后返回最后错误。
pub fn NewSession<F: SessionFactory>(
    ctx: &CancellationContext,
    logPrefix: &str,
    factory: &mut F,
    retryCnt: i64,
    ttl: i32,
) -> Result<Option<F::Session>, Error> {
    let mut last_error = None;
    for (failed_count, _) in (0..retryCnt).enumerate() {
        if let Some(error) = contextDone(ctx, last_error.as_ref()) {
            return Err(error);
        }

        let started = Instant::now();
        match factory.new_session(ctx, ttl) {
            Ok(session) => {
                log::debug!(
                    "new etcd session established for {logPrefix} in {:?}",
                    started.elapsed()
                );
                return Ok(Some(session));
            }
            Err(error) => last_error = Some(error),
        }

        if failed_count % logIntervalCnt as usize == 0 {
            log::warn!(
                "failed to establish new session to etcd; ownerInfo={logPrefix}; error={}",
                last_error.as_ref().expect("failed attempt has an error")
            );
        }
        thread::sleep(newSessionRetryInterval);
    }

    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// 若上下文已结束，或上次错误是取消/超时/连接关闭，则立即返回，不再继续重试。
fn contextDone(ctx: &CancellationContext, err: Option<&Error>) -> Option<Error> {
    if let Some(error) = ctx.error() {
        return Some(error);
    }
    let error = err?;
    if error.is::<ContextCanceled>() {
        return Some(Error::new(ContextCanceled));
    }
    if error.is::<DeadlineExceeded>() {
        return Some(Error::new(DeadlineExceeded));
    }
    if error.is::<ClientConnectionClosing>() {
        return Some(Error::new(ClientConnectionClosing));
    }
    None
}

/// 将 lease ID 格式化为 16 位十六进制字符串。
pub fn FormatLeaseID(id: i64) -> String {
    format!("{id:016x}")
}
