// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! RangeController and RPCResult retry strategies, matching `import_retry.go`.
//!
//! 本文件提供跨 region 应用函数时的重试控制器与 RPC 结果分类，对齐 Go `import_retry.go`。
//! `RangeController` 扫描 `[start,end)` 上的 region，对每个 region 调用 `RegionFunc`。
//! 失败时按 `RetryStrategy` 决定：放弃、从当前 region 重试、或从整段起点重扫。
//! Store 级错误（busy/not-leader/epoch）与 gRPC 瞬时错误走不同策略，与 Go 分支一致。
//! 事件监听器用于埋点；默认 Nop，导入路径挂 MetricListener。
//! CreateRangeController 会对 end 做 TruncateTS + PrefixNext，调用方传入原始 end 即可。
//! StrategyFromThisRegion 适合 leader 漂移、短暂 busy；StrategyFromStart 适合拓扑重构。
//! memory is limited 单独睡 15s，给 TiKV 释放内存的窗口，可用 failpoint 缩短。
//! 累计 errors 用 multierr，最终失败时保留整条重试轨迹。
//! RegionFunc 返回 RPCResult 而非 Result，以便携带 store/import 结构化错误。
//! 本文件被 import.rs 与单测共同依赖，是日志导入重试的核心状态机。

use std::thread;
use std::time::Duration;

use crate::stubs::berrors;
use crate::stubs::errorpb;
use crate::stubs::grpc_status::{self, Code};
use crate::stubs::import_sstpb;
use crate::stubs::logutil;
use crate::stubs::metapb;
use crate::stubs::multierr;
use crate::stubs::split_client::{
    CheckRegionEpoch, PaginateScanRegion, RegionInfo, ScanRegionPaginationLimit, SplitClient,
};
use crate::stubs::utils_retry::{self, RetryState};
use crate::stubs::{Context, Error, Result, failpoint};
use astersql_br_pkg_restore_utils::TruncateTS;

/// 在单个 region 上执行导入/下载的回调；返回 RPCResult 供策略机判定。
// 闭包可修改 RegionInfo（例如更新 Leader），供后续重试复用。
pub type RegionFunc = Box<dyn FnMut(&Context, &mut RegionInfo) -> RPCResult + Send>;

/// 观察 RangeController 生命周期事件：请求、region 重试、整段重扫、成功。
pub trait RangeCtlEventListener: Send {
    fn OnRequestRegion(&mut self, ctx: &Context, region: &RegionInfo);
    fn OnRetryRegion(&mut self, ctx: &Context, region: &RegionInfo, err: &Error);
    fn OnRetryRange(&mut self, ctx: &Context, err: &Error);
    fn OnRegionSuccess(&mut self, ctx: &Context, region: &RegionInfo);
}

/// 空监听器：测试或无需埋点时的默认实现。
// 零开销占位，避免 RangeController 强制依赖指标库。
pub struct RangeCtlNopListener;

impl RangeCtlEventListener for RangeCtlNopListener {
    fn OnRequestRegion(&mut self, _ctx: &Context, _region: &RegionInfo) {}
    fn OnRetryRegion(&mut self, _ctx: &Context, _region: &RegionInfo, _err: &Error) {}
    fn OnRetryRange(&mut self, _ctx: &Context, _err: &Error) {}
    fn OnRegionSuccess(&mut self, _ctx: &Context, _region: &RegionInfo) {}
}

/// 键范围上的 region 遍历与重试状态机。
pub struct RangeController {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
    pub metaClient: std::sync::Arc<dyn SplitClient>,
    // 累计的多错误，耗尽重试后一并返回。
    pub errors: Option<Error>,
    // 共享退避与剩余次数；整段重扫也消耗同一预算。
    pub rs: RetryState,
    pub listener: Box<dyn RangeCtlEventListener>,
}

/// 将四类事件映射到 Prometheus Counter，供 ImportKVFiles 使用。
pub struct RangeCtlMetricListener {
    pub RequestRegion: &'static crate::stubs::metrics::Counter,
    pub RetryRegion: &'static crate::stubs::metrics::Counter,
    pub RetryRange: &'static crate::stubs::metrics::Counter,
    pub RegionSuccess: &'static crate::stubs::metrics::Counter,
}

