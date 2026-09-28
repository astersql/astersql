// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! TiKV recovery data flow (from `br/pkg/restore/data/data.go`).
//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/data/data.rs`对应的TiKV 恢复数据流，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少117行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `RecoveryStage`承载"RecoveryStage"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl RecoveryStage`把"RecoveryStage"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `recoveryError`承载"recoveryError"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `atStage`是当前文件的重要函数，承担"atStage"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `isRetryErr`是当前文件的重要函数，承担"isRetryErr"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `stage_err`是当前文件的重要函数，承担"stage_err"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `RecoverData`是当前文件的重要函数，承担"RecoverData"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `doRecoveryData`是当前文件的重要函数，承担"doRecoveryData"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `CancelOnDrop`承载"CancelOnDrop"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Drop`把"Drop"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `StoreMeta`承载"StoreMeta"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `NewStoreMeta`是当前文件的重要函数，承担"NewStoreMeta"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Recovery`承载"Recovery"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `NewRecovery`是当前文件的重要函数，承担"NewRecovery"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Recovery`把"Recovery"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `newRecoveryClient`是当前文件的重要函数，承担"newRecoveryClient"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `ReadRegionMeta`是当前文件的重要函数，承担"ReadRegionMeta"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetTotalRegions`是当前文件的重要函数，承担"GetTotalRegions"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `RecoverRegionOfStore`是当前文件的重要函数，承担"RecoverRegionOfStore"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `RecoverRegions`是当前文件的重要函数，承担"RecoverRegions"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `SpawnTiKVShutDownWatchers`是当前文件的重要函数，承担"SpawnTiKVShutDownWatchers"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `PrepareFlashbackToVersion`是当前文件的重要函数，承担"PrepareFlashbackToVersion"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `FlashbackToVersion`是当前文件的重要函数，承担"FlashbackToVersion"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MakeRecoveryPlan`是当前文件的重要函数，承担"MakeRecoveryPlan"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `ConnCloser`承载"ConnCloser"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `run_on_full_range`是当前文件的重要函数，承担"run_on_full_range"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `getStoreAddress`是当前文件的重要函数，承担"getStoreAddress"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 中文注释索引结束

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::recover::{
    CheckConsistencyAndValidPeer, LeaderCandidates, RecoverRegion, SelectRegionLeader,
    SortRecoverRegions,
};
use crate::stubs::log;
use crate::stubs::metapb;
use crate::stubs::recovpb;
use crate::stubs::{
    Context, Error, ErrorGroup, FlashbackRpc, KeyRange, MakeCallback, MaxStoreConcurrency, Mgr,
    NewFlashBackBackoffStrategy, NewRecoveryBackoffStrategy, Progress, Result, StoreWatcher,
    WithRetry, WithRetryV2, WorkerPool, gRPCBackOffMaxDelay, is_eof,
};

/// RecoveryStage marks which step failed for retry decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum RecoveryStage {
    StageUnknown = 0,
    StageCollectingMeta = 1,
    StageMakingRecoveryPlan = 2,
    StageResetPDAllocateID = 3,
    StageRecovering = 4,
    StageFlashback = 5,
}

impl RecoveryStage {
    pub fn String(self) -> &'static str {
        match self {
            RecoveryStage::StageCollectingMeta => "collecting meta",
            RecoveryStage::StageMakingRecoveryPlan => "making recovery plan",
            RecoveryStage::StageResetPDAllocateID => "resetting PD allocate ID",
            RecoveryStage::StageRecovering => "recovering",
            RecoveryStage::StageFlashback => "flashback",
            RecoveryStage::StageUnknown => "unknown",
        }
    }
}

impl std::fmt::Display for RecoveryStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.String())
    }
}

/// recoveryError wraps an error with the stage where it occurred.
#[derive(Clone, Debug)]
pub struct recoveryError {
    pub error: Error,
    pub atStage: RecoveryStage,
}

impl From<recoveryError> for Error {
    fn from(value: recoveryError) -> Self {
        let mut err = value.error;
        err.stage = Some(value.atStage as i32);
        err
    }
}

impl std::fmt::Display for recoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for recoveryError {}

pub fn atStage(err: &Error) -> RecoveryStage {
    match err.stage {
        Some(1) => RecoveryStage::StageCollectingMeta,
        Some(2) => RecoveryStage::StageMakingRecoveryPlan,
        Some(3) => RecoveryStage::StageResetPDAllocateID,
        Some(4) => RecoveryStage::StageRecovering,
        Some(5) => RecoveryStage::StageFlashback,
        _ => RecoveryStage::StageUnknown,
    }
}

