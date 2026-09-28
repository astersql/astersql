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
// 当前 Rust 文件保留了一个可编译的纯逻辑内核（状态机、TS 校验、key range 合并、
// 集群设置的保存/恢复），与外部集群交互的 Go 代码以注释块形式保留，供后续接线。

/*
// Flashback Cluster DDL：保存/关闭/恢复 PD schedule 与系统变量，校验 flashback TS，计算 key range，并向 TiKV 发送 prepare/flashback RPC。

#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables, unused_mut)]

// Go var 声明：保留全局语义，具体类型后续细化。
// var pdScheduleKey = []string{
    "merge-schedule-limit",
}

// Go const 块：保留声明顺序；真实 Rust 类型和初始化后续接线。
/*
    flashbackMaxBackoff = 1800000         // 1800s
    flashbackTimeout    = 3 * time.Minute // 3min
)
*/

// closePDSchedule 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn closePDSchedule(ctx: context::Context) -> Result<(), errors::Error> {
    let mut closeMap = make(map[string]any)
    for _, key = range pdScheduleKey {
        closeMap[key] = 0
    }
    return infosync.SetPDScheduleConfig(ctx, closeMap)
}

// savePDSchedule 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn savePDSchedule(ctx: context::Context, args: &mut model::FlashbackClusterArgs) -> Result<(), errors::Error> {
    let mut retValue, err = infosync.GetPDScheduleConfig(ctx)
    if err != None {
        return err
    }
    let mut saveValue = make(map[string]any)
    for _, key = range pdScheduleKey {
        saveValue[key] = retValue[key]
    }
    args.PDScheduleValue = saveValue
    return None
}
*/

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

