// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// TiFlash 副本管理 API（DDL 侧）。
//
// TiFlash 是 TiDB 生态中的列式存储引擎，通过 Raft Learner 副本从 TiKV（行存）
// 异步同步数据，用于加速分析型（OLAP）查询。当用户执行
// `ALTER TABLE ... SET TIFLASH REPLICA n` 后，DDL 层需要周期性轮询 PD
// （Placement Driver，负责调度与元数据管理的组件）来确认各表/分区的
// TiFlash 副本同步进度，并在副本全部就绪后把表标记为“可用”。
//
// 本模块提供该轮询流程的核心数据结构与纯函数实现：
// - `PollTiFlashContext`：带指数退避（backoff）的轮询上下文，避免对长期
//   未就绪的大表频繁发起进度查询；
// - `load_tiflash_replica_status` / `poll_replica_status`：把逻辑表展开为
//   物理表（分区）粒度的副本状态并按观测进度刷新；
// - `desired_placement_rules` / `refresh_tiflash_placement_rules`：计算并
//   修复 PD 上缺失或过期的 placement rule（副本放置规则）。
//
// 文件开头以块注释保留了 Go(TiDB) 原始实现的机械翻译版本，仅作对照参考。

/*
//

use std::collections::HashMap;

// TiFlashReplicaStatus records status for each TiFlash replica.
// TiFlashReplicaStatus 对应 Go 结构体，记录每个逻辑表或分区的 TiFlash 副本状态。
pub struct TiFlashReplicaStatus {
    pub ID: i64,
    pub Count: u64,
    pub LocationLabels: Vec<String>,
    pub Available: bool,
    pub LogicalTableAvailable: bool,
    pub HighPriority: bool,
    pub IsPartition: bool,
}

// TiFlashTick is type for backoff threshold.
// TiFlashTick 是 TiFlash 轮询退避阈值类型。
pub type TiFlashTick = f64;

// PollTiFlashBackoffElement records backoff for each TiFlash Table.
// `Counter` increases every `Tick`, if it reached `Threshold`, it will be reset to 0 while `Threshold` grows.
// `TotalCounter` records total `Tick`s this element has since created.
// PollTiFlashBackoffElement 保存单表退避状态。
pub struct PollTiFlashBackoffElement {
    pub Counter: i32,
    pub Threshold: TiFlashTick,
    pub TotalCounter: i32,
}

// NewPollTiFlashBackoffElement initialize backoff element for a TiFlash table.
// NewPollTiFlashBackoffElement 使用最小 tick 初始化退避元素。
pub fn NewPollTiFlashBackoffElement() -> *mut PollTiFlashBackoffElement {
    Box::into_raw(Box::new(PollTiFlashBackoffElement {
        Counter: 0,
        Threshold: PollTiFlashBackoffMinTick,
        TotalCounter: 0,
    }))
}

// PollTiFlashBackoffContext is a collection of all backoff states.
// PollTiFlashBackoffContext 保存所有正在退避的表 ID 及全局阈值配置。
pub struct PollTiFlashBackoffContext {
    pub MinThreshold: TiFlashTick,
    pub MaxThreshold: TiFlashTick,
    // Capacity limits tables a backoff pool can handle, in order to limit handling of big tables.
    pub Capacity: i32,
    pub Rate: TiFlashTick,
    pub elements: HashMap<i64, *mut PollTiFlashBackoffElement>,
}

// NewPollTiFlashBackoffContext creates an instance of PollTiFlashBackoffContext.
// NewPollTiFlashBackoffContext 对应 Go 的参数校验：阈值顺序、最小值、容量和增长率都必须合法。
pub fn NewPollTiFlashBackoffContext(
    minThreshold: TiFlashTick,
    maxThreshold: TiFlashTick,
    capacity: i32,
    rate: TiFlashTick,
) -> Result<*mut PollTiFlashBackoffContext, errors::Error> {
    if maxThreshold < minThreshold {
        return Err(fmt::Errorf("`maxThreshold` should always be larger than `minThreshold`"));
    }
    if minThreshold < 1.0 {
        return Err(fmt::Errorf("`minThreshold` should not be less than 1"));
    }
    if capacity < 0 {
        return Err(fmt::Errorf("negative `capacity`"));
    }
    if rate <= 1.0 {
        return Err(fmt::Errorf("`rate` should always be larger than 1"));
    }
    Ok(Box::into_raw(Box::new(PollTiFlashBackoffContext {
        MinThreshold: minThreshold,
        MaxThreshold: maxThreshold,
        Capacity: capacity,
        elements: HashMap::new(),
        Rate: rate,
    })))
}

// TiFlashManagementContext is the context for TiFlash Replica Management
// TiFlashManagementContext 汇总本轮轮询需要的 store 缓存、退避状态和待刷新进度表队列。
pub struct TiFlashManagementContext {
    // The latest TiFlash stores info. For Classic kernel, it contains all TiFlash nodes. For NextGen kernel, it contains only TiFlash write nodes.
    pub TiFlashStores: HashMap<i64, pd::StoreInfo>,
    pub TiKVStores: HashMap<i64, pd::StoreInfo>,
    pub PollCounter: u64,
    pub Backoff: *mut PollTiFlashBackoffContext,
    // tables waiting for updating progress after become available.
    pub UpdatingProgressTables: list::List,
}

// AvailableTableID is the table id info of available table for waiting to update TiFlash replica progress.
// AvailableTableID 保存已经可用但仍等待刷新进度缓存的表或分区 ID。
pub struct AvailableTableID {
    pub ID: i64,
    pub IsPartition: bool,
}

impl PollTiFlashBackoffContext {
    // Tick will first check increase Counter.
    // It returns:
    // 1. A bool indicates whether threshold is grown during this tick.
    // 2. A bool indicates whether this ID exists.
    // 3. A int indicates how many ticks ID has counted till now.
    // Tick 先尝试增长阈值，再累加计数；返回值顺序保留 Go 的 grew/exist/cnt。
    pub fn Tick(&mut self, id: i64) -> (bool, bool, i32) {
        let (e, ok) = self.Get(id);
        if !ok {
            return (false, false, 0);
        }
        let grew = unsafe { (*e).MaybeGrow(self) };
        unsafe {
            (*e).Counter += 1;
            (*e).TotalCounter += 1;
            (grew, true, (*e).TotalCounter)
        }
    }

    // Remove will reset table from backoff.
    // Remove 从退避池删除表 ID，返回原先是否存在。
    pub fn Remove(&mut self, id: i64) -> bool {
        self.elements.remove(&id).is_some()
    }

    // Get returns pointer to inner PollTiFlashBackoffElement.
    // Only exported for test.
    // Get 直接返回内部元素指针，保留 Go 测试可见性。
    pub fn Get(&mut self, id: i64) -> (*mut PollTiFlashBackoffElement, bool) {
        match self.elements.get(&id) {
            Some(v) => (*v, true),
            None => (std::ptr::null_mut(), false),
        }
    }

    // Put will record table into backoff pool, if there is enough room, or returns false.
    // Put 在容量允许时把表加入退避池；已有元素视为成功。
    pub fn Put(&mut self, id: i64) -> bool {
        if self.elements.contains_key(&id) {
            true
        } else if self.Len() < self.Capacity {
            self.elements.insert(id, NewPollTiFlashBackoffElement());
            true
        } else {
            false
        }
    }

    // Len gets size of PollTiFlashBackoffContext.
    // Len 返回当前退避池大小。
    pub fn Len(&self) -> i32 {
        self.elements.len() as i32
    }
}

impl PollTiFlashBackoffElement {
    // NeedGrow returns if we need to grow.
    // It is exported for testing.
    // NeedGrow 判断 Counter 是否达到当前阈值。
    pub fn NeedGrow(&self) -> bool {
        self.Counter >= self.Threshold as i32
    }

    // doGrow 增长阈值并重置 Counter，增长上限为 MaxThreshold。
    pub fn doGrow(&mut self, b: &PollTiFlashBackoffContext) {
        if self.Threshold < b.MinThreshold {
            self.Threshold = b.MinThreshold;
        }
        if self.Threshold * b.Rate > b.MaxThreshold {
            self.Threshold = b.MaxThreshold;
        } else {
            self.Threshold *= b.Rate;
        }
        self.Counter = 0;
    }

    // MaybeGrow grows threshold and reset counter when needed.
    // MaybeGrow 仅当 NeedGrow 为真时执行增长。
    pub fn MaybeGrow(&mut self, b: &PollTiFlashBackoffContext) -> bool {
        if !self.NeedGrow() {
            return false;
        }
        self.doGrow(b);
        true
    }
}

// NewTiFlashManagementContext creates an instance for TiFlashManagementContext.
// NewTiFlashManagementContext 初始化 store 缓存、退避池和待刷新队列。
pub fn NewTiFlashManagementContext() -> Result<*mut TiFlashManagementContext, errors::Error> {
    let c = NewPollTiFlashBackoffContext(
        PollTiFlashBackoffMinTick,
        PollTiFlashBackoffMaxTick,
        PollTiFlashBackoffCapacity,
        PollTiFlashBackoffRate,
    )?;
    Ok(Box::into_raw(Box::new(TiFlashManagementContext {
        PollCounter: 0,
        TiFlashStores: HashMap::new(),
        TiKVStores: HashMap::new(),
        Backoff: c,
        UpdatingProgressTables: list::List::new(),
    })))
}

// PollTiFlashInterval is the interval between every pollTiFlashReplicaStatus call.
pub static mut PollTiFlashInterval: time::Duration = time::Second * 2;
// PullTiFlashPdTick indicates the number of intervals before we fully sync all TiFlash pd rules and tables.
pub static PullTiFlashPdTick: atomicutil::Uint64 = atomicutil::NewUint64(30 * 5);
// UpdateTiFlashStoreTick indicates the number of intervals before we fully update TiFlash stores.
pub static UpdateTiFlashStoreTick: atomicutil::Uint64 = atomicutil::NewUint64(5);
// RefreshRulesTick indicates the number of intervals before we refresh TiFlash rules.
pub static RefreshRulesTick: atomicutil::Uint64 = atomicutil::NewUint64(10);
// PollTiFlashBackoffMaxTick is the max tick before we try to update TiFlash replica availability for one table.
pub static mut PollTiFlashBackoffMaxTick: TiFlashTick = 10.0;
// PollTiFlashBackoffMinTick is the min tick before we try to update TiFlash replica availability for one table.
pub static mut PollTiFlashBackoffMinTick: TiFlashTick = 1.0;
// PollTiFlashBackoffCapacity is the cache size of backoff struct.
pub static mut PollTiFlashBackoffCapacity: i32 = 1000;
// PollTiFlashBackoffRate is growth rate of exponential backoff threshold.
pub static mut PollTiFlashBackoffRate: TiFlashTick = 1.5;
// RefreshProgressMaxTableCount is the max count of table to refresh progress after available each poll.
pub static mut RefreshProgressMaxTableCount: u64 = 1000;

// LoadTiFlashReplicaInfo parses model.TableInfo into []TiFlashReplicaStatus.
// LoadTiFlashReplicaInfo 从 TableInfo 提取逻辑表或分区 TiFlash 状态；无 TiFlashReplica 的系统表直接跳过。
pub fn LoadTiFlashReplicaInfo(tblInfo: *mut model::TableInfo, tableList: &mut Vec<TiFlashReplicaStatus>) {
    unsafe {
        if (*tblInfo).TiFlashReplica.is_none() {
            // reject tables that has no tiflash replica such like `INFORMATION_SCHEMA`
            return;
        }
        if let Some(pi) = (*tblInfo).GetPartitionInfo() {
            for p in &pi.Definitions {
                logutil::DDLLogger().Debug(format!("Table {} has partition {}\n", (*tblInfo).ID, p.ID));
                tableList.push(TiFlashReplicaStatus {
                    ID: p.ID,
                    Count: (*tblInfo).TiFlashReplica.as_ref().unwrap().Count,
                    LocationLabels: (*tblInfo).TiFlashReplica.as_ref().unwrap().LocationLabels.clone(),
                    Available: (*tblInfo).TiFlashReplica.as_ref().unwrap().IsPartitionAvailable(p.ID),
                    LogicalTableAvailable: (*tblInfo).TiFlashReplica.as_ref().unwrap().Available,
                    HighPriority: false,
                    IsPartition: true,
                });
            }
            // partitions that in adding mid-state
            for p in &pi.AddingDefinitions {
                logutil::DDLLogger().Debug(format!("Table {} has partition adding {}\n", (*tblInfo).ID, p.ID));
                tableList.push(TiFlashReplicaStatus {
                    ID: p.ID,
                    Count: (*tblInfo).TiFlashReplica.as_ref().unwrap().Count,
                    LocationLabels: (*tblInfo).TiFlashReplica.as_ref().unwrap().LocationLabels.clone(),
                    Available: (*tblInfo).TiFlashReplica.as_ref().unwrap().IsPartitionAvailable(p.ID),
                    LogicalTableAvailable: (*tblInfo).TiFlashReplica.as_ref().unwrap().Available,
                    HighPriority: true,
                    IsPartition: true,
                });
            }
        } else {
            logutil::DDLLogger().Debug(format!("Table {} has no partition\n", (*tblInfo).ID));
            tableList.push(TiFlashReplicaStatus {
                ID: (*tblInfo).ID,
                Count: (*tblInfo).TiFlashReplica.as_ref().unwrap().Count,
                LocationLabels: (*tblInfo).TiFlashReplica.as_ref().unwrap().LocationLabels.clone(),
                Available: (*tblInfo).TiFlashReplica.as_ref().unwrap().Available,
                LogicalTableAvailable: (*tblInfo).TiFlashReplica.as_ref().unwrap().Available,
                HighPriority: false,
                IsPartition: false,
            });
        }
    }
}

// updateTiFlashWriteStores updates TiFlash (write) stores info from PD to `pollTiFlashContext.TiFlashStores`.
// updateTiFlashWriteStores 从 PD/infosync 拉取 store 状态，并区分 TiFlash write node 与 TiKV store。
pub fn updateTiFlashWriteStores(pollTiFlashContext: &mut TiFlashManagementContext) -> Result<(), errors::Error> {
    // We need the up-to-date information about TiFlash stores.
    // Since TiFlash Replica synchronize may happen immediately after new TiFlash stores are added.
    let tikvStats = infosync::GetTiFlashStoresStat(context::Background())?;
    pollTiFlashContext.TiFlashStores = HashMap::new();
    pollTiFlashContext.TiKVStores = HashMap::new();
    for store in tikvStats.Stores {
        if engine::IsTiFlashHTTPResp(&store.Store) {
            // Ignore the TiFlash read node.
            if !engine::IsTiFlashWriteHTTPResp(&store.Store) {
                continue;
            }
            pollTiFlashContext.TiFlashStores.insert(store.Store.ID, store);
        } else {
            pollTiFlashContext.TiKVStores.insert(store.Store.ID, store);
        }
    }
    logutil::DDLLogger().Debug(
        "updateTiFlashWriteStores finished",
        zap::Int("TiFlash store count", pollTiFlashContext.TiFlashStores.len() as i32),
        zap::Int("TiKV store count", pollTiFlashContext.TiKVStores.len() as i32),
    );
    Ok(())
}

// PollAvailableTableProgress will poll and check availability of available tables.
// PollAvailableTableProgress 对已经可用的表继续刷新同步进度缓存，最多处理 RefreshProgressMaxTableCount 个。
pub fn PollAvailableTableProgress(
    schemas: infoschema::InfoSchema,
    _ctx: sessionctx::Context,
    pollTiFlashContext: &mut TiFlashManagementContext,
) {
    let mut pollMaxCount = unsafe { RefreshProgressMaxTableCount };
    failpoint::Inject("PollAvailableTableProgressMaxCount", |val: failpoint::Value| {
        pollMaxCount = val.as_i32().unwrap() as u64;
    });

    let mut element = pollTiFlashContext.UpdatingProgressTables.Front();
    while element.is_some() && pollMaxCount > 0 {
        pollMaxCount -= 1;
        let availableTableID = element.Value::<AvailableTableID>();
        let table = if availableTableID.IsPartition {
            let (table, _, _) = schemas.FindTableByPartitionID(availableTableID.ID);
            if table.is_none() {
                logutil::DDLLogger().Info("get table by partition failed, may be dropped or truncated", zap::Int64("partitionID", availableTableID.ID));
                let next = element.Next();
                pollTiFlashContext.UpdatingProgressTables.Remove(element);
                element = next;
                continue;
            }
            table.unwrap()
        } else {
            let (table, ok) = schemas.TableByID(context::Background(), availableTableID.ID);
            if !ok {
                logutil::DDLLogger().Info("get table id failed, may be dropped or truncated", zap::Int64("tableID", availableTableID.ID));
                let next = element.Next();
                pollTiFlashContext.UpdatingProgressTables.Remove(element);
                element = next;
                continue;
            }
            table
        };
        let tableInfo = table.Meta();
        if tableInfo.TiFlashReplica.is_none() {
            logutil::DDLLogger().Info("table has no TiFlash replica", zap::Int64("tableID or partitionID", availableTableID.ID), zap::Bool("IsPartition", availableTableID.IsPartition));
            let next = element.Next();
            pollTiFlashContext.UpdatingProgressTables.Remove(element);
            element = next;
            continue;
        }

        let checkTiFlash = config::GetGlobalConfig().CSE.IsTiFlashEnabled();
        let checkColumnar = config::GetGlobalConfig().CSE.IsColumnarStoreEnabled();
        let mut tiflashProgress = 1.0;
        let mut columnarProgress = 1.0;

        if checkTiFlash {
            match infosync::CalculateTiFlashProgress(availableTableID.ID, tableInfo.TiFlashReplica.unwrap().Count, &pollTiFlashContext.TiFlashStores) {
                Ok((progress, _, _)) => tiflashProgress = progress,
                Err(err) => {
                    if intest::EnableInternalCheck && err.Error() != "EOF" {
                        // 测试环境端口冲突时 Go 会快速 panic；此处保留该防御分支。
                        panic!("{}", err);
                    }
                    let next = element.Next();
                    pollTiFlashContext.UpdatingProgressTables.Remove(element);
                    element = next;
                    continue;
                }
            }
        }
        if checkColumnar {
            match infosync::CalculateColumnarProgress(availableTableID.ID, &pollTiFlashContext.TiKVStores) {
                Ok(progress) => columnarProgress = progress,
                Err(err) => {
                    logutil::DDLLogger().Error("calculate columnar progress failed", zap::Error(err), zap::Int64("tableID", availableTableID.ID), zap::Bool("IsPartition", availableTableID.IsPartition));
                    let next = element.Next();
                    pollTiFlashContext.UpdatingProgressTables.Remove(element);
                    element = next;
                    continue;
                }
            }
        }
        let progress = math::Min(tiflashProgress, columnarProgress);
        if let Err(err) = infosync::UpdateTiFlashProgressCache(availableTableID.ID, progress) {
            logutil::DDLLogger().Error("update tiflash sync progress cache failed", zap::Error(err), zap::Int64("tableID", availableTableID.ID), zap::Bool("IsPartition", availableTableID.IsPartition), zap::Float64("progress", progress));
            let next = element.Next();
            pollTiFlashContext.UpdatingProgressTables.Remove(element);
            element = next;
            continue;
        }
        let next = element.Next();
        pollTiFlashContext.UpdatingProgressTables.Remove(element);
        element = next;
    }
}

impl ddl {
    // refreshTiFlashTicker 对应 Go 的单次 TiFlash 轮询：刷新 store、扫描表、更新进度和可用状态。
    pub fn refreshTiFlashTicker(
        &mut self,
        ctx: sessionctx::Context,
        pollTiFlashContext: &mut TiFlashManagementContext,
    ) -> Result<(), errors::Error> {
        if pollTiFlashContext.PollCounter % UpdateTiFlashStoreTick.Load() == 0 {
            // Update store info from pd every `UpdateTiFlashStoreTick` ticks.
            if let Err(err) = updateTiFlashWriteStores(pollTiFlashContext) {
                // If we failed to get stores from pd, retry every time.
                pollTiFlashContext.PollCounter = 0;
                return Err(err);
            }
        }

        failpoint::Inject("OneTiFlashStoreDown", || {
            for (storeID, store) in pollTiFlashContext.TiFlashStores.clone() {
                let mut store = store;
                store.Store.StateName = "Down".to_string();
                pollTiFlashContext.TiFlashStores.insert(storeID, store);
                break;
            }
        });
        pollTiFlashContext.PollCounter += 1;

        // Start to process every table.
        let schema = self.infoCache.GetLatest();
        if schema.is_none() {
            return Err(errors::New("Schema is nil"));
        }
        let schema = schema.unwrap();
        PollAvailableTableProgress(schema.clone(), ctx.clone(), pollTiFlashContext);

        let mut tableList: Vec<TiFlashReplicaStatus> = Vec::new();
        // Collect TiFlash Replica info, for every table.
        let ch = schema.ListTablesWithSpecialAttribute(infoschemacontext::TiFlashAttribute);
        for v in ch {
            for tblInfo in v.TableInfos {
                LoadTiFlashReplicaInfo(tblInfo, &mut tableList);
            }
        }

        failpoint::Inject("waitForAddPartition", |val: failpoint::Value| {
            for phyTable in &tableList {
                let is = self.infoCache.GetLatest();
                let (_, ok) = is.TableByID(self.ctx.clone(), phyTable.ID);
                if !ok {
                    let (tb, _, _) = is.FindTableByPartitionID(phyTable.ID);
                    if tb.is_none() {
                        logutil::DDLLogger().Info("waitForAddPartition");
                        time::Sleep(time::Duration::from_secs(val.as_i32().unwrap() as u64));
                    }
                }
            }
        });

        let mut needPushPending = false;
        if pollTiFlashContext.UpdatingProgressTables.Len() == 0 {
            needPushPending = true;
        }

        let checkTiFlash = config::GetGlobalConfig().CSE.IsTiFlashEnabled();
        let checkColumnar = config::GetGlobalConfig().CSE.IsColumnarStoreEnabled();

        for tb in tableList {
            // For every region in each table, if it has one replica, we reckon it ready.
            // These request can be batched as an optimization.
            let mut available = tb.Available;
            failpoint::Inject("PollTiFlashReplicaStatusReplacePrevAvailableValue", |val: failpoint::Value| {
                available = val.as_bool();
            });
            // We only check unavailable tables here, so doesn't include blocked add partition case.
            if !available && !tb.LogicalTableAvailable {
                let (enabled, inqueue, _) = unsafe { (*pollTiFlashContext.Backoff).Tick(tb.ID) };
                if inqueue && !enabled {
                    logutil::DDLLogger().Info("Escape checking available status due to backoff", zap::Int64("tableId", tb.ID));
                    continue;
                }

                let mut tiflashProgress = 1.0;
                let mut tiflashAvailProgress = 1.0;
                let mut columnarProgress = 1.0;
                if checkTiFlash {
                    // Collect the replica progress for this table from TiFlash stores.
                    // fullReplicasProgress is the progress of all TiFlash replicas is setup, while availProgress is the progress of at least 1 replicas.
                    match infosync::CalculateTiFlashProgress(tb.ID, tb.Count, &pollTiFlashContext.TiFlashStores) {
                        Ok((progress, availProgress, _)) => {
                            tiflashProgress = progress;
                            tiflashAvailProgress = availProgress;
                            logutil::DDLLogger().Debug("tiflashProgress", zap::Float64("progress", tiflashProgress), zap::Float64("availProgress", tiflashAvailProgress));
                        }
                        Err(err) => {
                            logutil::DDLLogger().Error("get tiflash sync progress failed", zap::Error(err), zap::Int64("tableID", tb.ID));
                            continue;
                        }
                    }
                }
                if checkColumnar {
                    match infosync::CalculateColumnarProgress(tb.ID, &pollTiFlashContext.TiKVStores) {
                        Ok(progress) => {
                            columnarProgress = progress;
                            logutil::DDLLogger().Debug("columnarProgress", zap::Float64("progress", columnarProgress));
                        }
                        Err(err) => {
                            logutil::DDLLogger().Error("calculate columnar progress failed", zap::Error(err), zap::Int64("tableID", tb.ID));
                            continue;
                        }
                    }
                }
                let progress = math::Min(tiflashProgress, columnarProgress);
                let availProgress = math::Min(tiflashAvailProgress, columnarProgress);
                if let Err(err) = infosync::UpdateTiFlashProgressCache(tb.ID, progress) {
                    logutil::DDLLogger().Error("get tiflash sync progress from cache failed", zap::Error(err), zap::Int64("tableID", tb.ID), zap::Bool("IsPartition", tb.IsPartition), zap::Float64("progress", progress), zap::Float64("availProgress", availProgress));
                    continue;
                }

                // `avail` indicates that all replicas have been built, and the tiflash replica
                // is ready for executing queries.
                let mut avail = availProgress >= 1.0;
                failpoint::Inject("PollTiFlashReplicaStatusReplaceCurAvailableValue", |val: failpoint::Value| {
                    avail = val.as_bool();
                });

                if progress != 1.0 {
                    if avail {
                        logutil::DDLLogger().Info("Tiflash replica is available but some Region replicas is being built", zap::Int64("tableID", tb.ID), zap::Float64("progress", progress), zap::Float64("availProgress", availProgress));
                    } else {
                        logutil::DDLLogger().Info("Tiflash replica is not available", zap::Int64("tableID", tb.ID), zap::Float64("progress", progress), zap::Float64("availProgress", availProgress));
                    }
                    // keep the table in backoff until all replicas are built.
                    unsafe { (*pollTiFlashContext.Backoff).Put(tb.ID); }
                } else {
                    logutil::DDLLogger().Info("Tiflash replica is available and all Region replicas have been built", zap::Int64("tableID", tb.ID), zap::Float64("progress", progress), zap::Float64("availProgress", availProgress));
                    unsafe { (*pollTiFlashContext.Backoff).Remove(tb.ID); }
                }
                failpoint::Inject("skipUpdateTableReplicaInfoInLoop", || {
                    failpoint::Continue();
                });
                // Will call `onUpdateFlashReplicaStatus` to update `TiFlashReplica`.
                if let Err(err) = self.executor.UpdateTableReplicaInfo(ctx.clone(), tb.ID, avail) {
                    if infoschema::ErrTableNotExists.Equal(&err) && tb.IsPartition {
                        // May be due to blocking add partition
                        logutil::DDLLogger().Info("updating TiFlash replica status err, maybe false alarm by blocking add", zap::Error(err), zap::Int64("tableID", tb.ID), zap::Bool("isPartition", tb.IsPartition));
                    } else {
                        logutil::DDLLogger().Error("updating TiFlash replica status err", zap::Error(err), zap::Int64("tableID", tb.ID), zap::Bool("isPartition", tb.IsPartition));
                    }
                }
            } else if needPushPending {
                pollTiFlashContext.UpdatingProgressTables.PushFront(AvailableTableID { ID: tb.ID, IsPartition: tb.IsPartition });
            }
        }

        Ok(())
    }

    // refreshTiFlashPlacementRules will refresh the placement rules of TiFlash replicas if on tick.
    // 1. It will scan all the meta and check if there is any TiFlash replica.
    // 2. If there is, it will check if the placement rules are missing.
    // 3. If the placement rules are missing, it will add by submit a ActionSetTiFlashReplica job to repair the entire table.
    // refreshTiFlashPlacementRules 按 tick 修复缺失的 TiFlash placement rule；只在 TiFlash 开启时运行。
    pub fn refreshTiFlashPlacementRules(&mut self, sctx: sessionctx::Context, tick: u64) -> Result<(), errors::Error> {
        if tick % RefreshRulesTick.Load() != 0 {
            return Ok(());
        }
        // No need to refresh placement rules if TiFlash is not enabled
        if !config::GetGlobalConfig().CSE.IsTiFlashEnabled() {
            return Ok(());
        }
        let schema = self.infoCache.GetLatest();
        if schema.is_none() {
            return Err(errors::New("schema is nil"));
        }
        let schema = schema.unwrap();
        let mut pendings: Vec<pending> = Vec::new();

        for dbResult in schema.ListTablesWithSpecialAttribute(infoschemacontext::TiFlashAttribute) {
            let (db, ok) = schema.SchemaByName(dbResult.DBName.clone());
            if !ok {
                return Err(infoschema::ErrDatabaseNotExists.GenWithStackByArgs(dbResult.DBName.O));
            }
            for tblInfo in dbResult.TableInfos {
                if unsafe { (*tblInfo).TiFlashReplica.is_none() } {
                    continue;
                }
                if let Some(ps) = unsafe { (*tblInfo).GetPartitionInfo() } {
                    // Go 的闭包 collectPendings 会同时收集正式分区和 adding mid-state 分区。
                    for p in ps.Definitions.iter().chain(ps.AddingDefinitions.iter()) {
                        pendings.push(pending { ID: p.ID, TableInfo: tblInfo, DBInfo: db });
                    }
                } else {
                    unsafe {
                        pendings.push(pending { ID: (*tblInfo).ID, TableInfo: tblInfo, DBInfo: db });
                    }
                }
            }
        }

        let mut fixed: HashMap<i64, ()> = HashMap::new();
        for replica in pendings {
            unsafe {
                if fixed.contains_key(&(*replica.TableInfo).ID) {
                    continue;
                }
                let rule = match infosync::GetPlacementRule(self.ctx.clone(), replica.ID) {
                    Ok(rule) => rule,
                    Err(err) => {
                        logutil::DDLLogger().Warn("get placement rule err", zap::Error(err));
                        continue;
                    }
                };
                // pdhttp.GetPlacementRule returns the zero object instead of nil pointer when not found.
                let ruleIsMissing = rule.is_none() || rule.as_ref().unwrap().ID.len() == 0;
                if ruleIsMissing && (*replica.TableInfo).TiFlashReplica.as_ref().unwrap().Count > 0 {
                    let mut job = model::Job {
                        Version: model::GetJobVerInUse(),
                        SchemaID: (*replica.DBInfo).ID,
                        TableID: (*replica.TableInfo).ID,
                        SchemaName: (*replica.DBInfo).Name.L.clone(),
                        TableName: (*replica.TableInfo).Name.L.clone(),
                        Type: model::ActionSetTiFlashReplica,
                        BinlogInfo: Some(Box::new(model::HistoryInfo::default())),
                        CDCWriteSource: sctx.GetSessionVars().CDCWriteSource,
                        SQLMode: sctx.GetSessionVars().SQLMode,
                        ..Default::default()
                    };
                    // We should reset tiflash replica available to false so that the user can wait before
                    // tiflash replica is built after fixing the placement rules.
                    let args = model::SetTiFlashReplicaArgs {
                        TiflashReplica: ast::TiFlashReplicaSpec {
                            Count: (*replica.TableInfo).TiFlashReplica.as_ref().unwrap().Count,
                            Labels: (*replica.TableInfo).TiFlashReplica.as_ref().unwrap().LocationLabels.clone(),
                        },
                        ResetAvailable: true,
                    };
                    if let Err(err) = self.executor.doDDLJob2(sctx.clone(), &mut job, &args) {
                        logutil::DDLLogger().Warn("fix tiflash placement rule err", zap::Int64("tableID", (*replica.TableInfo).ID), zap::Uint64("count", (*replica.TableInfo).TiFlashReplica.as_ref().unwrap().Count), zap::Error(err));
                    } else {
                        logutil::DDLLogger().Info("fix tiflash placement rule success", zap::Int64("tableID", (*replica.TableInfo).ID), zap::Uint64("count", (*replica.TableInfo).TiFlashReplica.as_ref().unwrap().Count));
                        fixed.insert((*replica.TableInfo).ID, ());
                    }
                }
            }
        }
        Ok(())
    }

    // PollTiFlashRoutine 对应 Go 后台循环：周期性刷新 TiFlash 状态，owner 负责更新，非 owner 清理缓存。
    pub fn PollTiFlashRoutine(&mut self) {
        let pollTiflashContext = match NewTiFlashManagementContext() {
            Ok(v) => v,
            Err(err) => {
                logutil::DDLLogger().Fatal("TiFlashManagement init failed", zap::Error(err));
                return;
            }
        };

        let mut hasSetTiFlashGroup = false;
        let mut nextSetTiFlashGroupTime = time::Now();
        loop {
            select! {
                _ = self.ctx.Done() => return,
                _ = time::After(unsafe { PollTiFlashInterval }) => {},
            }
            if self.IsTiFlashPollEnabled() {
                if self.sessPool.is_none() {
                    logutil::DDLLogger().Error("failed to get sessionPool for refreshTiFlashTicker");
                    return;
                }
                failpoint::Inject("BeforeRefreshTiFlashTickerLoop", || {
                    failpoint::Continue();
                });

                if !hasSetTiFlashGroup && !time::Now().Before(nextSetTiFlashGroupTime) {
                    // We should set tiflash rule group a higher index than other placement groups to forbid override by them.
                    // Once `SetTiFlashGroupConfig` succeed, we do not need to invoke it again. If failed, we should retry it util success.
                    if let Err(err) = infosync::SetTiFlashGroupConfig(self.ctx.clone()) {
                        logutil::DDLLogger().Warn("SetTiFlashGroupConfig failed", zap::Error(err));
                        nextSetTiFlashGroupTime = time::Now().Add(time::Minute);
                    } else {
                        hasSetTiFlashGroup = true;
                    }
                }

                let (sctx, err) = self.sessPool.as_mut().unwrap().Get();
                if err.is_none() {
                    if self.ownerManager.IsOwner() {
                        if let Err(err) = unsafe { self.refreshTiFlashTicker(sctx.clone(), &mut *pollTiflashContext) } {
                            match err.downcast_ref::<infosync::MockTiFlashError>() {
                                Some(_) => {
                                    // If we have not set up MockTiFlash instance, for those tests without TiFlash, just suppress.
                                }
                                None => logutil::DDLLogger().Warn("refreshTiFlashTicker returns error", zap::Error(err)),
                            }
                        }
                        if kerneltype::IsNextGen() {
                            if let Err(err) = unsafe { self.refreshTiFlashPlacementRules(sctx.clone(), (*pollTiflashContext).PollCounter) } {
                                logutil::DDLLogger().Warn("refreshTiFlashPlacementRules returns error", zap::Error(err));
                            }
                        }
                    } else {
                        infosync::CleanTiFlashProgressCache();
                    }
                    self.sessPool.as_mut().unwrap().Put(sctx);
                } else {
                    if sctx.is_some() {
                        self.sessPool.as_mut().unwrap().Put(sctx);
                    }
                    logutil::DDLLogger().Error("failed to get session for pollTiFlashReplicaStatus", zap::Error(err.unwrap()));
                }
            }
        }
    }
}

// pending 对应 Go 的局部修复队列元素：一个物理表或分区关联其逻辑表和数据库信息。
pub struct pending {
    pub ID: i64,
    pub TableInfo: *mut model::TableInfo,
    pub DBInfo: *mut model::DBInfo,
}
*/

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// TiFlash 轮询退避阈值，与 Go `TiFlashTick` 保持相同的浮点语义。
pub type TiFlashTick = f64;

