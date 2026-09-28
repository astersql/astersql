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

// KV store driver 的退避（backoff）实现。
//
// 在访问 TiKV 时，Region 路由失效、事务锁冲突等瞬时错误会触发按配置等待后重试。
// 本模块提供与 client-go 兼容的 Context、Jitter、BackoffConfig、TiKvBackoffer 与
// Backoffer，并在达到总睡眠上限或会话被杀死/取消时停止重试。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use crate::driver_error as derr;
use crate::errors::SharedError;
use crate::kv;
use tokio_util::sync::CancellationToken;

/// 无会话变量时使用的默认 Killed 标志（0 表示未中断）。
static DEFAULT_KILLED: AtomicU32 = AtomicU32::new(0);
/// 交给官方 tikv_client::Backoff 的“不限次数”占位；实际停止由 max_sleep 控制。
const UNLIMITED_ATTEMPTS: u32 = u32::MAX;
/// client-go 最多记录最近三条退避错误。
const MAX_RECORD_BACKOFF_ERROR_COUNT: usize = 3;
/// 事务快速锁（txnLockFast）退避配置名，对应 Go client-go 同名策略。
const TXN_LOCK_FAST: &str = "txnLockFast";
/// TiKV server busy 的睡眠不计入普通上限，但排除睡眠自身最多累计十分钟。
const TIKV_SERVER_BUSY: &str = "tikvServerBusy";
const TIKV_SERVER_BUSY_MAX_EXCLUDED_SLEEP_MS: isize = 600_000;

/// Context 中由 client-go Backoffer 更新的执行统计。
#[derive(Debug, Default)]
pub struct ExecDetails {
    backoff_duration_ns: AtomicI64,
    backoff_count: AtomicI64,
}

impl ExecDetails {
    /// 返回累计退避时长（纳秒）。
    pub fn backoff_duration_ns(&self) -> i64 {
        self.backoff_duration_ns.load(Ordering::Relaxed)
    }

    /// 返回累计退避次数。
    pub fn backoff_count(&self) -> i64 {
        self.backoff_count.load(Ordering::Relaxed)
    }

    fn record_backoff(&self, sleep_ms: isize) {
        let duration_ns = i64::try_from(sleep_ms)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000_000);
        self.backoff_duration_ns
            .fetch_add(duration_ns, Ordering::Relaxed);
        self.backoff_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Cloneable cancellation context used by the synchronous client-go-compatible API.
/// 可克隆的取消上下文，供同步、与 client-go 兼容的 API 使用。
#[derive(Clone, Debug)]
pub struct Context {
    token: CancellationToken,
    identity: Arc<()>,
    exec_details: Option<Arc<ExecDetails>>,
}

impl Context {
    /// 创建未取消的新 Context。
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            identity: Arc::new(()),
            exec_details: None,
        }
    }

    /// 绑定 ExecDetails，等价于 Go context.WithValue(ctx, util.ExecDetailsKey, details)。
    pub fn with_exec_details(mut self, details: Arc<ExecDetails>) -> Self {
        self.exec_details = Some(details);
        self
    }

    /// 取消该 Context，使进行中的睡眠尽快退出。
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Eq for Context {}

/// Jitter algorithms supported by client-go and the official Rust TiKV client.
/// client-go 与官方 Rust TiKV client 支持的抖动（jitter）算法。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Jitter {
    NoJitter,
    FullJitter,
    EqualJitter,
    DecorrJitter,
}

/// Client-go-compatible configuration backed by `tikv_client::Backoff`.
/// 与 client-go 兼容的退避配置，底层由 `tikv_client::Backoff` 生成延迟。
#[derive(Clone, Debug)]
pub struct BackoffConfig {
    name: String,
    base_ms: isize,
    cap_ms: isize,
    jitter: Jitter,
    error: Option<SharedError>,
}

impl BackoffConfig {
    /// 按名称、基准毫秒、上限毫秒与抖动类型构造配置。
    pub fn new(name: impl Into<String>, base_ms: isize, cap_ms: isize, jitter: Jitter) -> Self {
        let name = name.into();
        Self {
            error: default_config_error(&name),
            name,
            base_ms,
            cap_ms,
            jitter,
        }
    }

    /// 覆盖超限时代表该退避类型返回的错误，对应 client-go Config.SetErrors。
    pub fn with_error(mut self, error: SharedError) -> Self {
        self.error = Some(error);
        self
    }

