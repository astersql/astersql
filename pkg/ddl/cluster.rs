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

// Flashback Cluster（集群闪回）DDL 模块。
//
// “闪回”指把整个集群的数据回退到过去某个时间戳（TS）的状态。在 TiDB/TiKV 体系中，
// 数据以 MVCC（多版本并发控制，每次写入都保留历史版本）方式存储，因此闪回可以通过
// 把 MVCC 键回写到指定版本来实现，而不需要物理恢复备份。
//
// 原 Go 实现的完整流程分为四个阶段：
// 1. 校验并锁定 flashback 任务 ID，防止并发闪回；
// 2. 校验闪回时间戳（必须位于 GC 安全点之后、当前时间之前），关闭 GC（垃圾回收，
//    会清理旧版本数据）与 PD 调度（PD 是集群的元信息/调度中心，调度会移动 Region），
//    并计算需要闪回的 key range（键区间）；
// 3. 第一阶段：向 TiKV 发送 prepare RPC，锁定相关 Region（数据分片单元），使其停写；
// 4. 第二阶段：发送 flashback RPC，将 MVCC 数据回写到目标版本，类似两阶段提交
//    （先 prepare 再 commit）的模式。
//

/// 闪回任务的状态机状态。
///
/// 对应 Go 实现中 job.SchemaState 的推进过程：从待处理开始，
/// 依次经历准备（锁定 Region）、执行闪回（回写 MVCC 版本），最终完成或取消。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashbackState {
    /// 待处理：任务刚创建，尚未做任何集群变更。
    Pending,
    /// 准备中：已保存并关闭集群设置（GC、PD 调度等），正在锁定 key range。
    Preparing,
    /// 已准备：所有相关 Region 已进入闪回状态（停写），可开始回写数据。
    Prepared,
    /// 闪回执行中：正在向各 key range 发送 flashback 请求回写 MVCC 数据。
    FlashingBack,
    /// 已完成：全部区间闪回完成，集群设置已恢复。
    Done,
    /// 已取消：任务被中止，集群设置按取消路径恢复（含 TTL 开关）。
    Cancelled,
}

/// 驱动闪回状态机前进的动作。
///
/// 类似两阶段提交：先 Prepare 锁定资源，再 Flashback 实际回写，最后 Finish 收尾。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashbackAction {
    /// 准备动作：保存/关闭集群设置并锁定 Region。
    Prepare,
    /// 闪回动作：向 TiKV 发送回写请求，把数据回退到目标版本。
    Flashback,
    /// 收尾动作：全部区间完成后恢复集群设置，任务标记完成。
    Finish,
}

/// 键区间（key range）：左闭右开的字节序区间 `[start, end)`。
///
/// TiKV 中所有数据按 key 的字节序全局有序存放，闪回操作以 key range 为单位下发。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRange {
    /// 区间起始 key（包含）。
    pub start: Vec<u8>,
    /// 区间结束 key（不包含）。
    pub end: Vec<u8>,
}

/// 闪回期间需要保存并临时关闭的集群设置快照。
///
/// 闪回前必须关闭这些后台机制，避免它们在回写过程中修改数据或搬移 Region；
/// 闪回结束后按快照恢复原值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterSettings {
    /// GC（垃圾回收）开关：GC 会清理 MVCC 旧版本，闪回期间必须关闭以保留历史数据。
    pub gc_enabled: bool,
    /// 超级只读开关（tidb_super_read_only）：闪回期间置为 true，阻止用户写入。
    pub super_read_only: bool,
    /// TTL 任务开关：TTL（按生存时间自动删除过期行）后台任务在闪回期间需暂停。
    pub ttl_job_enabled: bool,
    /// 自动统计信息收集（auto analyze）开关：闪回期间暂停以避免额外写入。
    pub auto_analyze_enabled: bool,
    /// PD 调度器列表：PD 调度会迁移/合并 Region，闪回期间需清空关闭。
    pub pd_schedulers: Vec<String>,
}

/// 一次集群闪回任务的完整上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashbackJob {
    /// 闪回目标时间戳（TSO 时间戳），数据将回退到该时刻的状态。
    pub flashback_ts: u64,
    /// 当前状态机状态。
    pub state: FlashbackState,
    /// 闪回前保存的集群设置快照；完成或取消时取出并恢复。
    pub saved_settings: Option<ClusterSettings>,
    /// 需要闪回的全部 key range。
    pub ranges: Vec<KeyRange>,
    /// 已完成闪回的区间数量，用于判断能否进入收尾阶段。
    pub completed_ranges: usize,
}