/// 单表 TiFlash 退避状态，对应 Go `PollTiFlashBackoffElement`。
#[allow(non_snake_case)]
#[derive(Clone, Debug, PartialEq)]
pub struct PollTiFlashBackoffElement {
    pub Counter: i32,
    pub Threshold: TiFlashTick,
    pub TotalCounter: i32,
}

/// TiFlash 退避参数错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollTiFlashBackoffError {
    MaxThresholdBelowMin,
    MinThresholdBelowOne,
    NegativeCapacity,
    RateNotAboveOne,
}

impl std::fmt::Display for PollTiFlashBackoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::MaxThresholdBelowMin => {
                "`maxThreshold` should always be larger than `minThreshold`"
            }
            Self::MinThresholdBelowOne => "`minThreshold` should not be less than 1",
            Self::NegativeCapacity => "negative `capacity`",
            Self::RateNotAboveOne => "`rate` should always be larger than 1",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PollTiFlashBackoffError {}

/// 按 Go 默认值创建单表退避元素。
#[allow(non_snake_case)]
pub fn NewPollTiFlashBackoffElement() -> Box<PollTiFlashBackoffElement> {
    Box::new(PollTiFlashBackoffElement {
        Counter: 0,
        Threshold: 1.0,
        TotalCounter: 0,
    })
}