    /// 生成底层延迟调度器；可覆盖 base（用于 txnLockFast 的会话变量）。
    fn schedule(&self, base_override: Option<isize>) -> tikv_client::Backoff {
        // client-go clamps bases below two to avoid empty random ranges.
        let base_ms = base_override.unwrap_or(self.base_ms).max(2) as u64;
        let cap_ms = self.cap_ms.max(0) as u64;
        match self.jitter {
            Jitter::NoJitter => {
                tikv_client::Backoff::no_jitter_backoff(base_ms, cap_ms, UNLIMITED_ATTEMPTS)
            }
            Jitter::FullJitter => tikv_client::Backoff::full_jitter_backoff(
                base_ms,
                cap_ms.max(1),
                UNLIMITED_ATTEMPTS,
            ),
            Jitter::EqualJitter => tikv_client::Backoff::equal_jitter_backoff(
                base_ms,
                cap_ms.max(2),
                UNLIMITED_ATTEMPTS,
            ),
            Jitter::DecorrJitter => tikv_client::Backoff::decorrelated_jitter_backoff(
                base_ms,
                cap_ms,
                UNLIMITED_ATTEMPTS,
            ),
        }
    }
}

fn default_config_error(name: &str) -> Option<SharedError> {
    let error = match name {
        "tikvRPC" => derr::TiKvError::TiKVServerTimeout,
        "tiflashRPC" => derr::TiKvError::TiFlashServerTimeout,
        "txnLock" | TXN_LOCK_FAST | "txnNotFound" => derr::TiKvError::ResolveLockTimeout,
        "pdRPC" => derr::TiKvError::PdServerTimeout {
            message: String::new(),
        },
        "regionMiss" | "regionScheduling" => derr::TiKvError::RegionUnavailable,
        TIKV_SERVER_BUSY => derr::TiKvError::TiKVServerBusy,
        "tiflashServerBusy" => derr::TiKvError::TiFlashServerBusy,
        "staleCommand" => derr::TiKvError::TiKVStaleCommand,
        "maxTsNotSynced" => derr::TiKvError::TiKVMaxTimestampNotSynced,
        _ => return None,
    };
    Some(SharedError::new(error))
}

#[derive(Clone, Debug)]
pub(crate) struct BackoffError {
    pub(crate) reason: String,
    pub(crate) time: SystemTime,
}

/// 会话变量：借用外部 Variables，或持有默认静态值。
enum Variables<'a> {
    Borrowed(&'a kv::Variables<'a>),
    Default(kv::Variables<'static>),
}

impl Variables<'_> {
    fn get(&self) -> &kv::Variables<'_> {
        match self {
            Self::Borrowed(vars) => vars,
            Self::Default(vars) => vars,
        }
    }
}

/// 面向 TiKV 的退避状态：在官方延迟生成器外包一层 Context、变量、总睡眠上限与统计。
/// The TiKV-facing backoffer state. Rust's official client exposes the delay
/// generator but not client-go's context, variables, aggregate limit or stats,
/// so this adapter preserves those public semantics around the official type.
pub struct TiKvBackoffer<'a> {
    ctx: Context,
    max_sleep_ms: isize,
    total_sleep_ms: isize,
    pub(crate) excluded_sleep_ms: isize,
    vars: Variables<'a>,
    schedules: HashMap<String, tikv_client::Backoff>,
    errors: [Option<BackoffError>; MAX_RECORD_BACKOFF_ERROR_COUNT],
    errors_num: usize,
    configs: Vec<BackoffConfig>,
    backoff_sleep_ms: HashMap<String, isize>,
    backoff_times: HashMap<String, isize>,
}

impl<'a> TiKvBackoffer<'a> {
    /// 由 Context、最大睡眠、可选变量与是否应用 BackOffWeight 组装。
    fn from_parts(
        ctx: Context,
        mut max_sleep_ms: isize,
        vars: Option<&'a kv::Variables<'a>>,
        apply_weight: bool,
    ) -> Self {
        let vars = match vars {
            Some(vars) => Variables::Borrowed(vars),
            None => Variables::Default(kv::Variables {
                BackoffLockFast: kv::DefBackoffLockFast,
                BackOffWeight: kv::DefBackOffWeight,
                Killed: &DEFAULT_KILLED,
            }),
        };
        let weight = vars.get().BackOffWeight as isize;
        if apply_weight && max_sleep_ms > 0 && (i32::MAX as isize) / weight >= max_sleep_ms {
            max_sleep_ms *= weight;
        }
        Self {
            ctx,
            max_sleep_ms,
            total_sleep_ms: 0,
            excluded_sleep_ms: 0,
            vars,
            schedules: HashMap::new(),
            errors: std::array::from_fn(|_| None),
            errors_num: 0,
            configs: Vec::new(),
            backoff_sleep_ms: HashMap::new(),
            backoff_times: HashMap::new(),
        }
    }