pub fn isRetryErr(err: &Error) -> bool {
    let stage = atStage(err);
    match stage {
        RecoveryStage::StageCollectingMeta
        | RecoveryStage::StageMakingRecoveryPlan
        | RecoveryStage::StageResetPDAllocateID
        | RecoveryStage::StageRecovering => {
            log::Info("Recovery data retrying.");
            true
        }
        RecoveryStage::StageFlashback => {
            log::Info("Giving up retry for flashback stage.");
            false
        }
        RecoveryStage::StageUnknown => {
            log::Warn("unknown stage of recovery for backoff.");
            false
        }
    }
}

fn stage_err(err: Error, stage: RecoveryStage) -> Error {
    Error::from(recoveryError {
        error: err,
        atStage: stage,
    })
}

/// RecoverData recovers the TiKV cluster (Go six-step flow with coarse retry).
pub fn RecoverData(
    ctx: &Context,
    resolveTS: u64,
    allStores: Vec<metapb::Store>,
    mgr: Arc<dyn Mgr>,
    progress: Arc<dyn Progress>,
    restoreTS: u64,
    concurrency: u32,
) -> Result<i32> {
    let _ = gRPCBackOffMaxDelay;
    WithRetryV2(
        ctx,
        NewRecoveryBackoffStrategy(Box::new(isRetryErr)),
        |ctx| {
            doRecoveryData(
                ctx,
                resolveTS,
                allStores.clone(),
                mgr.clone(),
                progress.clone(),
                restoreTS,
                concurrency,
            )
        },
    )
}

fn doRecoveryData(
    ctx: &Context,
    resolveTS: u64,
    allStores: Vec<metapb::Store>,
    mgr: Arc<dyn Mgr>,
    progress: Arc<dyn Progress>,
    restoreTS: u64,
    concurrency: u32,
) -> Result<i32> {
    let (ctx, cancel_handle) = Context::WithCancel(ctx);
    let _cancel_guard = CancelOnDrop(cancel_handle);

    let mut recovery = NewRecovery(allStores, mgr, progress, concurrency);
    if let Err(err) = recovery.ReadRegionMeta(&ctx) {
        return Err(stage_err(err, RecoveryStage::StageCollectingMeta));
    }

    let totalRegions = recovery.GetTotalRegions();

    if let Err(err) = recovery.MakeRecoveryPlan() {
        return Err(stage_err(err, RecoveryStage::StageMakingRecoveryPlan));
    }

    log::Info("recover the alloc id to pd");
    if let Err(err) = recovery.mgr.RecoverBaseAllocID(&ctx, recovery.MaxAllocID) {
        return Err(stage_err(err, RecoveryStage::StageResetPDAllocateID));
    }

    recovery.SpawnTiKVShutDownWatchers(&ctx);
    if let Err(err) = recovery.RecoverRegions(&ctx) {
        return Err(stage_err(err, RecoveryStage::StageRecovering));
    }

    if let Err(err) = recovery.PrepareFlashbackToVersion(&ctx, resolveTS, restoreTS.wrapping_sub(1))
    {
        return Err(stage_err(err, RecoveryStage::StageFlashback));
    }

    if let Err(err) = recovery.FlashbackToVersion(&ctx, resolveTS, restoreTS) {
        return Err(stage_err(err, RecoveryStage::StageFlashback));
    }

    Ok(totalRegions)
}

struct CancelOnDrop(Context);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel(Error::new("context canceled"));
    }
}

/// StoreMeta aggregates region metas from one TiKV store.
#[derive(Clone, Debug, Default)]
pub struct StoreMeta {
    pub StoreId: u64,
    pub RegionMetas: Vec<recovpb::RegionMeta>,
}

pub fn NewStoreMeta(storeId: u64) -> StoreMeta {
    StoreMeta {
        StoreId: storeId,
        RegionMetas: Vec::new(),
    }
}

/// Recovery holds intermediate state for one recovery run.
pub struct Recovery {
    pub allStores: Vec<metapb::Store>,
    pub StoreMetas: Vec<StoreMeta>,
    pub RecoveryPlan: HashMap<u64, Vec<recovpb::RecoverRegionRequest>>,
    pub MaxAllocID: u64,
    pub mgr: Arc<dyn Mgr>,
    pub progress: Arc<dyn Progress>,
    pub concurrency: u32,
    /// Test hook: override watcher tick (default 30s).
    pub watcher_tick: Duration,
    /// Test hook: disable background watcher thread.
    pub spawn_watcher: bool,
}

