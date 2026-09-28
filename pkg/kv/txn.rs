// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 内部事务辅助：在新事务中执行回调、重试与长事务监控。
//
// 提供 `RunInNewTxn`：开启事务、注入请求来源、执行用户回调、提交；
// 遇到可重试错误（TxnRetryable，通常由锁冲突或写冲突触发）时按指数退避重试。
// 同时维护内部事务起始时间戳集合，用于 SafeTS / GC 相关的最小 startTS 计算，
// 并对运行过久的内部事务打日志。

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::Rng;

use crate::{
    Context, ErrTxnRetryable, Error, ExplicitRequestSourceType, IsTxnRetryableError,
    RPCInterceptor, RequestKind, RequestSourceInternal, RequestSourceType, ResourceGroupName,
    RpcInterceptor, Storage, Transaction, errors,
};

/// 内部事务持续时间超过该阈值时打印告警日志（默认 5 分钟）。
pub const TimeToPrintLongTimeInternalTxn: Duration = Duration::from_secs(5 * 60);

/// 全局内部事务 startTS 登记表。
static globalInnerTxnTsBox: LazyLock<InnerTxnStartTsBox> = LazyLock::new(InnerTxnStartTsBox::new);

/// 跟踪当前存活的内部事务起始时间戳集合。
struct InnerTxnStartTsBox {
    innerTxnStartTsMap: Mutex<HashSet<u64>>,
}

impl InnerTxnStartTsBox {
    fn new() -> Self {
        Self {
            innerTxnStartTsMap: Mutex::new(HashSet::with_capacity(256)),
        }
    }

    /// 登记一个内部事务的 startTS。
    fn storeInnerTxnTS(&self, start_ts: u64) {
        self.innerTxnStartTsMap
            .lock()
            .expect("inner transaction timestamp mutex poisoned")
            .insert(start_ts);
    }

    /// 移除已结束内部事务的 startTS。
    fn deleteInnerTxnTS(&self, start_ts: u64) {
        self.innerTxnStartTsMap
            .lock()
            .expect("inner transaction timestamp mutex poisoned")
            .remove(&start_ts);
    }

    /// 在下限之上扫描登记表，返回更小的最小 startTS，并对长事务打日志。
    fn getMinStartTS(
        &self,
        now: SystemTime,
        start_ts_lower_limit: u64,
        current_min_start_ts: u64,
    ) -> u64 {
        let mut min_start_ts = current_min_start_ts;
        let start_timestamps = self
            .innerTxnStartTsMap
            .lock()
            .expect("inner transaction timestamp mutex poisoned");
        for &inner_ts in start_timestamps.iter() {
            PrintLongTimeInternalTxn(now, inner_ts, true);
            if inner_ts > start_ts_lower_limit && inner_ts < min_start_ts {
                min_start_ts = inner_ts;
            }
        }
        min_start_ts
    }
}

/// 查询当前登记的内部事务中、高于下限的最小 startTS。
pub fn GetMinInnerTxnStartTS(
    now: SystemTime,
    start_ts_lower_limit: u64,
    current_min_start_ts: u64,
) -> u64 {
    globalInnerTxnTsBox.getMinStartTS(now, start_ts_lower_limit, current_min_start_ts)
}

/// 从 TiKV TSO 解析物理时间：高 46 位为毫秒时间戳，低 18 位为逻辑计数。
fn GetTimeFromTS(start_ts: u64) -> SystemTime {
    // A TiKV TSO stores Unix milliseconds in the high 46 bits and its logical
    // counter in the low 18 bits.
    UNIX_EPOCH + Duration::from_millis(start_ts >> 18)
}

/// 若内部事务已运行超过阈值，则按调用路径打印 info 日志。
pub fn PrintLongTimeInternalTxn(now: SystemTime, start_ts: u64, run_by_function: bool) {
    if start_ts == 0 {
        return;
    }
    let start_time = GetTimeFromTS(start_ts);
    let elapsed = now.duration_since(start_time).unwrap_or_default();
    if elapsed > TimeToPrintLongTimeInternalTxn {
        let caller_name = if run_by_function {
            "RunInNewTxn"
        } else {
            "internal session"
        };
        log::info!(
            "An internal transaction running by {caller_name} lasts long time; time={elapsed:?}, startTS={start_ts}, start_time={start_time:?}"
        );
    }
}

/// RAII 守卫：Drop 时从全局表删除登记的 startTS。
struct InnerTxnGuard {
    start_ts: Option<u64>,
}

impl Drop for InnerTxnGuard {
    fn drop(&mut self) {
        if let Some(start_ts) = self.start_ts {
            globalInnerTxnTsBox.deleteInnerTxnTS(start_ts);
        }
    }
}