    /// 使用默认会话变量构造（不应用 BackOffWeight 放大）。
    pub fn new(ctx: Context, max_sleep_ms: isize) -> TiKvBackoffer<'static> {
        TiKvBackoffer::from_parts(ctx, max_sleep_ms, None, false)
    }

    /// 带会话变量构造，并按 BackOffWeight 放大 max_sleep。
    fn new_with_vars(
        ctx: Context,
        max_sleep_ms: isize,
        vars: Option<&'a kv::Variables<'a>>,
    ) -> Self {
        Self::from_parts(ctx, max_sleep_ms, vars, true)
    }

    /// 返回当前最大允许累计睡眠毫秒。
    pub fn MaxSleep(&self) -> isize {
        self.max_sleep_ms
    }

    /// 分段睡眠，期间若 Context 取消则提前返回 0。
    fn sleep(&self, requested: Duration) -> isize {
        let mut remaining = requested;
        let quantum = Duration::from_millis(5);
        while !remaining.is_zero() {
            if self.ctx.is_cancelled() {
                return 0;
            }
            let current = remaining.min(quantum);
            std::thread::sleep(current);
            remaining = remaining.saturating_sub(current);
        }
        requested.as_millis().min(isize::MAX as u128) as isize
    }

    fn append_error(&mut self, error: &SharedError) {
        self.errors[self.errors_num % MAX_RECORD_BACKOFF_ERROR_COUNT] = Some(BackoffError {
            reason: error.to_string(),
            time: SystemTime::now(),
        });
        self.errors_num += 1;
    }

    pub(crate) fn latest_errors(&self) -> Vec<&BackoffError> {
        let valid_count = self.errors_num.min(MAX_RECORD_BACKOFF_ERROR_COUNT);
        if self.errors_num <= MAX_RECORD_BACKOFF_ERROR_COUNT {
            return self.errors[..valid_count]
                .iter()
                .filter_map(Option::as_ref)
                .collect();
        }

        let first = self.errors_num % MAX_RECORD_BACKOFF_ERROR_COUNT;
        (0..MAX_RECORD_BACKOFF_ERROR_COUNT)
            .filter_map(|offset| {
                self.errors[(first + offset) % MAX_RECORD_BACKOFF_ERROR_COUNT].as_ref()
            })
            .collect()
    }

    fn longest_sleep_config(&self) -> Option<&BackoffConfig> {
        let candidate = self
            .backoff_sleep_ms
            .iter()
            .filter(|(name, sleep_ms)| excluded_sleep_limit(name).is_none() && **sleep_ms > 0)
            .max_by_key(|(_, sleep_ms)| *sleep_ms)
            .map(|(name, _)| name)?;
        self.configs.iter().find(|config| config.name == *candidate)
    }

    /// 按配置计算下一次延迟、睡眠、累计统计，并检查 Killed 信号。
    fn backoff(
        &mut self,
        cfg: &BackoffConfig,
        max_sleep_ms: isize,
        error: SharedError,
    ) -> Result<(), SharedError> {
        if self.ctx.is_cancelled() {
            return Err(error);
        }

        let max_backoff_time_exceeded =
            self.total_sleep_ms - self.excluded_sleep_ms >= self.max_sleep_ms;
        let max_excluded_time_exceeded = excluded_sleep_limit(&cfg.name).is_some_and(|max_limit| {
            self.excluded_sleep_ms >= max_limit && self.excluded_sleep_ms >= self.max_sleep_ms
        });
        if self.max_sleep_ms > 0 && (max_backoff_time_exceeded || max_excluded_time_exceeded) {
            return Err(self
                .longest_sleep_config()
                .and_then(|config| config.error.clone())
                .unwrap_or(error));
        }

        self.append_error(&error);
        self.configs.push(cfg.clone());

        let base_override = cfg
            .name
            .eq_ignore_ascii_case(TXN_LOCK_FAST)
            .then_some(self.vars.get().BackoffLockFast as isize);
        let schedule = self
            .schedules
            .entry(cfg.name.clone())
            .or_insert_with(|| cfg.schedule(base_override));
        let mut sleep = schedule.next_delay_duration().unwrap_or_default();
        if max_sleep_ms >= 0 {
            sleep = sleep.min(Duration::from_millis(
                u64::try_from(max_sleep_ms).unwrap_or(u64::MAX),
            ));
        }
        let real_sleep_ms = self.sleep(sleep);

        self.total_sleep_ms = self.total_sleep_ms.saturating_add(real_sleep_ms);
        if excluded_sleep_limit(&cfg.name).is_some() {
            self.excluded_sleep_ms = self.excluded_sleep_ms.saturating_add(real_sleep_ms);
        }
        let sleep_total = self.backoff_sleep_ms.entry(cfg.name.clone()).or_default();
        *sleep_total = sleep_total.saturating_add(real_sleep_ms);
        let times = self.backoff_times.entry(cfg.name.clone()).or_default();
        *times = times.saturating_add(1);

        if let Some(details) = &self.ctx.exec_details {
            details.record_backoff(real_sleep_ms);
        }

        let signal = self.vars.get().Killed.load(Ordering::Relaxed);
        if signal != 0 {
            return Err(SharedError::new(
                derr::TiKvError::QueryInterruptedWithSignal { signal },
            ));
        }
        Ok(())
    }
}