/*

// recoverPDSchedule 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn recoverPDSchedule(ctx: context::Context, pdScheduleParam: std::collections::HashMap<String, any>) -> Result<(), errors::Error> {
    if pdScheduleParam == None {
        return None
    }
    return infosync.SetPDScheduleConfig(ctx, pdScheduleParam)
}

// getStoreGlobalMinSafeTS 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn getStoreGlobalMinSafeTS(s: kv::Storage) -> time::Time {
    let mut minSafeTS = s.GetMinSafeTS(kv.GlobalTxnScope)
    // Inject mocked SafeTS for test.
    // failpoint 测试注入点保留为注释语义，不在这里执行。
    failpoint.Inject("injectSafeTS", func(val failpoint.Value) {
        let mut injectTS = val.(int)
        let mut minSafeTS = uint64(injectTS)
    })
    return oracle.GetTimeFromTS(minSafeTS)
}

// ValidateFlashbackTS validates that flashBackTS in range [gcSafePoint, currentTS).
// ValidateFlashbackTS 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn ValidateFlashbackTS(ctx: context::Context, sctx: sessionctx::Context, flashBackTS: u64) -> Result<(), errors::Error> {
    let mut currentVer, err = sctx.GetStore().CurrentVersion(oracle.GlobalTxnScope)
    if err != None {
        return errors::Errorf("fail to validate flashback timestamp: %v", err)
    }
    let mut currentTS = currentVer.Ver

    let mut oracleFlashbackTS = oracle.GetTimeFromTS(flashBackTS)
    if oracleFlashbackTS.After(oracle.GetTimeFromTS(currentTS)) {
        return errors::Errorf("cannot set flashback timestamp to future time")
    }

    let mut flashbackGetMinSafeTimeTimeout = time.Minute
    // failpoint 测试注入点保留为注释语义，不在这里执行。
    failpoint.Inject("changeFlashbackGetMinSafeTimeTimeout", func(val failpoint.Value) {
        let mut t = val.(int)
        let mut flashbackGetMinSafeTimeTimeout = time.Duration(t)
    })

    let mut start = time.Now()
    let mut minSafeTime = getStoreGlobalMinSafeTS(sctx.GetStore())
    // ticker 轮询迁移点：Rust 后续应映射到定时器并确保退出时释放。
    let mut ticker = time.NewTicker(time.Second)
    // Go defer 在 Rust 中应改成 Drop/作用域收尾；这里保留原收尾时机说明。
    defer ticker.Stop()
    for oracleFlashbackTS.After(minSafeTime) {
        if time.Since(start) >= flashbackGetMinSafeTimeTimeout {
            return errors::Errorf("cannot set flashback timestamp after min-resolved-ts(%s)", minSafeTime)
        }
        // Go select 同时等待 context 和 channel；Rust 后续需用 async select 或等价轮询表达。
        select {
        // Go case <-ticker.C:
            let mut minSafeTime = getStoreGlobalMinSafeTS(sctx.GetStore())
        // Go case <-ctx.Done():
            return ctx.Err()
        }
    }

    let mut gcSafePoint, err = gcutil.GetGCSafePoint(sctx)
    if err != None {
        return err
    }

    return gcutil.ValidateSnapshotWithGCSafePoint(flashBackTS, gcSafePoint)
}

// getGlobalSysVarAsBool 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。

pub fn getGlobalSysVarAsBool(sess: sessionctx::Context, name: String) -> Result<bool, errors::Error> {
    //nolint:forbidigo
    // 全局系统变量读取依赖 session accessor；这里只保留读取边界。
    let mut val, err = sess.GetSessionVars().GlobalVarsAccessor.GetGlobalSysVar(name)
    if err != None {
        return false, errors::Trace(err)
    }
    return variable.TiDBOptOn(val), None
}

// setGlobalSysVarFromBool 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。

pub fn setGlobalSysVarFromBool(ctx: context::Context, sess: sessionctx::Context, name: String, value: bool) -> Result<(), errors::Error> {
    let mut sv = vardef.On
    if !value {
        let mut sv = vardef.Off
    }

    //nolint:forbidigo
    // Global sysvar write affects cluster behavior.
    return sess.GetSessionVars().GlobalVarsAccessor.SetGlobalSysVar(ctx, name, sv)
}

// isFlashbackSupportedDDLAction 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn isFlashbackSupportedDDLAction(action: model::ActionType) -> bool {
    match action {
    // Go case model.ActionSetTiFlashReplica, model.ActionUpdateTiFlashReplicaStatus, model.ActionAlterPlacementPolicy,
        model.ActionAlterTablePlacement, model.ActionAlterTablePartitionPlacement, model.ActionCreatePlacementPolicy,
        model.ActionDropPlacementPolicy, model.ActionModifySchemaDefaultPlacement,
        model.ActionAlterTableAttributes, model.ActionAlterTablePartitionAttributes:
        return false
    // Go default:
        return true
    }
}

// checkSystemSchemaID 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。

pub fn checkSystemSchemaID(t: meta::Reader, schemaID: i64, flashbackTSString: String) -> Result<(), errors::Error> {
    if schemaID <= 0 {
        return None
    }
    let mut dbInfo, err = t.GetDatabase(schemaID)
    if err != None || dbInfo == None {
        return errors::Trace(err)
    }
    if filter.IsSystemSchema(dbInfo.Name.L) {
        return errors::Errorf("Detected modified system table during [%s, now), can't do flashback", flashbackTSString)
    }
    return None
}

// checkAndSetFlashbackClusterInfo 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn checkAndSetFlashbackClusterInfo(ctx: context::Context, se: sessionctx::Context, store: kv::Storage, t: &mut meta::Mutator, job: &mut model::Job, flashbackTS: u64) -> Result<(), errors::Error> {
    if err = ValidateFlashbackTS(ctx, se, flashbackTS); err != None {
        return err
    }

    if err = gcutil.DisableGC(se); err != None {
        return err
    }
    if err = closePDSchedule(ctx); err != None {
        return err
    }
    if err = setGlobalSysVarFromBool(ctx, se, vardef.TiDBEnableAutoAnalyze, false); err != None {
        return err
    }
    if err = setGlobalSysVarFromBool(ctx, se, vardef.TiDBSuperReadOnly, true); err != None {
        return err
    }
    if err = setGlobalSysVarFromBool(ctx, se, vardef.TiDBTTLJobEnable, false); err != None {
        return err
    }

    let mut nowSchemaVersion, err = t.GetSchemaVersion()
    if err != None {
        return errors::Trace(err)
    }

    let mut flashbackSnapshotMeta = meta.NewReader(store.GetSnapshot(kv.NewVersion(flashbackTS)))
    let mut flashbackSchemaVersion, err = flashbackSnapshotMeta.GetSchemaVersion()
    if err != None {
        return errors::Trace(err)
    }

    let mut flashbackTSString = oracle.GetTimeFromTS(flashbackTS).Format(types.TimeFSPFormat)

    // Check if there is an upgrade during [flashbackTS, now)
    let mut sql = fmt.Sprintf("select VARIABLE_VALUE from mysql.tidb as of timestamp '%s' where VARIABLE_NAME='tidb_server_version'", flashbackTSString)
    let mut rows, err = sess.NewSession(se).Execute(ctx, sql, "check_tidb_server_version")
    if err != None || len(rows) == 0 {
        return errors::Errorf("Get history `tidb_server_version` failed, can't do flashback")
    }
    let mut sql = fmt.Sprintf("select 1 from mysql.tidb where VARIABLE_NAME='tidb_server_version' and VARIABLE_VALUE=%s", rows[0].GetString(0))
    let mut rows, err = sess.NewSession(se).Execute(ctx, sql, "check_tidb_server_version")
    if err != None {
        return errors::Trace(err)
    }
    if len(rows) == 0 {
        return errors::Errorf("Detected TiDB upgrade during [%s, now), can't do flashback", flashbackTSString)
    }

    // Check is there a DDL task at flashbackTS.
    let mut sql = fmt.Sprintf("select count(*) from mysql.tidb_ddl_job as of timestamp '%s'", flashbackTSString)
    let mut rows, err = sess.NewSession(se).Execute(ctx, sql, "check_history_job")
    if err != None || len(rows) == 0 {
        return errors::Errorf("Get history ddl jobs failed, can't do flashback")
    }
    if rows[0].GetInt64(0) != 0 {
        return errors::Errorf("Detected another DDL job at %s, can't do flashback", flashbackTSString)
    }

    // If flashbackSchemaVersion not same as nowSchemaVersion, we should check all schema diffs during [flashbackTs, now).
    for i = flashbackSchemaVersion + 1; i <= nowSchemaVersion; i++ {
        let mut diff, err = t.GetSchemaDiff(i)
        if err != None {
            return errors::Trace(err)
        }
        if diff == None {
            continue
        }
        if !isFlashbackSupportedDDLAction(diff.Type) {
            return errors::Errorf("Detected unsupported DDL job type(%s) during [%s, now), can't do flashback", diff.Type.String(), flashbackTSString)
        }
        let mut err = checkSystemSchemaID(flashbackSnapshotMeta, diff.SchemaID, flashbackTSString)
        if err != None {
            return errors::Trace(err)
        }
    }

    let mut jobs, err = GetAllDDLJobs(ctx, se)
    if err != None {
        return errors::Trace(err)
    }
    // Other ddl jobs in queue, return error.
    if len(jobs) != 1 {
        var otherJob *model.Job
        for _, j = range jobs {
            if j.ID != job.ID {
                let mut otherJob = j
                break
            }
        }
        return errors::Errorf("have other ddl jobs(jobID: %d) in queue, can't do flashback", otherJob.ID)
    }
    return None
}

// addToSlice 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。

pub fn addToSlice(schema: String, tableName: String, tableID: i64, flashbackIDs: Vec<i64>) -> Vec<i64> {
    if filter.IsSystemSchema(schema) && !strings.HasPrefix(tableName, "stats_") && tableName != "gc_delete_range" {
        let mut flashbackIDs = append(flashbackIDs, tableID)
    }
    return flashbackIDs
}

// getTableDataKeyRanges get keyRanges by `flashbackIDs`.
// This func will return all flashback table data key ranges.
// getTableDataKeyRanges 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn getTableDataKeyRanges(nonFlashbackTableIDs: Vec<i64>) -> Vec<kv::KeyRange> {
    var keyRanges []kv.KeyRange

    let mut nonFlashbackTableIDs = append(nonFlashbackTableIDs, -1)

    slices.SortFunc(nonFlashbackTableIDs, func(a, b int64) int {
        return cmp.Compare(a, b)
    })

    for i = 1; i < len(nonFlashbackTableIDs); i++ {
        let mut keyRanges = append(keyRanges, kv.KeyRange{
            StartKey: tablecodec.EncodeTablePrefix(nonFlashbackTableIDs[i-1] + 1),
            EndKey:   tablecodec.EncodeTablePrefix(nonFlashbackTableIDs[i]),
        })
    }

    // Add all other key ranges.
    let mut keyRanges = append(keyRanges, kv.KeyRange{
        StartKey: tablecodec.EncodeTablePrefix(nonFlashbackTableIDs[len(nonFlashbackTableIDs)-1] + 1),
        EndKey:   tablecodec.EncodeTablePrefix(metadef.MaxUserGlobalID),
    })

    return keyRanges
}

// keyRangeMayExclude 对应 Go struct；字段顺序沿用源文件，指针、接口、channel 与锁均为占位类型。
pub struct keyRangeMayExclude {
    pub r: kv::KeyRange,
    pub exclude: bool,
}

// mergeContinuousKeyRanges merges not exclude continuous key ranges and appends
// to given []kv.KeyRange, assuming the gap between key ranges has no data.
// Precondition: schemaKeyRanges is sorted by start key. schemaKeyRanges are
// non-overlapping.
// mergeContinuousKeyRanges 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn mergeContinuousKeyRanges(schemaKeyRanges: Vec<keyRangeMayExclude>) -> Vec<kv::KeyRange> {
    var (
        continuousStart, continuousEnd kv.Key
    )

    let mut result = make([]kv.KeyRange, 0, 1)

    for _, r = range schemaKeyRanges {
        if r.exclude {
            if continuousStart != None {
                let mut result = append(result, kv.KeyRange{
                    StartKey: continuousStart,
                    EndKey:   continuousEnd,
                })
                let mut continuousStart = None
            }
            continue
        }

        if continuousStart == None {
            let mut continuousStart = r.r.StartKey
        }
        let mut continuousEnd = r.r.EndKey
    }

    if continuousStart != None {
        let mut result = append(result, kv.KeyRange{
            StartKey: continuousStart,
            EndKey:   continuousEnd,
        })
    }
    return result
}

// getFlashbackKeyRanges get keyRanges for flashback cluster.
// It contains all non system table key ranges and meta data key ranges.
// The time complexity is O(nlogn).
// getFlashbackKeyRanges 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn getFlashbackKeyRanges(ctx: context::Context, sess: sessionctx::Context, flashbackTS: u64) -> Result<Vec<kv::KeyRange>, errors::Error> {
    let mut is = sess.GetLatestInfoSchema().(infoschema.InfoSchema)
    let mut schemas = is.AllSchemas()

    // get snapshot schema IDs.
    let mut flashbackSnapshotMeta = meta.NewReader(sess.GetStore().GetSnapshot(kv.NewVersion(flashbackTS)))
    let mut snapshotSchemas, err = flashbackSnapshotMeta.ListDatabases()
    if err != None {
        return None, errors::Trace(err)
    }

    let mut schemaIDs = make(map[int64]struct{})
    let mut excludeSchemaIDs = make(map[int64]struct{})
    for _, schema = range schemas {
        if filter.IsSystemSchema(schema.Name.L) {
            excludeSchemaIDs[schema.ID] = struct{}{}
        } else {
            schemaIDs[schema.ID] = struct{}{}
        }
    }
    for _, schema = range snapshotSchemas {
        if filter.IsSystemSchema(schema.Name.L) {
            excludeSchemaIDs[schema.ID] = struct{}{}
        } else {
            schemaIDs[schema.ID] = struct{}{}
        }
    }

    let mut schemaKeyRanges = make([]keyRangeMayExclude, 0, len(schemaIDs)+len(excludeSchemaIDs))
    for schemaID = range schemaIDs {
        let mut metaStartKey = tablecodec.EncodeMetaKeyPrefix(meta.DBkey(schemaID))
        let mut metaEndKey = tablecodec.EncodeMetaKeyPrefix(meta.DBkey(schemaID + 1))
        let mut schemaKeyRanges = append(schemaKeyRanges, keyRangeMayExclude{
            r: kv.KeyRange{
                StartKey: metaStartKey,
                EndKey:   metaEndKey,
            },
            exclude: false,
        })
    }
    for schemaID = range excludeSchemaIDs {
        let mut metaStartKey = tablecodec.EncodeMetaKeyPrefix(meta.DBkey(schemaID))
        let mut metaEndKey = tablecodec.EncodeMetaKeyPrefix(meta.DBkey(schemaID + 1))
        let mut schemaKeyRanges = append(schemaKeyRanges, keyRangeMayExclude{
            r: kv.KeyRange{
                StartKey: metaStartKey,
                EndKey:   metaEndKey,
            },
            exclude: true,
        })
    }

    slices.SortFunc(schemaKeyRanges, func(a, b keyRangeMayExclude) int {
        return bytes.Compare(a.r.StartKey, b.r.StartKey)
    })

    let mut keyRanges = mergeContinuousKeyRanges(schemaKeyRanges)

    let mut startKey = tablecodec.EncodeMetaKeyPrefix([]byte("DBs"))
    let mut keyRanges = append(keyRanges, kv.KeyRange{
        StartKey: startKey,
        EndKey:   startKey.PrefixNext(),
    })

    var nonFlashbackTableIDs []int64
    for _, db = range schemas {
        let mut tbls, err2 = is.SchemaTableInfos(ctx, db.Name)
        if err2 != None {
            return None, errors::Trace(err2)
        }
        for _, table = range tbls {
            if !table.IsBaseTable() || table.ID > metadef.MaxUserGlobalID {
                continue
            }
            let mut nonFlashbackTableIDs = addToSlice(db.Name.L, table.Name.L, table.ID, nonFlashbackTableIDs)
            if table.Partition != None {
                for _, partition = range table.Partition.Definitions {
                    let mut nonFlashbackTableIDs = addToSlice(db.Name.L, table.Name.L, partition.ID, nonFlashbackTableIDs)
                }
            }
        }
    }

    return append(keyRanges, getTableDataKeyRanges(nonFlashbackTableIDs)...), None
}

// SendPrepareFlashbackToVersionRPC prepares regions for flashback, the purpose is to put region into flashback state which region stop write
// Function also be called by BR for volume snapshot backup and restore
// SendPrepareFlashbackToVersionRPC 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn SendPrepareFlashbackToVersionRPC(ctx: context::Context, s: tikv::Storage, flashbackTS: u64, startTS: u64, r: tikvstore::KeyRange) -> Result<rangetask::TaskStat, errors::Error> {
    let mut startKey, rangeEndKey = r.StartKey, r.EndKey
    var taskStat rangetask.TaskStat
    // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
    let mut bo = tikv.NewBackoffer(ctx, flashbackMaxBackoff)
    for {
        // Go select 同时等待 context 和 channel；Rust 后续需用 async select 或等价轮询表达。
        select {
        // Go case <-ctx.Done():
            return taskStat, errors.WithStack(ctx.Err())
        // Go default:
        }

        if len(rangeEndKey) > 0 && bytes.Compare(startKey, rangeEndKey) >= 0 {
            break
        }

        let mut loc, err = s.GetRegionCache().LocateKey(bo, startKey)
        if err != None {
            return taskStat, err
        }

        let mut endKey = loc.EndKey
        let mut isLast = len(endKey) == 0 || (len(rangeEndKey) > 0 && bytes.Compare(endKey, rangeEndKey) >= 0)
        // If it is the last region.
        if isLast {
            let mut endKey = rangeEndKey
        }

        logutil.DDLLogger().Info("send prepare flashback request", zap.Uint64("region_id", loc.Region.GetID()),
            zap.String("start_key", hex.EncodeToString(startKey)), zap.String("end_key", hex.EncodeToString(endKey)))

        let mut req = tikvrpc.NewRequest(tikvrpc.CmdPrepareFlashbackToVersion, &kvrpcpb.PrepareFlashbackToVersionRequest{
            StartKey: startKey,
            EndKey:   endKey,
            StartTs:  startTS,
            Version:  flashbackTS,
        })

        // TiKV RPC 调用边界：保留请求、region error、重试处理顺序。
        let mut resp, err = s.SendReq(bo, req, loc.Region, flashbackTimeout)
        if err != None {
            return taskStat, err
        }
        let mut regionErr, err = resp.GetRegionError()
        if err != None {
            return taskStat, err
        }
        // failpoint 测试注入点保留为注释语义，不在这里执行。
        failpoint.Inject("mockPrepareMeetsEpochNotMatch", func(val failpoint.Value) {
            if val.(bool) && bo.ErrorsNum() == 0 {
                let mut regionErr = &errorpb.Error{
                    Message:       "stale epoch",
                    EpochNotMatch: &errorpb.EpochNotMatch{},
                }
            }
        })
        if regionErr != None {
            // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
            let mut err = bo.Backoff(tikv.BoRegionMiss(), errors::New(regionErr.String()))
            if err != None {
                return taskStat, err
            }
            continue
        }
        if resp.Resp == None {
            logutil.DDLLogger().Warn("prepare flashback miss resp body", zap.Uint64("region_id", loc.Region.GetID()))
            // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
            let mut err = bo.Backoff(tikv.BoTiKVRPC(), errors::New("prepare flashback rpc miss resp body"))
            if err != None {
                return taskStat, err
            }
            continue
        }
        let mut prepareFlashbackToVersionResp = resp.Resp.(*kvrpcpb.PrepareFlashbackToVersionResponse)
        if err = prepareFlashbackToVersionResp.GetError(); err != "" {
            // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
            let mut boErr = bo.Backoff(tikv.BoTiKVRPC(), errors::New(err))
            if boErr != None {
                return taskStat, boErr
            }
            continue
        }
        taskStat.CompletedRegions++
        if isLast {
            break
        }
        // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
        let mut bo = tikv.NewBackoffer(ctx, flashbackMaxBackoff)
        let mut startKey = endKey
    }
    return taskStat, None
}

// SendFlashbackToVersionRPC flashback the MVCC key to the version
// Function also be called by BR for volume snapshot backup and restore
// SendFlashbackToVersionRPC 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn SendFlashbackToVersionRPC(ctx: context::Context, s: tikv::Storage, version: u64, startTS: u64, commitTS: u64, r: tikvstore::KeyRange) -> Result<rangetask::TaskStat, errors::Error> {
    let mut startKey, rangeEndKey = r.StartKey, r.EndKey
    var taskStat rangetask.TaskStat
    // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
    let mut bo = tikv.NewBackoffer(ctx, flashbackMaxBackoff)
    for {
        // Go select 同时等待 context 和 channel；Rust 后续需用 async select 或等价轮询表达。
        select {
        // Go case <-ctx.Done():
            return taskStat, errors.WithStack(ctx.Err())
        // Go default:
        }

        if len(rangeEndKey) > 0 && bytes.Compare(startKey, rangeEndKey) >= 0 {
            break
        }

        let mut loc, err = s.GetRegionCache().LocateKey(bo, startKey)
        if err != None {
            return taskStat, err
        }

        let mut endKey = loc.EndKey
        let mut isLast = len(endKey) == 0 || (len(rangeEndKey) > 0 && bytes.Compare(endKey, rangeEndKey) >= 0)
        // If it is the last region.
        if isLast {
            let mut endKey = rangeEndKey
        }

        logutil.DDLLogger().Info("send flashback request", zap.Uint64("region_id", loc.Region.GetID()),
            zap.String("start_key", hex.EncodeToString(startKey)), zap.String("end_key", hex.EncodeToString(endKey)))

        let mut req = tikvrpc.NewRequest(tikvrpc.CmdFlashbackToVersion, &kvrpcpb.FlashbackToVersionRequest{
            Version:  version,
            StartKey: startKey,
            EndKey:   endKey,
            StartTs:  startTS,
            CommitTs: commitTS,
        })

        // TiKV RPC 调用边界：保留请求、region error、重试处理顺序。
        let mut resp, err = s.SendReq(bo, req, loc.Region, flashbackTimeout)
        if err != None {
            logutil.DDLLogger().Warn("send request meets error", zap.Uint64("region_id", loc.Region.GetID()), zap.Error(err))
            if err.Error() != fmt.Sprintf("region %d is not prepared for the flashback", loc.Region.GetID()) {
                return taskStat, err
            }
        } else {
            let mut regionErr, err = resp.GetRegionError()
            if err != None {
                return taskStat, err
            }
            if regionErr != None {
                // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
                let mut err = bo.Backoff(tikv.BoRegionMiss(), errors::New(regionErr.String()))
                if err != None {
                    return taskStat, err
                }
                continue
            }
            if resp.Resp == None {
                logutil.DDLLogger().Warn("flashback miss resp body", zap.Uint64("region_id", loc.Region.GetID()))
                // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
                let mut err = bo.Backoff(tikv.BoTiKVRPC(), errors::New("flashback rpc miss resp body"))
                if err != None {
                    return taskStat, err
                }
                continue
            }
            let mut flashbackToVersionResp = resp.Resp.(*kvrpcpb.FlashbackToVersionResponse)
            if respErr = flashbackToVersionResp.GetError(); respErr != "" {
                // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
                let mut boErr = bo.Backoff(tikv.BoTiKVRPC(), errors::New(respErr))
                if boErr != None {
                    return taskStat, boErr
                }
                continue
            }
        }
        taskStat.CompletedRegions++
        if isLast {
            break
        }
        // TiKV backoff/retry 语义需要保持；这里不实现真实等待。
        let mut bo = tikv.NewBackoffer(ctx, flashbackMaxBackoff)
        let mut startKey = endKey
    }
    return taskStat, None
}

// flashbackToVersion 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn flashbackToVersion(ctx: context::Context, store: kv::Storage, handler: rangetask::TaskHandler, startKey: Vec<u8>, endKey: Vec<u8>) -> Result<(), errors::Error> {
    return rangetask.NewRangeTaskRunner(
        "flashback-to-version-runner",
        store.(tikv.Storage),
        int(vardef.GetDDLFlashbackConcurrency()),
        handler,
    ).RunOnRange(ctx, startKey, endKey)
}

// splitRegionsByKeyRanges 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn splitRegionsByKeyRanges(ctx: context::Context, store: kv::Storage, keyRanges: Vec<model::KeyRange>) {
    if s, ok = store.(kv.SplittableStore); ok {
        for _, keys = range keyRanges {
            for {
                // tableID is useless when scatter == false
                let mut _, err = s.SplitRegions(ctx, [][]byte{keys.StartKey, keys.EndKey}, false, None)
                if err == None {
                    break
                }
            }
        }
    }
}

// A Flashback has 4 different stages.
// 1. before lock flashbackClusterJobID, check clusterJobID and lock it.
// 2. before flashback start, check timestamp, disable GC and close PD schedule, get flashback key ranges.
// 3. phase 1, lock flashback key ranges.
// 4. phase 2, send flashback RPC, do flashback jobs.
// onFlashbackCluster 对应 Go 的 worker 方法，保留接收者语义和调用顺序。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
impl worker {
    pub fn onFlashbackCluster(&mut self, jobCtx: &mut jobContext, job: &mut model::Job) -> Result<i64, errors::Error> {
    let mut inFlashbackTest = false
    // failpoint 测试注入点保留为注释语义，不在这里执行。
    failpoint.Inject("mockFlashbackTest", func(val failpoint.Value) {
        if val.(bool) {
            let mut inFlashbackTest = true
        }
    })
    // TODO: Support flashback in unistore.
    if jobCtx.store.Name() != "TiKV" && !inFlashbackTest {
        job.State = model.JobStateCancelled
        return ver, errors::Errorf("Not support flashback cluster in non-TiKV env")
    }

    let mut args, err = model.GetFlashbackClusterArgs(job)
    if err != None {
        job.State = model.JobStateCancelled
        return ver, errors::Trace(err)
    }

    var totalRegions, completedRegions atomic.Uint64
    totalRegions.Store(args.LockedRegionCnt)

    let mut sess, err = w.sessPool.Get()
    if err != None {
        job.State = model.JobStateCancelled
        return ver, errors::Trace(err)
    }
    // Go defer 在 Rust 中应改成 Drop/作用域收尾；这里保留原收尾时机说明。
    defer w.sessPool.Put(sess)

    match job.SchemaState {
    // Stage 1, check and set FlashbackClusterJobID, and update job args.
    // Go case model.StateNone:
        if err = savePDSchedule(w.workCtx, args); err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }

        args.EnableGC, err = gcutil.CheckGCEnable(sess)
        if err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }

        args.EnableAutoAnalyze, err = getGlobalSysVarAsBool(sess, vardef.TiDBEnableAutoAnalyze)
        if err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }

        args.SuperReadOnly, err = getGlobalSysVarAsBool(sess, vardef.TiDBSuperReadOnly)
        if err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }

        args.EnableTTLJob, err = getGlobalSysVarAsBool(sess, vardef.TiDBTTLJobEnable)
        if err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }

        job.FillArgs(args)
        job.SchemaState = model.StateDeleteOnly
        return ver, None
    // Stage 2, check flashbackTS, close GC and PD schedule, get flashback key ranges.
    // Go case model.StateDeleteOnly:
        if err = checkAndSetFlashbackClusterInfo(w.workCtx, sess, jobCtx.store, jobCtx.metaMut, job, args.FlashbackTS); err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }
        // We should get startTS here to avoid lost startTS when TiDB crashed during send prepare flashback RPC.
        args.StartTS, err = jobCtx.store.GetOracle().GetTimestamp(w.workCtx, &oracle.Option{TxnScope: oracle.GlobalTxnScope})
        if err != None {
            job.State = model.JobStateCancelled
            return ver, errors::Trace(err)
        }
        let mut keyRanges, err = getFlashbackKeyRanges(w.workCtx, sess, args.FlashbackTS)
        if err != None {
            return ver, errors::Trace(err)
        }
        args.FlashbackKeyRanges = make([]model.KeyRange, len(keyRanges))
        for i, keyRange = range keyRanges {
            args.FlashbackKeyRanges[i] = model.KeyRange{
                StartKey: keyRange.StartKey,
                EndKey:   keyRange.EndKey,
            }
        }

        job.FillArgs(args)
        job.SchemaState = model.StateWriteOnly
        return updateSchemaVersion(jobCtx, job)
    // Stage 3, lock related key ranges.
    // Go case model.StateWriteOnly:
        // TODO: Support flashback in unistore.
        if inFlashbackTest {
            job.SchemaState = model.StateWriteReorganization
            return updateSchemaVersion(jobCtx, job)
        }
        // Split region by keyRanges, make sure no unrelated key ranges be locked.
        splitRegionsByKeyRanges(w.workCtx, jobCtx.store, args.FlashbackKeyRanges)
        totalRegions.Store(0)
        for _, r = range args.FlashbackKeyRanges {
            if err = flashbackToVersion(w.workCtx, jobCtx.store,
                func(ctx context.Context, r tikvstore.KeyRange) (rangetask.TaskStat, error) {
                    let mut stats, err = SendPrepareFlashbackToVersionRPC(ctx, jobCtx.store.(tikv.Storage), args.FlashbackTS, args.StartTS, r)
                    totalRegions.Add(uint64(stats.CompletedRegions))
                    return stats, err
                }, r.StartKey, r.EndKey); err != None {
                logutil.DDLLogger().Warn("Get error when do flashback", zap.Error(err))
                return ver, err
            }
        }
        args.LockedRegionCnt = totalRegions.Load()

        // We should get commitTS here to avoid lost commitTS when TiDB crashed during send flashback RPC.
        args.CommitTS, err = jobCtx.store.GetOracle().GetTimestamp(w.workCtx, &oracle.Option{TxnScope: oracle.GlobalTxnScope})
        if err != None {
            return ver, errors::Trace(err)
        }
        job.FillArgs(args)
        job.SchemaState = model.StateWriteReorganization
        return ver, None
    // Stage 4, get key ranges and send flashback RPC.
    // Go case model.StateWriteReorganization:
        // TODO: Support flashback in unistore.
        if inFlashbackTest {
            let mut err = asyncNotifyEvent(jobCtx, notifier.NewFlashbackClusterEvent(), job, noSubJob, w.sess)
            if err != None {
                return ver, errors::Trace(err)
            }
            job.State = model.JobStateDone
            job.SchemaState = model.StatePublic
            return ver, None
        }

        for _, r = range args.FlashbackKeyRanges {
            if err = flashbackToVersion(w.workCtx, jobCtx.store,
                func(ctx context.Context, r tikvstore.KeyRange) (rangetask.TaskStat, error) {
                    // Use same startTS as prepare phase to simulate 1PC txn.
                    let mut stats, err = SendFlashbackToVersionRPC(ctx, jobCtx.store.(tikv.Storage), args.FlashbackTS, args.StartTS, args.CommitTS, r)
                    completedRegions.Add(uint64(stats.CompletedRegions))
                    logutil.DDLLogger().Info("flashback cluster stats",
                        zap.Uint64("complete regions", completedRegions.Load()),
                        zap.Uint64("total regions", totalRegions.Load()),
                        zap.Error(err))
                    return stats, err
                }, r.StartKey, r.EndKey); err != None {
                logutil.DDLLogger().Warn("Get error when do flashback", zap.Error(err))
                return ver, errors::Trace(err)
            }
        }
        let mut err = asyncNotifyEvent(jobCtx, notifier.NewFlashbackClusterEvent(), job, noSubJob, w.sess)
        if err != None {
            return ver, errors::Trace(err)
        }

        job.State = model.JobStateDone
        job.SchemaState = model.StatePublic
        return updateSchemaVersion(jobCtx, job)
    }
    return ver, None
}
}

// finishFlashbackCluster 对应 Go 函数，保留源文件中的声明顺序、主要分支和错误返回语义。
// 该函数属于 flashback/PD/TiKV 交互路径；保留校验、重试和 RPC 调用边界，不访问真实集群。
pub fn finishFlashbackCluster(w: &mut worker, job: &mut model::Job) -> Result<(), errors::Error> {
    // Didn't do anything during flashback, return directly
    if job.SchemaState == model.StateNone {
        return None
    }

    let mut args, err = model.GetFlashbackClusterArgs(job)
    if err != None {
        return errors::Trace(err)
    }

    let mut sess, err = w.sessPool.Get()
    if err != None {
        return errors::Trace(err)
    }
    // Go defer 在 Rust 中应改成 Drop/作用域收尾；这里保留原收尾时机说明。
    defer w.sessPool.Put(sess)

    // 事务边界迁移点：Go 在闭包内提交/回滚，新 Rust 接线时需保持同一事务作用域。
    let mut err = kv.RunInNewTxn(w.workCtx, w.store, true, func(context.Context, kv.Transaction) error {
        if err = recoverPDSchedule(w.ctx, args.PDScheduleValue); err != None {
            return errors::Trace(err)
        }

        if args.EnableGC {
            if err = gcutil.EnableGC(sess); err != None {
                return errors::Trace(err)
            }
        }

        if err = setGlobalSysVarFromBool(w.workCtx, sess, vardef.TiDBSuperReadOnly, args.SuperReadOnly); err != None {
            return errors::Trace(err)
        }

        if job.IsCancelled() {
            // only restore `tidb_ttl_job_enable` when flashback failed
            if err = setGlobalSysVarFromBool(w.workCtx, sess, vardef.TiDBTTLJobEnable, args.EnableTTLJob); err != None {
                return errors::Trace(err)
            }
        }

        if err = setGlobalSysVarFromBool(w.workCtx, sess, vardef.TiDBEnableAutoAnalyze, args.EnableAutoAnalyze); err != None {
            return errors::Trace(err)
        }

        return None
    })
    if err != None {
        return errors::Trace(err)
    }

    return None
}
*/