impl RangeCtlEventListener for RangeCtlMetricListener {
    fn OnRequestRegion(&mut self, _ctx: &Context, _region: &RegionInfo) {
        self.RequestRegion.Inc();
    }
    fn OnRetryRegion(&mut self, _ctx: &Context, _region: &RegionInfo, _err: &Error) {
        self.RetryRegion.Inc();
    }
    fn OnRetryRange(&mut self, _ctx: &Context, _err: &Error) {
        self.RetryRange.Inc();
    }
    fn OnRegionSuccess(&mut self, _ctx: &Context, _region: &RegionInfo) {
        self.RegionSuccess.Inc();
    }
}

/// 构造控制器：截断 end 的 TS、再 PrefixNext，使扫描上界与 Go 半开区间一致。
pub fn CreateRangeController(
    start: Vec<u8>,
    end: Vec<u8>,
    metaClient: std::sync::Arc<dyn SplitClient>,
    retryStatus: RetryState,
) -> RangeController {
    // 去掉 ts 后缀再取 next key，保证半开区间覆盖完整用户键。
    let end = TruncateTS(&end).unwrap_or(end);
    let end = utils_retry::PrefixNextKey(&end);
    RangeController {
        start,
        end,
        metaClient,
        errors: None,
        rs: retryStatus,
        listener: Box::new(RangeCtlNopListener),
    }
}

impl RangeController {
    /// 替换事件监听器（通常在 Apply 前挂上 MetricListener）。
    pub fn SetEventListener(&mut self, listener: Box<dyn RangeCtlEventListener>) {
        self.listener = listener;
    }

    // 将本次失败注解 region id 后追加到 errors 链。
    // 即使最终从其它路径成功，历史错误仍保留在链中直至耗尽返回。
    fn onError(&mut self, result: &RPCResult, region: &RegionInfo) {
        let annotated = Error::Annotatef(
            Error::new(result.Error()),
            format!(
                "execute over region {} failed",
                region.Region.as_ref().map(|r| r.Id).unwrap_or(0)
            ),
        );
        self.errors = multierr::Append(self.errors.clone(), annotated);
    }

    // 带退避地查询 PD 找 leader；epoch 变化则报 ErrKVEpochNotMatch。
    fn tryFindLeader(&self, ctx: &Context, region: &RegionInfo) -> Result<metapb::Peer> {
        // 找 leader 使用独立退避策略，不消耗外层 Apply 的重试次数。
        let strategy = utils_retry::NewBackoffRetryAllExceptStrategy(
            4,
            Duration::from_secs(2),
            Duration::from_secs(10),
            isNonRetryErrForFindLeader,
        );
        let region_id = region.Region.as_ref().map(|r| r.Id).unwrap_or(0);
        let client = self.metaClient.clone();
        let region = region.clone();
        utils_retry::WithRetryV2(ctx, strategy, move |_ctx| {
            let r = client.GetRegionByID(_ctx, region_id)?;
            if r.Region.is_none() {
                return Err(Error::Annotatef(
                    berrors::ErrKVEpochNotMatch("region is not found"),
                    format!("region {region_id} is not found"),
                ));
            }
            // epoch 已变说明拓扑更新，外层应走 FromStart 而非继续用旧 peer。
            if !CheckRegionEpoch(&r, &region) {
                return Err(Error::Annotatef(
                    berrors::ErrKVEpochNotMatch("epoch changed"),
                    format!("the current epoch of region {region_id} has changed"),
                ));
            }
            if let Some(leader) = r.Leader {
                return Ok(leader);
            }
            Err(Error::Annotatef(
                berrors::ErrPDLeaderNotFound("no leader"),
                format!("there is no leader for region {region_id}"),
            ))
        })
    }