fn excluded_sleep_limit(name: &str) -> Option<isize> {
    (name == TIKV_SERVER_BUSY).then_some(TIKV_SERVER_BUSY_MAX_EXCLUDED_SLEEP_MS)
}

/// Backoffer wraps the TiKV backoffer and normalizes its errors to TiDB errors.
/// 包装 TiKvBackoffer，并将底层错误归一化为 TiDB 错误。
pub struct Backoffer<'a> {
    pub b: TiKvBackoffer<'a>,
}

/// Creates a Backoffer with maximum sleep time (in milliseconds) and variables.
/// 按最大睡眠毫秒与可选会话变量创建 Backoffer。
pub fn NewBackofferWithVars<'a>(
    ctx: Context,
    maxSleep: isize,
    vars: Option<&'a kv::Variables<'a>>,
) -> Backoffer<'a> {
    Backoffer {
        b: TiKvBackoffer::new_with_vars(ctx, maxSleep, vars),
    }
}

/// Creates a Backoffer by wrapping an existing TiKV backoffer.
/// 包装已有 TiKvBackoffer 创建 Backoffer。
pub fn NewBackofferWithTikvBo(bo: TiKvBackoffer<'_>) -> Backoffer<'_> {
    Backoffer { b: bo }
}

/// Creates a Backoffer with maximum sleep time (in milliseconds).
/// 仅按最大睡眠毫秒创建 Backoffer（默认变量）。
pub fn NewBackoffer(ctx: Context, maxSleep: isize) -> Backoffer<'static> {
    Backoffer {
        b: TiKvBackoffer::new(ctx, maxSleep),
    }
}

impl Backoffer<'_> {
    /// Returns the wrapped TiKV backoffer.
    /// 返回内部 TiKvBackoffer 引用。
    pub fn TiKVBackoffer(&self) -> &TiKvBackoffer<'_> {
        &self.b
    }

    /// Sleeps according to the configuration and normalizes any returned error.
    /// 按配置睡眠；错误经 ToTiDBErr 归一化。
    pub fn Backoff(&mut self, cfg: &BackoffConfig, err: SharedError) -> Result<(), SharedError> {
        self.b
            .backoff(cfg, -1, err)
            .map_err(|error| derr::ToTiDBErr(Some(error)).expect("non-nil error stays non-nil"))
    }

    /// Uses txnLockFast and caps this individual sleep to `maxSleepMs`.
    /// 使用 txnLockFast 策略，并将本次睡眠上限截断为 `maxSleepMs`。
    pub fn BackoffWithMaxSleepTxnLockFast(
        &mut self,
        maxSleepMs: isize,
        err: SharedError,
    ) -> Result<(), SharedError> {
        let cfg = BackoffConfig::new(TXN_LOCK_FAST, 2, 3_000, Jitter::EqualJitter);
        self.b
            .backoff(&cfg, maxSleepMs, err)
            .map_err(|error| derr::ToTiDBErr(Some(error)).expect("non-nil error stays non-nil"))
    }

    /// 各退避名称的触发次数。
    pub fn GetBackoffTimes(&self) -> HashMap<String, isize> {
        self.b.backoff_times.clone()
    }

    /// 返回关联 Context。
    pub fn GetCtx(&self) -> Context {
        self.b.ctx.clone()
    }

    /// 返回当前会话变量。
    pub fn GetVars(&self) -> &kv::Variables<'_> {
        self.b.vars.get()
    }

    /// 各退避名称累计睡眠毫秒。
    pub fn GetBackoffSleepMS(&self) -> HashMap<String, isize> {
        self.b.backoff_sleep_ms.clone()
    }

    /// 全部退避累计睡眠毫秒。
    pub fn GetTotalSleep(&self) -> isize {
        self.b.total_sleep_ms
    }
}