/// 多表 TiFlash 退避池，容量满时拒绝新表，不淘汰旧状态。
#[allow(non_snake_case)]
#[derive(Clone, Debug, PartialEq)]
pub struct PollTiFlashBackoffContext {
    pub MinThreshold: TiFlashTick,
    pub MaxThreshold: TiFlashTick,
    pub Capacity: i32,
    pub Rate: TiFlashTick,
    elements: BTreeMap<i64, Box<PollTiFlashBackoffElement>>,
}

/// 创建 Go 等价的 TiFlash 退避池并保留其参数校验顺序。
#[allow(non_snake_case)]
pub fn NewPollTiFlashBackoffContext(
    minThreshold: TiFlashTick,
    maxThreshold: TiFlashTick,
    capacity: i32,
    rate: TiFlashTick,
) -> Result<PollTiFlashBackoffContext, PollTiFlashBackoffError> {
    if maxThreshold < minThreshold {
        return Err(PollTiFlashBackoffError::MaxThresholdBelowMin);
    }
    if minThreshold < 1.0 {
        return Err(PollTiFlashBackoffError::MinThresholdBelowOne);
    }
    if capacity < 0 {
        return Err(PollTiFlashBackoffError::NegativeCapacity);
    }
    if rate <= 1.0 {
        return Err(PollTiFlashBackoffError::RateNotAboveOne);
    }
    Ok(PollTiFlashBackoffContext {
        MinThreshold: minThreshold,
        MaxThreshold: maxThreshold,
        Capacity: capacity,
        Rate: rate,
        elements: BTreeMap::new(),
    })
}