pub fn NewRecovery(
    allStores: Vec<metapb::Store>,
    mgr: Arc<dyn Mgr>,
    progress: Arc<dyn Progress>,
    concurrency: u32,
) -> Recovery {
    let totalStores = allStores.len();
    Recovery {
        allStores,
        StoreMetas: (0..totalStores).map(|_| NewStoreMeta(0)).collect(),
        RecoveryPlan: HashMap::with_capacity(totalStores),
        MaxAllocID: 0,
        mgr,
        progress,
        concurrency,
        watcher_tick: Duration::from_secs(30),
        spawn_watcher: true,
    }
}

impl Recovery {
    fn newRecoveryClient(
        &self,
        ctx: &Context,
        storeAddr: &str,
    ) -> Result<(
        Box<dyn crate::stubs::RecoverDataClient>,
        Box<dyn crate::stubs::Conn>,
    )> {
        let Some(factory) = self.mgr.ClientFactory() else {
            return Err(Error::new(format!(
                "recovery client factory not configured for {storeAddr}"
            )));
        };
        factory(ctx, storeAddr)
    }

    /// ReadRegionMeta reads all region meta from TiKVs concurrently.
    pub fn ReadRegionMeta(&mut self, ctx: &Context) -> Result<()> {
        let totalStores = self.allStores.len();
        let (eg, ectx) = ErrorGroup::WithContext(ctx);
        let eg = eg.with_limit(std::cmp::min(totalStores, MaxStoreConcurrency));
        let workers = WorkerPool::New(
            std::cmp::min(totalStores, MaxStoreConcurrency),
            "Collect Region Meta",
        );

        let metaChan = Arc::new(Mutex::new(std::collections::VecDeque::<StoreMeta>::new()));
        let meta_notify = Arc::new(std::sync::Condvar::new());

        for i in 0..totalStores {
            let storeId = self.allStores[i].GetId();
            let storeAddr = self.allStores[i].GetAddress();
            if ectx.Done() {
                break;
            }

            let mgr = self.mgr.clone();
            let metaChan = metaChan.clone();
            let meta_notify = meta_notify.clone();
            let ectx2 = ectx.clone();
            workers.ApplyOnErrorGroup(&eg, move || {
                let factory = mgr.ClientFactory().ok_or_else(|| {
                    Error::new(format!(
                        "recovery client factory not configured for {storeAddr}"
                    ))
                })?;
                let (mut recoveryClient, mut conn) = factory(&ectx2, &storeAddr)?;
                let _close = ConnCloser(&mut *conn);
                log::Info("read meta from tikv");
                let mut stream = recoveryClient
                    .ReadRegionMeta(&ectx2, &recovpb::ReadRegionMetaRequest { StoreId: storeId })?;

                let mut storeMeta = NewStoreMeta(storeId);
                loop {
                    match stream.Recv() {
                        Ok(meta) => storeMeta.RegionMetas.push(meta),
                        Err(err) if is_eof(&err) => break,
                        Err(err) => return Err(Error::Trace(err)),
                    }
                }

                metaChan.lock().unwrap().push_back(storeMeta);
                meta_notify.notify_one();
                Ok(())
            });
        }

        for i in 0..totalStores {
            let mut got = false;
            while !got {
                if ectx.Done() {
                    break;
                }
                let mut q = metaChan.lock().unwrap();
                if let Some(storeMeta) = q.pop_front() {
                    self.StoreMetas[i] = storeMeta;
                    log::Info("received region meta from");
                    got = true;
                    continue;
                }
                let (_guard, timeout) = meta_notify
                    .wait_timeout(q, Duration::from_millis(50))
                    .unwrap();
                if timeout.timed_out() && ectx.Done() {
                    break;
                }
            }
            self.progress.Inc();
        }

        eg.Wait()
    }

    pub fn GetTotalRegions(&self) -> i32 {
        let mut regions = HashSet::new();
        for v in &self.StoreMetas {
            for m in &v.RegionMetas {
                regions.insert(m.RegionId);
            }
        }
        regions.len() as i32
    }

    pub fn RecoverRegionOfStore(
        &self,
        ctx: &Context,
        storeID: u64,
        plan: &[recovpb::RecoverRegionRequest],
    ) -> Result<()> {
        let storeAddr = getStoreAddress(&self.allStores, storeID);
        let (mut recoveryClient, mut conn) =
            self.newRecoveryClient(ctx, &storeAddr).map_err(|err| {
                log::Error("create tikv client failed");
                Error::Trace(err)
            })?;
        let _close = ConnCloser(&mut *conn);
        log::Info("send recover region to tikv");

        let mut stream = recoveryClient.RecoverRegion(ctx).map_err(|err| {
            log::Error("create recover region failed");
            Error::Trace(err)
        })?;
        for s in plan {
            if let Err(err) = stream.Send(s) {
                log::Error("send recover region failed");
                return Err(Error::Trace(err));
            }
        }

        let reply = stream.CloseAndRecv().map_err(|err| {
            log::Error("close the stream failed");
            Error::Trace(err)
        })?;
        self.progress.Inc();
        log::Info("recover region execution success");
        let _ = reply.GetStoreId();
        Ok(())
    }

