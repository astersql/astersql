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