#[allow(non_snake_case)]
impl PollTiFlashBackoffContext {
    /// 先按当前计数判断是否增长阈值，再累加本轮与总 tick 数。
    pub fn Tick(&mut self, id: i64) -> (bool, bool, i32) {
        let Some(mut element) = self.elements.remove(&id) else {
            return (false, false, 0);
        };
        let grew = element.MaybeGrow(self);
        element.Counter += 1;
        element.TotalCounter += 1;
        let total = element.TotalCounter;
        self.elements.insert(id, element);
        (grew, true, total)
    }

    /// 删除指定表的退避状态，并返回原先是否存在。
    pub fn Remove(&mut self, id: i64) -> bool {
        self.elements.remove(&id).is_some()
    }

    /// 返回稳定的元素指针及存在标记，以保留 Go 测试的可观察契约。
    pub fn Get(&mut self, id: i64) -> (*mut PollTiFlashBackoffElement, bool) {
        match self.elements.get_mut(&id) {
            Some(element) => (&mut **element as *mut PollTiFlashBackoffElement, true),
            None => (std::ptr::null_mut(), false),
        }
    }

    /// 已有表视为成功；容量已满时与 Go 一样返回 false。
    pub fn Put(&mut self, id: i64) -> bool {
        if self.elements.contains_key(&id) {
            true
        } else if self.Len() < self.Capacity {
            self.elements.insert(id, NewPollTiFlashBackoffElement());
            true
        } else {
            false
        }
    }