/// 闪回相关操作可能返回的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterError {
    /// 动作不被闪回流程支持。
    UnsupportedAction,
    /// 闪回时间戳不早于当前时间（不能回退到未来）。
    TimestampInFuture,
    /// 闪回时间戳早于 GC 安全点，历史版本可能已被 GC 清理，无法闪回。
    BeforeGcSafePoint,
    /// 存在活跃事务，其开始时间戳落在闪回区间内，闪回会破坏其快照一致性。
    ActiveTransaction,
    /// key range 非法（起始 key 不小于结束 key）。
    InvalidRange,
    /// 当前状态下不允许执行该动作（状态机转移非法）。
    InvalidState,
}

// 用 Debug 表示直接作为 Display 输出，便于错误信息展示。
impl std::fmt::Display for ClusterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ClusterError {}

/// 校验闪回时间戳的合法性，对应 Go 的 `ValidateFlashbackTS`。
///
/// 要求 `flashback_ts` 落在 `[gc_safe_point, current_ts)` 区间：
/// - 不能指向未来（>= 当前时间戳）；
/// - 不能早于 GC 安全点（该点之前的 MVCC 历史版本可能已被清理）；
/// - 若存在最早活跃事务，其开始时间戳不得落在 `(flashback_ts, current_ts]` 内，
///   否则闪回会破坏该事务基于快照隔离的读取一致性。
pub fn validate_flashback_ts(
    flashback_ts: u64,
    current_ts: u64,
    gc_safe_point: u64,
    min_active_start_ts: Option<u64>,
) -> Result<(), ClusterError> {
    if flashback_ts >= current_ts {
        return Err(ClusterError::TimestampInFuture);
    }
    if flashback_ts < gc_safe_point {
        return Err(ClusterError::BeforeGcSafePoint);
    }
    // 活跃事务的 start_ts 晚于闪回点：闪回会改写它可能读到的数据版本，必须拒绝。
    if min_active_start_ts.is_some_and(|start_ts| start_ts <= current_ts && start_ts > flashback_ts)
    {
        return Err(ClusterError::ActiveTransaction);
    }
    Ok(())
}

/// 判断给定动作是否属于闪回流程支持的动作集合。
pub fn is_flashback_action_supported(action: FlashbackAction) -> bool {
    matches!(
        action,
        FlashbackAction::Prepare | FlashbackAction::Flashback | FlashbackAction::Finish
    )
}

/// 计算某张表数据的 key range。
///
/// TiDB 的行数据以 `t{tableID}` 为前缀编码（大端字节序保证按 ID 有序），
/// 因此表的数据区间为 `[t{id}, t{id+1})`。
pub fn table_key_range(table_id: i64) -> KeyRange {
    let mut start = b"t".to_vec();
    start.extend_from_slice(&table_id.to_be_bytes());
    let mut end = b"t".to_vec();
    // saturating_add 防止 i64::MAX 溢出。
    end.extend_from_slice(&table_id.saturating_add(1).to_be_bytes());
    KeyRange { start, end }
}

/// 合并未排除的 key range，对应 Go 的 `mergeContinuousKeyRanges`。
///
/// Go 调用方保证相邻 schema range 之间没有数据，所以未排除的 range 即使字节键
/// 不相接也应合并。`excluded`（如系统库 range）是唯一会切断连续组的边界。
pub fn merge_continuous_key_ranges(
    ranges: Vec<KeyRange>,
    excluded: &[KeyRange],
) -> Result<Vec<KeyRange>, ClusterError> {
    if ranges
        .iter()
        .chain(excluded)
        .any(|range| range.start >= range.end)
    {
        return Err(ClusterError::InvalidRange);
    }

    let mut tagged = Vec::with_capacity(ranges.len() + excluded.len());
    tagged.extend(ranges.into_iter().map(|range| (range, false)));
    tagged.extend(excluded.iter().cloned().map(|range| (range, true)));
    tagged.sort_by(|left, right| left.0.start.cmp(&right.0.start));

    let mut merged: Vec<KeyRange> = Vec::new();
    let mut current: Option<KeyRange> = None;
    for (range, is_excluded) in tagged {
        if is_excluded {
            if let Some(range) = current.take() {
                merged.push(range);
            }
            continue;
        }

        if let Some(current) = current.as_mut() {
            if range.end > current.end {
                current.end = range.end;
            }
        } else {
            current = Some(range);
        }
    }
    if let Some(range) = current {
        merged.push(range);
    }
    Ok(merged)
}