    // 处理可本地修复的 store 错误；返回 false 表示应升级为整段重扫。
    // memory is limited：固定睡 15s（failpoint 可缩短）；NotLeader 优先用响应内 leader。
    fn handleRegionError(
        &mut self,
        ctx: &Context,
        result: &RPCResult,
        region: &mut RegionInfo,
    ) -> bool {
        // 先处理 memory is limited：与普通 busy 不同，需要更长冷却。
        if let Some(store_error) = result.StoreError.as_ref() {
            if IsMemoryLimited(store_error) {
                let mut sleepDuration = Duration::from_secs(15);
                // 测试用 failpoint：把长睡眠压到微秒级，避免单测超时。
                failpoint::Inject("hint-memory-is-limited", || {
                    sleepDuration = Duration::from_micros(100);
                });
                thread::sleep(sleepDuration);
                return true;
            }
        }

        // NotLeader：响应自带新 leader 时可零 RTT 切换。
        if let Some(nl) = result.StoreError.as_ref().and_then(|e| e.GetNotLeader()) {
            if let Some(leader) = &nl.Leader {
                region.Leader = Some(leader.clone());
                return true;
            }
            // 响应未带 leader：退避后主动问 PD。
            thread::sleep(self.rs.ExponentialBackoff());
            match self.tryFindLeader(ctx, region) {
                Ok(leader) => {
                    region.Leader = Some(leader);
                    return true;
                }
                Err(err) => {
                    // 找不到 leader：返回 false，触发整段重扫以刷新拓扑。
                    logutil::CL(ctx).Warn(&format!("failed to find leader: {err}"));
                    return false;
                }
            }
        }
        // 其它 FromThisRegion 错误：退避后仍在本 region 重试。
        thread::sleep(self.rs.ExponentialBackoff());
        true
    }

    /// 分页扫描区域内所有 region 并依次应用 f；重试耗尽返回累计错误。
    pub fn ApplyFuncToRange(&mut self, ctx: &Context, f: &mut RegionFunc) -> Result<()> {
        let adjustedCtx = ctx.clone();
        // 进入扫描前先检查预算，避免无意义的 PD Scan。
        if !self.rs.ShouldRetry() {
            return Err(self
                .errors
                .clone()
                .unwrap_or_else(|| Error::new("retry exhausted")));
        }
        // 分页扫全范围，limit 使用 ScanRegionPaginationLimit。
        let regionInfos = PaginateScanRegion(
            &adjustedCtx,
            self.metaClient.as_ref(),
            &self.start,
            &self.end,
            ScanRegionPaginationLimit,
        )?;
        for mut region in regionInfos {
            self.listener.OnRequestRegion(&adjustedCtx, &region);
            let (cont, err) = self.applyFuncToRegion(&adjustedCtx, f, &mut region);
            if let Some(e) = err {
                return Err(e);
            }
            // cont=false 表示已递归完成整段重扫，外层应停止。
            // 此时错误已在递归内处理或成功完成。
            if !cont {
                return Ok(());
            }
        }
        Ok(())
    }

    // 单 region 执行与策略分发；返回 (是否继续下一 region, 致命错误)。
    fn applyFuncToRegion(
        &mut self,
        ctx: &Context,
        f: &mut RegionFunc,
        region: &mut RegionInfo,
    ) -> (bool, Option<Error>) {
        if !self.rs.ShouldRetry() {
            return (
                false,
                Some(
                    self.errors
                        .clone()
                        .unwrap_or_else(|| Error::new("retry exhausted")),
                ),
            );
        }
        let result = f(ctx, region);
        if !result.OK() {
            self.onError(&result, region);
            match result.StrategyForRetry() {
                // 不可重试：立即带累计错误返回。
                RetryStrategy::StrategyGiveUp => {
                    return (
                        false,
                        Some(
                            self.errors
                                .clone()
                                .unwrap_or_else(|| Error::new(result.Error())),
                        ),
                    );
                }
                // 可在本 region 修复：更新 leader / 等待 busy 后递归重试同一 region。
                RetryStrategy::StrategyFromThisRegion => {
                    // 本地处理失败则退化为从起点重扫整段。
                    if !self.handleRegionError(ctx, &result, region) {
                        self.listener.OnRetryRange(ctx, &Error::new(result.Error()));
                        return match self.ApplyFuncToRange(ctx, f) {
                            Ok(()) => (false, None),
                            Err(e) => (false, Some(e)),
                        };
                    }
                    self.listener
                        .OnRetryRegion(ctx, region, &Error::new(result.Error()));
                    return self.applyFuncToRegion(ctx, f, region);
                }
                // 拓扑可能整体变化：放弃当前扫描游标。
                RetryStrategy::StrategyFromStart => {
                    // epoch 等拓扑变化：退避后整段重扫，避免在过期 region 上死循环。
                    self.listener.OnRetryRange(ctx, &Error::new(result.Error()));
                    thread::sleep(self.rs.ExponentialBackoff());
                    return match self.ApplyFuncToRange(ctx, f) {
                        Ok(()) => (false, None),
                        Err(e) => (false, Some(e)),
                    };
                }
            }
        }
        self.listener.OnRegionSuccess(ctx, region);
        (true, None)
    }
}