    pub fn Len(&self) -> i32 {
        self.elements.len() as i32
    }
}

#[allow(non_snake_case)]
impl PollTiFlashBackoffElement {
    pub fn NeedGrow(&self) -> bool {
        self.Counter >= self.Threshold as i32
    }

    fn doGrow(&mut self, backoff: &PollTiFlashBackoffContext) {
        if self.Threshold < backoff.MinThreshold {
            self.Threshold = backoff.MinThreshold;
        }
        if self.Threshold * backoff.Rate > backoff.MaxThreshold {
            self.Threshold = backoff.MaxThreshold;
        } else {
            self.Threshold *= backoff.Rate;
        }
        self.Counter = 0;
    }

    pub fn MaybeGrow(&mut self, backoff: &PollTiFlashBackoffContext) -> bool {
        if !self.NeedGrow() {
            return false;
        }
        self.doGrow(backoff);
        true
    }
}

/// 单个物理表（普通表或某个分区）的 TiFlash 副本状态。
///
/// 分区表会被展开成多条记录：每个分区对应一个 `physical_id`，
/// 但共享同一个逻辑 `table_id`。
#[derive(Clone, Debug, PartialEq)]
pub struct TiFlashReplicaStatus {
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 物理表 ID：普通表等于 `table_id`，分区表为分区 ID。
    pub physical_id: i64,
    /// 期望的 TiFlash 副本数量。
    pub replica_count: u64,
    /// 副本是否已全部就绪（progress 达到 1.0），可用于查询。
    pub available: bool,
    /// 副本同步进度，取值范围 [0.0, 1.0]。
    pub progress: f64,
}