/// 在新事务中执行 `callback`：可选地对可重试错误进行退避重试后再次 Begin/Commit。
pub fn RunInNewTxn<F>(
    ctx: &Context,
    store: &dyn Storage,
    retryable: bool,
    mut callback: F,
) -> Result<(), Error>
where
    F: FnMut(&Context, &mut dyn Transaction) -> Result<(), Error>,
{
    let mut original_txn_ts = 0;
    let mut guard = InnerTxnGuard { start_ts: None };
    let mut last_error = None;

    for attempt in 0..MaxRetryCnt.load(Ordering::Relaxed) {
        let mut txn = store.Begin(&[]).map_err(|error| {
            log::error!("RunInNewTxn: {error}");
            error
        })?;
        setRequestSourceForInnerTxn(ctx, txn.as_mut());

        // 首次成功 Begin 时登记原始 startTS，供长事务监控与重试日志使用。
        if attempt == 0 {
            original_txn_ts = txn.StartTS();
            guard.start_ts = Some(original_txn_ts);
            globalInnerTxnTsBox.storeInnerTxnTS(original_txn_ts);
        }

        if let Err(error) = callback(ctx, txn.as_mut()) {
            if let Err(rollback_error) = txn.Rollback() {
                log::warn!("RunInNewTxn rollback: {rollback_error}");
            }
            if retryable && IsTxnRetryableError(Some(&error)) {
                logRetry(txn.StartTS(), original_txn_ts, &error);
                last_error = Some(error);
                continue;
            }
            return Err(error);
        }

        // failpoint 可注入提交错误，便于测试重试路径。
        let injected_error =
            fail::eval("mockCommitErrorInNewTxn", |value| match value.as_deref() {
                Some("retry_once") if attempt == 0 => Some(ErrTxnRetryable.FastGenByArgs(&[])),
                Some("no_retry") => Some(errors::New("mock commit error")),
                _ => None,
            })
            .flatten();
        let commit_error = match injected_error {
            Some(error) => Some(error),
            None => txn.Commit(ctx).err(),
        };
        let Some(error) = commit_error else {
            return Ok(());
        };

        if retryable && IsTxnRetryableError(Some(&error)) {
            logRetry(txn.StartTS(), original_txn_ts, &error);
            last_error = Some(error);
            BackOff(attempt);
            continue;
        }
        return Err(error);
    }

    match last_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 记录一次事务重试的警告日志。
fn logRetry(retry_ts: u64, original_ts: u64, error: &Error) {
    log::warn!("RunInNewTxn: retry txn={retry_ts}, original txn={original_ts}, error={error}");
}

/// `RunInNewTxn` 最大重试次数（可运行时调整）。
pub static MaxRetryCnt: AtomicU32 = AtomicU32::new(100);
/// 指数退避基数（毫秒级随机睡眠的底数）。
static retryBackOffBase: u32 = 1;
/// 指数退避上限（毫秒）。
static retryBackOffCap: u32 = 100;

/// 按尝试次数做有上限的指数随机退避并睡眠，返回实际睡眠时长。
pub fn BackOff(attempts: u32) -> Duration {
    let exponential = retryBackOffBase.saturating_mul(2_u32.saturating_pow(attempts));
    let upper = retryBackOffCap.min(exponential).max(1);
    let sleep = Duration::from_millis(rand::rng().random_range(0..upper) as u64);
    std::thread::sleep(sleep);
    sleep
}

/// 将 Context 中的请求来源选项复制到内部事务上。
fn setRequestSourceForInnerTxn(ctx: &Context, txn: &mut dyn Transaction) {
    if let Some(request_source) = ctx.RequestSource() {
        if !request_source.RequestSourceType.is_empty() {
            if !request_source.RequestSourceInternal {
                log::warn!("`RunInNewTxn` should be used by inner txn only");
            }
            txn.SetOption(
                RequestSourceInternal,
                Some(Box::new(request_source.RequestSourceInternal)),
            );
            txn.SetOption(
                RequestSourceType,
                Some(Box::new(request_source.RequestSourceType.clone())),
            );
            if !request_source.ExplicitRequestSourceType.is_empty() {
                txn.SetOption(
                    ExplicitRequestSourceType,
                    Some(Box::new(request_source.ExplicitRequestSourceType.clone())),
                );
            }
            return;
        }
    }

    log::warn!(
        "unexpected no source type context, if you see this warning, the `RequestSourceTypeKey` is missing in the context"
    );
}

/// 为事务设置资源组名称，并在 failpoint 开启时挂载 RPC 拦截器做一致性校验。
pub fn SetTxnResourceGroup(txn: &mut dyn Transaction, name: String) {
    txn.SetOption(ResourceGroupName, Some(Box::new(name)));

    if let Some(expected_name) =
        fail::eval("TxnResourceGroupChecker", |value| value.unwrap_or_default())
    {
        let interceptor: RpcInterceptor = Arc::new(move |request| {
            if matches!(
                request.Kind,
                RequestKind::Prewrite | RequestKind::Commit | RequestKind::PessimisticLock
            ) {
                assert_eq!(
                    expected_name, request.ResourceGroupName,
                    "resource group name not match"
                );
            }
            Ok(())
        });
        txn.SetOption(RPCInterceptor, Some(Box::new(interceptor)));
    }
}