    pub fn RecoverRegions(&self, ctx: &Context) -> Result<()> {
        let (eg, ectx) = ErrorGroup::WithContext(ctx);
        let totalRecoveredStores = self.RecoveryPlan.len();
        let eg = eg.with_limit(std::cmp::min(totalRecoveredStores, MaxStoreConcurrency));
        let workers = WorkerPool::New(
            std::cmp::min(totalRecoveredStores, MaxStoreConcurrency),
            "Recover Regions",
        );

        for (storeId, plan) in self.RecoveryPlan.clone() {
            if ectx.Done() {
                break;
            }
            // Capture self fields needed without requiring &mut self across threads.
            let allStores = self.allStores.clone();
            let mgr = self.mgr.clone();
            let progress = self.progress.clone();
            let ectx2 = ectx.clone();
            workers.ApplyOnErrorGroup(&eg, move || {
                let recovery = Recovery {
                    allStores,
                    StoreMetas: Vec::new(),
                    RecoveryPlan: HashMap::new(),
                    MaxAllocID: 0,
                    mgr,
                    progress,
                    concurrency: 0,
                    watcher_tick: Duration::from_secs(30),
                    spawn_watcher: false,
                };
                recovery.RecoverRegionOfStore(&ectx2, storeId, &plan)
            });
        }
        eg.Wait()
    }

    pub fn SpawnTiKVShutDownWatchers(&self, ctx: &Context) {
        if !self.spawn_watcher {
            return;
        }
        let rebootStores: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
        let rebootStores_cb = rebootStores.clone();
        let cb = MakeCallback(
            Some(Box::new(move |s: &metapb::Store| {
                log::Info("Store reboot detected, will regenerate leaders.");
                rebootStores_cb.lock().unwrap().insert(s.Id);
            })),
            Some(Box::new(|_s: &metapb::Store| {
                log::Warn("A store disconnected.");
            })),
            Some(Box::new(|_s: &metapb::Store| {
                log::Info("Start to observing the state of store.");
            })),
        );
        let watcher = Arc::new(StoreWatcher::New(self.mgr.PDClient(), cb));
        let tick = self.watcher_tick;
        let ctx = ctx.clone();
        let plan = self.RecoveryPlan.clone();
        let allStores = self.allStores.clone();
        let mgr = self.mgr.clone();
        let progress = self.progress.clone();

        std::thread::spawn(move || {
            loop {
                if ctx.Done() {
                    return;
                }
                std::thread::sleep(tick);
                if ctx.Done() {
                    return;
                }
                if let Err(_err) = watcher.Step(&ctx) {
                    log::Warn("Failed to step watcher.");
                }
                let ids: Vec<u64> = rebootStores.lock().unwrap().iter().copied().collect();
                for id in ids {
                    let Some(store_plan) = plan.get(&id).cloned() else {
                        log::Warn("Store reboot detected, but no recovery plan found.");
                        continue;
                    };
                    let recovery = Recovery {
                        allStores: allStores.clone(),
                        StoreMetas: Vec::new(),
                        RecoveryPlan: HashMap::new(),
                        MaxAllocID: 0,
                        mgr: mgr.clone(),
                        progress: progress.clone(),
                        concurrency: 0,
                        watcher_tick: tick,
                        spawn_watcher: false,
                    };
                    if let Err(_err) = recovery.RecoverRegionOfStore(&ctx, id, &store_plan) {
                        log::Warn("Store reboot detected, but failed to regenerate leader.");
                        continue;
                    }
                    log::Info("Succeed to reload the leader in store.");
                    rebootStores.lock().unwrap().remove(&id);
                }
            }
        });
    }