fn isNonRetryErrForFindLeader(err: &Error) -> bool {
    err.IsCode("BR:KV:ErrKVEpochNotMatch")
}

/// Go classifies the special TiKV memory-pressure response by the top-level
/// `errorpb.Error.Message`; the nested `ServerIsBusy` payload only identifies
/// the error kind.
pub(crate) fn IsMemoryLimited(store_error: &errorpb::Error) -> bool {
    store_error.GetServerIsBusy().is_some()
        && store_error.GetMessage().contains("memory is limited")
}

/// 统一包装 Apply 结果：Go error、import 消息、store error 三者互斥组合。
#[derive(Clone, Debug, Default)]
pub struct RPCResult {
    pub Err: Option<Error>,
    pub ImportError: String,
    pub StoreError: Option<errorpb::Error>,
}

/// 从 ImportSST PB Error 构造：保留 StoreError 供策略识别 NotLeader/Busy。
pub fn RPCResultFromPBError(err: &import_sstpb::Error) -> RPCResult {
    RPCResult {
        ImportError: err.GetMessage().to_string(),
        StoreError: err.StoreError.clone(),
        Err: None,
    }
}

/// 从本地/传输层 Error 构造；走 gRPC code 分类。
pub fn RPCResultFromError(err: Error) -> RPCResult {
    RPCResult {
        Err: Some(err),
        ..Default::default()
    }
}

/// 成功结果：三类错误字段皆空。
// 与 Go RPCResult{} 零值语义对齐。
pub fn RPCResultOK() -> RPCResult {
    RPCResult::default()
}

/// 重试策略：放弃 / 当前 region 重试 / 从范围起点重扫。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryStrategy {
    StrategyGiveUp,
    StrategyFromThisRegion,
    StrategyFromStart,
}

impl RPCResult {
    /// 有 Go Error 时优先按 gRPC code 分类，否则按 store/import 错误分类。
    pub fn StrategyForRetry(&self) -> RetryStrategy {
        if self.Err.is_some() {
            return self.StrategyForRetryGoError();
        }
        self.StrategyForRetryStoreError()
    }

    /// Busy / RegionNotInitialized / NotLeader → 本 region；其它 store/import 错 → 整段重扫。
    pub fn StrategyForRetryStoreError(&self) -> RetryStrategy {
        // 无任何错误字段却走到此处：视为逻辑 bug，直接放弃。
        if self.StoreError.is_none() && self.ImportError.is_empty() {
            return RetryStrategy::StrategyGiveUp;
        }
        if let Some(se) = &self.StoreError {
            if se.GetServerIsBusy().is_some()
                || se.GetRegionNotInitialized().is_some()
                || se.GetNotLeader().is_some()
            {
                return RetryStrategy::StrategyFromThisRegion;
            }
        }
        // 其它 store/import 错误（含 epoch not match）默认整段重扫。
        RetryStrategy::StrategyFromStart
    }

    /// Unavailable/Aborted/ResourceExhausted/DeadlineExceeded → 本 region；其余放弃。
    pub fn StrategyForRetryGoError(&self) -> RetryStrategy {
        // 依赖 stub 从错误字符串解析 gRPC code，与 Go status.FromError 对齐。
        if let Some(err) = &self.Err {
            if let Some(st) = grpc_status::FromError(err) {
                match st.Code() {
                    Code::Unavailable
                    | Code::Aborted
                    | Code::ResourceExhausted
                    | Code::DeadlineExceeded => {
                        return RetryStrategy::StrategyFromThisRegion;
                    }
                    _ => {}
                }
            }
        }
        // 非瞬时 gRPC 错误（如 InvalidArgument）不应盲目重试。
        RetryStrategy::StrategyGiveUp
    }

    /// 提取人类可读错误串；三者皆空时返回 BUG 哨兵，暴露调用方误用。
    pub fn Error(&self) -> String {
        if let Some(err) = &self.Err {
            return err.to_string();
        }
        if let Some(se) = &self.StoreError {
            return se.GetMessage().to_string();
        }
        if !self.ImportError.is_empty() {
            return self.ImportError.clone();
        }
        "BUG(There is no error but reported as error)".into()
    }

    /// 三者皆空才算成功。
    pub fn OK(&self) -> bool {
        // 与 Go RPCResult.OK 三字段全空判定一致。
        self.Err.is_none() && self.ImportError.is_empty() && self.StoreError.is_none()
    }
}