/// 某张表已累计的轮询 tick 数（tick 即一次轮询周期）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiFlashPollTick {
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 累计 tick 数。
    pub ticks: u64,
}

/// 单表的退避（backoff）状态元素。
///
/// 退避策略：表长期未就绪时逐步拉长轮询间隔，减轻对 PD/TiFlash 的压力。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollTiFlashBackoffEntry {
    /// 当前轮询间隔阈值：距上次轮询需要经过多少个 tick 才会再次轮询。
    pub threshold: u64,
    /// 上次轮询该表时全局 `poll_counter` 的值。
    pub count: u64,
}

/// 一张设置了 TiFlash 副本的逻辑表的元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiFlashTableReplica {
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 分区 ID 列表；为空表示非分区表。
    pub partition_ids: Vec<i64>,
    /// 期望的副本数。
    pub replica_count: u64,
    /// 位置标签（location labels），用于指导 PD 将副本分散到不同拓扑位置
    /// （如机架、可用区），提升容灾能力。
    pub location_labels: Vec<String>,
}

/// PD placement rule（副本放置规则）：描述某个物理表在 TiFlash 上应有的
/// 副本数量与位置约束。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementRule {
    /// 物理表 ID（普通表 ID 或分区 ID）。
    pub physical_id: i64,
    /// 副本数。
    pub replica_count: u64,
    /// 位置标签约束。
    pub location_labels: Vec<String>,
}