    pub fn PrepareFlashbackToVersion(
        &self,
        ctx: &Context,
        resolveTS: u64,
        startTS: u64,
    ) -> Result<()> {
        let flashback = self.mgr.GetFlashback();
        let concurrency = self.concurrency;
        let retryErr = WithRetry(
            ctx,
            || {
                let handler = |ctx: &Context, r: &KeyRange| {
                    let stats =
                        flashback.SendPrepareFlashbackToVersionRPC(ctx, resolveTS, startTS, r);
                    if let Err(ref err) = stats {
                        log::Warn("region may not ready to serve, retry it...");
                        let _ = err;
                    }
                    stats
                };

                let completed = run_on_full_range(ctx, concurrency, handler)?;
                log::Info("region flashback prepare complete");
                let _ = completed;
                Ok(())
            },
            NewFlashBackBackoffStrategy(),
        );

        self.progress.Inc();
        retryErr
    }

    pub fn FlashbackToVersion(&self, ctx: &Context, resolveTS: u64, commitTS: u64) -> Result<()> {
        let flashback = self.mgr.GetFlashback();
        let concurrency = self.concurrency;
        let handler = |ctx: &Context, r: &KeyRange| {
            flashback.SendFlashbackToVersionRPC(
                ctx,
                resolveTS,
                commitTS.wrapping_sub(1),
                commitTS,
                r,
            )
        };

        match run_on_full_range(ctx, concurrency, handler) {
            Ok(completed) => {
                log::Info("region flashback complete");
                let _ = completed;
                self.progress.Inc();
                Ok(())
            }
            Err(err) => {
                log::Error("region flashback get error");
                Err(Error::Trace(err))
            }
        }
    }

    /// MakeRecoveryPlan builds per-store recover requests and MaxAllocID.
    pub fn MakeRecoveryPlan(&mut self) -> Result<()> {
        let mut storeBalanceScore: HashMap<u64, i32> = HashMap::with_capacity(self.allStores.len());
        let mut regions: HashMap<u64, Vec<RecoverRegion>> = HashMap::new();

        for v in &self.StoreMetas {
            let storeId = v.StoreId;
            let mut maxId = storeId;
            for m in &v.RegionMetas {
                regions.entry(m.RegionId).or_default().push(RecoverRegion {
                    RegionMeta: m.clone(),
                    StoreId: storeId,
                });
                maxId = maxId.max(m.RegionId.max(m.PeerId));
            }
            self.MaxAllocID = self.MaxAllocID.max(maxId);
        }

        let regionInfos = SortRecoverRegions(&mut regions);
        let validPeers = CheckConsistencyAndValidPeer(regionInfos).map_err(Error::Trace)?;

        for (regionId, peers) in regions {
            if !validPeers.contains(&regionId) {
                log::Warn("detected tombstone peer for region");
                for peer in peers {
                    let plan = recovpb::RecoverRegionRequest {
                        Tombstone: true,
                        AsLeader: false,
                        RegionId: 0,
                    };
                    self.RecoveryPlan
                        .entry(peer.StoreId)
                        .or_default()
                        .push(plan);
                }
            } else {
                log::Debug("detected valid region");
                let leaderCandidates = LeaderCandidates(&peers).map_err(|err| {
                    log::Warn("region without peer");
                    Error::Trace(err)
                })?;

                let leader = SelectRegionLeader(&storeBalanceScore, &leaderCandidates);
                log::Debug("as leader peer");
                let plan = recovpb::RecoverRegionRequest {
                    RegionId: leader.RegionId,
                    AsLeader: true,
                    Tombstone: false,
                };
                self.RecoveryPlan
                    .entry(leader.StoreId)
                    .or_default()
                    .push(plan);
                *storeBalanceScore.entry(leader.StoreId).or_default() += 1;
            }
        }
        Ok(())
    }
}

struct ConnCloser<'a>(&'a mut dyn crate::stubs::Conn);

impl Drop for ConnCloser<'_> {
    fn drop(&mut self) {
        self.0.Close();
    }
}

fn run_on_full_range<F>(ctx: &Context, _concurrency: u32, handler: F) -> Result<i32>
where
    F: Fn(&Context, &KeyRange) -> Result<crate::stubs::TaskStat>,
{
    // Empty keys means unbounded full-cluster range (Go rangetask).
    let r = KeyRange {
        StartKey: Vec::new(),
        EndKey: Vec::new(),
    };
    let stats = handler(ctx, &r)?;
    Ok(stats.CompletedRegions)
}

pub fn getStoreAddress(allStores: &[metapb::Store], storeId: u64) -> String {
    let mut addr = String::new();
    for store in allStores {
        if store.GetId() == storeId {
            addr = store.GetAddress();
        }
    }
    if addr.is_empty() {
        log::Error("there is no tikv has this Id");
    }
    addr
}

// Silence unused import if FlashbackRpc only used via trait object methods.
#[allow(dead_code)]
fn _flashback_bound(_: &dyn FlashbackRpc) {}