/// 为闪回关闭集群的各类后台机制，并返回关闭前的设置快照以便事后恢复。
///
/// 关闭 GC 防止历史版本被清理；开启超级只读阻止用户写入；
/// 暂停 TTL 与自动统计任务、清空 PD 调度器，保证闪回期间数据与 Region 分布稳定。
pub fn close_cluster_for_flashback(settings: &mut ClusterSettings) -> ClusterSettings {
    let saved = settings.clone();
    settings.gc_enabled = false;
    settings.super_read_only = true;
    settings.ttl_job_enabled = false;
    settings.auto_analyze_enabled = false;
    settings.pd_schedulers.clear();
    saved
}

/// 闪回结束（完成或取消）后按快照恢复集群设置。
///
/// 对应 Go 的 `finishFlashbackCluster`：TTL 开关只在任务被取消时恢复——
/// 闪回成功时 TTL 任务表本身也被回退，保持关闭可避免旧 TTL 任务立即触发误删数据。
pub fn restore_cluster_after_flashback(
    settings: &mut ClusterSettings,
    saved: ClusterSettings,
    cancelled: bool,
) {
    settings.gc_enabled = saved.gc_enabled;
    settings.super_read_only = saved.super_read_only;
    settings.auto_analyze_enabled = saved.auto_analyze_enabled;
    settings.pd_schedulers = saved.pd_schedulers;
    // 仅在取消路径恢复 TTL 开关（对应 Go 中 job.IsCancelled 的分支）。
    if cancelled {
        settings.ttl_job_enabled = saved.ttl_job_enabled;
    }
}

impl FlashbackJob {
    /// 创建一个处于 Pending 状态的新闪回任务。
    pub fn new(flashback_ts: u64, ranges: Vec<KeyRange>) -> Self {
        Self {
            flashback_ts,
            state: FlashbackState::Pending,
            saved_settings: None,
            ranges,
            completed_ranges: 0,
        }
    }

    /// 对任务施加一个动作，驱动状态机转移；非法转移返回 `InvalidState`。
    ///
    /// 状态转移路径：Pending →(Prepare)→ Preparing →(Prepare)→ Prepared
    /// →(Flashback)→ FlashingBack →(Flashback 完成全部区间)→(Finish)→ Done。
    pub fn apply_action(
        &mut self,
        action: FlashbackAction,
        settings: &mut ClusterSettings,
    ) -> Result<(), ClusterError> {
        match (self.state, action) {
            // 首次 Prepare：保存并关闭集群设置，进入准备阶段。
            (FlashbackState::Pending, FlashbackAction::Prepare) => {
                self.saved_settings = Some(close_cluster_for_flashback(settings));
                self.state = FlashbackState::Preparing;
            }
            // 再次 Prepare：表示所有 Region 已锁定完毕，准备就绪。
            (FlashbackState::Preparing, FlashbackAction::Prepare) => {
                self.state = FlashbackState::Prepared
            }
            // 开始向 TiKV 发送 flashback 请求。
            (FlashbackState::Prepared, FlashbackAction::Flashback) => {
                self.state = FlashbackState::FlashingBack
            }
            // 再次 Flashback：表示所有区间回写完成。
            (FlashbackState::FlashingBack, FlashbackAction::Flashback) => {
                self.completed_ranges = self.ranges.len()
            }
            // 收尾：仅当全部区间完成时允许 Finish，恢复集群设置并置为 Done。
            (FlashbackState::FlashingBack, FlashbackAction::Finish)
                if self.completed_ranges == self.ranges.len() =>
            {
                if let Some(saved) = self.saved_settings.take() {
                    restore_cluster_after_flashback(settings, saved, false);
                }
                self.state = FlashbackState::Done;
            }
            _ => return Err(ClusterError::InvalidState),
        }
        Ok(())
    }

    /// 取消闪回任务。进入实际回写阶段后与 Go 一致拒绝取消；此前取消会恢复集群设置。
    pub fn cancel(&mut self, settings: &mut ClusterSettings) -> Result<(), ClusterError> {
        if self.state == FlashbackState::FlashingBack {
            return Err(ClusterError::InvalidState);
        }
        if let Some(saved) = self.saved_settings.take() {
            restore_cluster_after_flashback(settings, saved, true);
        }
        self.state = FlashbackState::Cancelled;
        Ok(())
    }
}