/// TiFlash 副本管理过程中的错误类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TiFlashError {
    /// 副本数非法（如为 0）。
    InvalidReplicaCount,
    /// 集群中没有可写入的 TiFlash store（存储节点）。
    NoWritableStore,
    /// 进度值非法（非有限数或超出 [0,1] 范围）。
    InvalidProgress,
}

impl std::fmt::Display for TiFlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TiFlashError {}

/// TiFlash 轮询上下文：维护全局 tick 计数以及每张表的指数退避状态。
///
/// 容量有限（`capacity`），超出时按插入/更新顺序淘汰最旧的表，
/// 避免大量长期未就绪的表占满退避池。
pub struct PollTiFlashContext {
    /// 退避池容量上限。
    capacity: usize,
    /// 退避阈值下限（最小轮询间隔，单位 tick）。
    min_threshold: u64,
    /// 退避阈值上限（最大轮询间隔，单位 tick）。
    max_threshold: u64,
    /// 阈值增长倍率：每次无进展时阈值乘以该值（指数退避）。
    grow_rate: u64,
    /// 表 ID -> 退避元素。
    entries: BTreeMap<i64, PollTiFlashBackoffEntry>,
    /// 记录插入/更新顺序，用于容量淘汰（近似 LRU）。
    order: VecDeque<i64>,
    /// 全局轮询计数器，每轮询一次加 1。
    pub poll_counter: u64,
}

impl PollTiFlashContext {
    /// 创建轮询上下文；参数会被钳制到合法范围
    /// （容量与最小阈值至少为 1，增长倍率至少为 2，上限不小于下限）。
    pub fn new(capacity: usize, min_threshold: u64, max_threshold: u64, grow_rate: u64) -> Self {
        Self {
            capacity: capacity.max(1),
            min_threshold: min_threshold.max(1),
            max_threshold: max_threshold.max(min_threshold.max(1)),
            grow_rate: grow_rate.max(2),
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            poll_counter: 0,
        }
    }

    /// 推进一个轮询周期（全局计数器加 1，饱和加法防溢出）。
    pub fn tick(&mut self) {
        self.poll_counter = self.poll_counter.saturating_add(1);
    }

    /// 判断本轮是否需要轮询该表：不在退避池中，
    /// 或距上次轮询已经过了阈值数量的 tick。
    pub fn need_poll(&self, table_id: i64) -> bool {
        self.entries.get(&table_id).is_none_or(|element| {
            self.poll_counter >= element.count.saturating_add(element.threshold)
        })
    }

    /// 更新该表的退避状态：
    /// - 若本轮观察到进度变化（`progressed`），阈值重置为下限；
    /// - 否则阈值按 `grow_rate` 指数增长，并封顶在上限。
    ///
    /// 同时刷新表在淘汰队列中的位置；池满时淘汰最旧的表。
    pub fn maybe_grow(&mut self, table_id: i64, progressed: bool) {
        let element = self
            .entries
            .entry(table_id)
            .or_insert(PollTiFlashBackoffEntry {
                threshold: self.min_threshold,
                count: self.poll_counter,
            });
        element.threshold = if progressed {
            self.min_threshold
        } else {
            element
                .threshold
                .saturating_mul(self.grow_rate)
                .min(self.max_threshold)
        };
        element.count = self.poll_counter;
        // 把该表移动到淘汰队列尾部（视为最近使用）。
        self.order.retain(|id| *id != table_id);
        self.order.push_back(table_id);
        // 超过容量时从队头淘汰最旧的表。
        while self.entries.len() > self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }

    /// 将表从退避池中移除（通常在副本已就绪时调用）。
    pub fn remove(&mut self, table_id: i64) {
        self.entries.remove(&table_id);
        self.order.retain(|id| *id != table_id);
    }

    /// 查询某表的退避元素。
    pub fn get(&self, table_id: i64) -> Option<&PollTiFlashBackoffEntry> {
        self.entries.get(&table_id)
    }
    /// 返回退避池中表的数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// 退避池是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 把逻辑表元信息展开为物理表（分区）粒度的初始副本状态列表。
///
/// 非分区表生成一条记录；分区表为每个分区各生成一条记录。
/// 初始状态均为不可用、进度 0。
pub fn load_tiflash_replica_status(tables: &[TiFlashTableReplica]) -> Vec<TiFlashReplicaStatus> {
    let mut result = Vec::new();
    for table in tables {
        let physical_ids: Vec<i64> = if table.partition_ids.is_empty() {
            vec![table.table_id]
        } else {
            table.partition_ids.clone()
        };
        result.extend(
            physical_ids
                .into_iter()
                .map(|physical_id| TiFlashReplicaStatus {
                    table_id: table.table_id,
                    physical_id,
                    replica_count: table.replica_count,
                    available: false,
                    progress: 0.0,
                }),
        );
    }
    result
}

/// 从 store（存储节点）列表中筛选出可写入的 TiFlash 节点 ID 集合。
///
/// 输入为 `(store_id, 是否为 TiFlash, 是否可写)` 三元组；
/// 只有同时满足“是 TiFlash”且“可写”的节点才会保留
/// （对应下一代架构中 TiFlash 读写节点分离，只有 write node 参与副本同步）。
pub fn writable_tiflash_stores(
    stores: impl IntoIterator<Item = (u64, bool, bool)>,
) -> BTreeSet<u64> {
    stores
        .into_iter()
        .filter_map(|(id, is_tiflash, writable)| (is_tiflash && writable).then_some(id))
        .collect()
}

/// 用观测到的同步进度刷新副本状态。
///
/// 进度必须为 [0.0, 1.0] 内的有限数，否则返回 `InvalidProgress`。
/// 进度达到 1.0 时把 `available` 置为 true。
/// 返回值表示进度相较之前是否发生了变化（用于退避策略判定是否有进展）。
pub fn update_replica_progress(
    status: &mut TiFlashReplicaStatus,
    progress: f64,
) -> Result<bool, TiFlashError> {
    if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
        return Err(TiFlashError::InvalidProgress);
    }
    // 用 EPSILON 比较浮点数，判定进度是否真的发生了变化。
    let changed = (status.progress - progress).abs() > f64::EPSILON;
    status.progress = progress;
    status.available = progress >= 1.0;
    Ok(changed)
}

/// 根据逻辑表元信息计算期望的 placement rule 列表（物理表粒度）。
///
/// 副本数为 0 视为非法配置，返回 `InvalidReplicaCount`。
pub fn desired_placement_rules(
    tables: &[TiFlashTableReplica],
) -> Result<Vec<PlacementRule>, TiFlashError> {
    let mut rules = Vec::new();
    for table in tables {
        if table.replica_count == 0 {
            return Err(TiFlashError::InvalidReplicaCount);
        }
        let physical_ids: Vec<i64> = if table.partition_ids.is_empty() {
            vec![table.table_id]
        } else {
            table.partition_ids.clone()
        };
        rules.extend(physical_ids.into_iter().map(|physical_id| PlacementRule {
            physical_id,
            replica_count: table.replica_count,
            location_labels: table.location_labels.clone(),
        }));
    }
    Ok(rules)
}

/// 对比期望规则与 PD 上现有规则，计算需要下发的增量修复动作。
///
/// 返回 `(updates, deletes)`：
/// - `updates`：缺失或内容不一致、需要新建/更新的规则；
/// - `deletes`：现有规则中已不再需要（表被删除等）、应删除的物理表 ID。
pub fn refresh_tiflash_placement_rules(
    tables: &[TiFlashTableReplica],
    existing: &BTreeMap<i64, PlacementRule>,
) -> Result<(Vec<PlacementRule>, Vec<i64>), TiFlashError> {
    let desired = desired_placement_rules(tables)?;
    let desired_ids: BTreeSet<i64> = desired.iter().map(|rule| rule.physical_id).collect();
    // 只保留现有规则中缺失或与期望不一致的条目。
    let updates = desired
        .into_iter()
        .filter(|rule| existing.get(&rule.physical_id) != Some(rule))
        .collect();
    // 现有规则中不在期望集合内的条目应被删除。
    let deletes = existing
        .keys()
        .filter(|id| !desired_ids.contains(id))
        .copied()
        .collect();
    Ok((updates, deletes))
}

/// 执行一轮副本状态轮询，返回本轮新变为“可用”的副本数量。
///
/// 流程：
/// 1. 推进全局 tick；
/// 2. 对每个副本状态，先按退避策略判断是否跳过本轮；
/// 3. 用 `observed_progress`（物理表 ID -> 观测进度）刷新状态：
///    - 无观测值视为无进展，加大退避间隔；
///    - 就绪则计数并移出退避池；未就绪则按是否有进展调整退避。
pub fn poll_replica_status(
    context: &mut PollTiFlashContext,
    statuses: &mut [TiFlashReplicaStatus],
    observed_progress: &BTreeMap<i64, f64>,
) -> Result<usize, TiFlashError> {
    context.tick();
    let mut completed = 0;
    for status in statuses {
        // 仍处于退避间隔内的表本轮跳过。
        if !context.need_poll(status.physical_id) {
            continue;
        }
        // 没有该物理表的观测进度：视为无进展，增大退避阈值。
        let Some(progress) = observed_progress.get(&status.physical_id).copied() else {
            context.maybe_grow(status.physical_id, false);
            continue;
        };
        let changed = update_replica_progress(status, progress)?;
        if status.available {
            // 副本已就绪：计数并从退避池移除。
            completed += 1;
            context.remove(status.physical_id);
        } else {
            // 未就绪：有进展则重置退避，无进展则指数退避。
            context.maybe_grow(status.physical_id, changed);
        }
    }
    Ok(completed)
}
