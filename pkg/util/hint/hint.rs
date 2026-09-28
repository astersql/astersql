// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// SQL 优化器 hint 常量、语句级/计划级解析与 unmatched warning 收集。
//
// 对应 Go `pkg/util/hint/hint.go`。提供 hint 名称字符串、Prefer* 位图、
// `StmtHints`/`PlanHints` 结构，以及 `ParseStmtHints`/`ParsePlanHints`、
// 表名匹配与 hint 恢复字符串工具。Hint 用于引导执行计划（物理算子选择），
// 不改变 SQL 语义正确性。

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

// Hint flags listed here are used by PlanBuilder.subQueryHintFlags.
// 下列字符串常量保持 Go 原值；SQL parser、binding 和 warning 文案都依赖这些稳定文本。
/// Sort Merge Join 旧版 TiDB hint 名。
pub const TiDBMergeJoin: &str = "tidb_smj";
/// Sort Merge Join hint 名。
pub const HintSMJ: &str = "merge_join";
/// 禁止 Merge Join。
pub const HintNoMergeJoin: &str = "no_merge_join";
/// Broadcast Join 旧版 hint 名。
pub const TiDBBroadCastJoin: &str = "tidb_bcj";
/// Broadcast Join hint 名。
pub const HintBCJ: &str = "broadcast_join";
/// Shuffle Join hint 名。
pub const HintShuffleJoin: &str = "shuffle_join";
/// 强制按 FROM 顺序 join（straight join）。
pub const HintStraightJoin: &str = "straight_join";
/// 指定 leading join 顺序。
pub const HintLeading: &str = "leading";
/// Index Nested Loop Join 旧版 hint 名。
pub const TiDBIndexNestedLoopJoin: &str = "tidb_inlj";
/// Index Nested Loop Join。
pub const HintINLJ: &str = "inl_join";
/// Index Nested Loop Hash Join。
pub const HintINLHJ: &str = "inl_hash_join";
// Deprecated: HintINLMJ is hint enforce index nested loop merge join.
/// 已弃用：Index Nested Loop Merge Join。
pub const HintINLMJ: &str = "inl_merge_join";
/// 禁止 Index Join。
pub const HintNoIndexJoin: &str = "no_index_join";
/// 禁止 Index Hash Join。
pub const HintNoIndexHashJoin: &str = "no_index_hash_join";
/// 禁止 Index Merge Join。
pub const HintNoIndexMergeJoin: &str = "no_index_merge_join";
/// Hash Join 旧版 hint 名。
pub const TiDBHashJoin: &str = "tidb_hj";
/// 禁止 Hash Join。
pub const HintNoHashJoin: &str = "no_hash_join";
/// Hash Join。
pub const HintHJ: &str = "hash_join";
/// 指定 Hash Join build 侧。
pub const HintHashJoinBuild: &str = "hash_join_build";
/// 指定 Hash Join probe 侧。
pub const HintHashJoinProbe: &str = "hash_join_probe";
/// Hash 聚合。
pub const HintHashAgg: &str = "hash_agg";
/// Stream 聚合。
pub const HintStreamAgg: &str = "stream_agg";
/// MPP 一阶段聚合。
pub const HintMPP1PhaseAgg: &str = "mpp_1phase_agg";
/// MPP 两阶段聚合。
pub const HintMPP2PhaseAgg: &str = "mpp_2phase_agg";
/// 使用指定索引。
pub const HintUseIndex: &str = "use_index";
/// 忽略指定索引。
pub const HintIgnoreIndex: &str = "ignore_index";
/// 强制使用指定索引。
pub const HintForceIndex: &str = "force_index";
/// 按索引有序扫描。
pub const HintOrderIndex: &str = "order_index";
/// 不按索引有序扫描。
pub const HintNoOrderIndex: &str = "no_order_index";
/// Index Lookup 下推。
pub const HintIndexLookUpPushDown: &str = "index_lookup_pushdown";
/// 禁止 Index Lookup 下推。
pub const HintNoIndexLookUpPushDown: &str = "no_index_lookup_pushdown";
/// 聚合下推到 coprocessor。
pub const HintAggToCop: &str = "agg_to_cop";
/// 指定读取引擎（TiKV/TiFlash）。
pub const HintReadFromStorage: &str = "read_from_storage";
/// TiFlash 存储引擎名。
pub const HintTiFlash: &str = "tiflash";
/// TiKV 存储引擎名。
pub const HintTiKV: &str = "tikv";
/// 使用 Index Merge。
pub const HintIndexMerge: &str = "use_index_merge";
/// 时间范围 hint。
pub const HintTimeRange: &str = "time_range";
/// 不写入计划缓存。
pub const HintIgnorePlanCache: &str = "ignore_plan_cache";
/// 显式启用计划缓存。
pub const HintUsePlanCache: &str = "use_plan_cache";
/// Limit 下推到 coprocessor。
pub const HintLimitToCop: &str = "limit_to_cop";
/// CTE merge hint。
pub const HintMerge: &str = "merge";
/// 半连接改写（子查询场景）。
pub const HintSemiJoinRewrite: &str = "semi_join_rewrite";
/// 禁止子查询解相关。
pub const HintNoDecorrelate: &str = "no_decorrelate";
/// 语句内存配额。
pub const HintMemoryQuota: &str = "memory_quota";
/// IN 子查询转 Join+Agg。
pub const HintUseToja: &str = "use_toja";
/// 禁止 Index Merge。
pub const HintNoIndexMerge: &str = "no_index_merge";
/// 最大执行时间。
pub const HintMaxExecutionTime: &str = "max_execution_time";
/// 强制写慢查询日志。
pub const HintWriteSlowLog: &str = "write_slow_log";

// HintFlagSemiJoinRewrite corresponds to HintSemiJoinRewrite.
// HintFlagNoDecorrelate corresponds to HintNoDecorrelate.
/// 子查询 hint 标志：半连接改写。
pub const HintFlagSemiJoinRewrite: u64 = 1 << 0;
/// 子查询 hint 标志：禁止解相关。
pub const HintFlagNoDecorrelate: u64 = 1 << 1;

// Prefer* 常量保留 Go iota 的位图顺序，PlanHints.PreferAggType 和 join 偏好都会读取这些位。
/// 偏好 Index Nested Loop Join 的位标志。
pub const PreferINLJ: u32 = 1 << 0;
/// 偏好 INLHJ。
pub const PreferINLHJ: u32 = 1 << 1;
/// 偏好 INLMJ。
pub const PreferINLMJ: u32 = 1 << 2;
/// 偏好 Hash Join build 侧。
pub const PreferHJBuild: u32 = 1 << 3;
/// 偏好 Hash Join probe 侧。
pub const PreferHJProbe: u32 = 1 << 4;
/// 偏好 Hash Join。
pub const PreferHashJoin: u32 = 1 << 5;
/// 禁止 Hash Join 偏好位。
pub const PreferNoHashJoin: u32 = 1 << 6;
/// 偏好 Merge Join。
pub const PreferMergeJoin: u32 = 1 << 7;
/// 禁止 Merge Join 偏好位。
pub const PreferNoMergeJoin: u32 = 1 << 8;
/// 禁止 Index Join 偏好位。
pub const PreferNoIndexJoin: u32 = 1 << 9;
/// 禁止 Index Hash Join 偏好位。
pub const PreferNoIndexHashJoin: u32 = 1 << 10;
/// 禁止 Index Merge Join 偏好位。
pub const PreferNoIndexMergeJoin: u32 = 1 << 11;
/// 偏好 Broadcast Join。
pub const PreferBCJoin: u32 = 1 << 12;
/// 偏好 Shuffle Join。
pub const PreferShuffleJoin: u32 = 1 << 13;
/// 偏好半连接改写。
pub const PreferRewriteSemiJoin: u32 = 1 << 14;
/// 左表作为 INLJ inner。
pub const PreferLeftAsINLJInner: u32 = 1 << 15;
/// 右表作为 INLJ inner。
pub const PreferRightAsINLJInner: u32 = 1 << 16;
/// 左表作为 INLHJ inner。
pub const PreferLeftAsINLHJInner: u32 = 1 << 17;
/// 右表作为 INLHJ inner。
pub const PreferRightAsINLHJInner: u32 = 1 << 18;
/// 左表作为 INLMJ inner。
pub const PreferLeftAsINLMJInner: u32 = 1 << 19;
/// 右表作为 INLMJ inner。
pub const PreferRightAsINLMJInner: u32 = 1 << 20;
/// 左表作为 HJ build。
pub const PreferLeftAsHJBuild: u32 = 1 << 21;
/// 右表作为 HJ build。
pub const PreferRightAsHJBuild: u32 = 1 << 22;
/// 左表作为 HJ probe。
pub const PreferLeftAsHJProbe: u32 = 1 << 23;
/// 右表作为 HJ probe。
pub const PreferRightAsHJProbe: u32 = 1 << 24;
/// 偏好 Hash 聚合。
pub const PreferHashAgg: u32 = 1 << 25;
/// 偏好 Stream 聚合。
pub const PreferStreamAgg: u32 = 1 << 26;
/// 偏好 MPP 一阶段聚合。
pub const PreferMPP1PhaseAgg: u32 = 1 << 27;
/// 偏好 MPP 两阶段聚合。
pub const PreferMPP2PhaseAgg: u32 = 1 << 28;

/// 偏好从 TiKV 读取。
pub const PreferTiKV: u32 = 1 << 0;
/// 偏好从 TiFlash 读取。
pub const PreferTiFlash: u32 = 1 << 1;

// StmtHints are hints that apply to the entire statement, like 'max_exec_time', 'memory_quota'.
// StmtHints 对应 Go 结构体；字段顺序跟随原文件，便于人工逐项核对 statement-level hint 的解析结果。
#[derive(Clone, Debug, Default)]
/// 作用于整条语句的 hint 结果（内存配额、执行时间、计划缓存等）。
pub struct StmtHints {
    // This is true iff there were hints in the statement.
    pub QueryHasHints: bool,

    // Hint Information
    pub MemQuotaQuery: i64,
    pub MaxExecutionTime: u64,
    pub ReplicaRead: u8,
    pub AllowInSubqToJoinAndAgg: bool,
    pub NoIndexMergeHint: bool,
    pub StraightJoinOrder: bool,
    // EnableCascadesPlanner is use cascades planner for a single query only.
    pub EnableCascadesPlanner: bool,
    // ForceNthPlan indicates the PlanCounterTp number for finding physical plan.
    // -1 for disable.
    pub ForceNthPlan: i64,
    pub ResourceGroup: String,
    // Do not store plan in either plan cache.
    pub IgnorePlanCache: bool,
    // Use plan cache under strategy that requires explicit hints.
    pub UsePlanCache: bool,
    pub WriteSlowLog: bool,

    // Hint flags
    pub HasAllowInSubqToJoinAndAggHint: bool,
    pub HasMemQuotaHint: bool,
    pub HasReplicaReadHint: bool,
    pub HasMaxExecutionTime: bool,
    pub HasEnableCascadesPlannerHint: bool,
    pub HasResourceGroup: bool,
    pub SetVars: HashMap<String, String>,

    // Hypo Indexes from Hints
    pub HintedHypoIndexes: HashMap<String, HashMap<String, HashMap<String, model::IndexInfo>>>, // dbName -> tblName -> idxName -> idxInfo

    // the original table hints
    pub OriginalTableHints: Vec<ast::TableOptimizerHint>,
}

impl StmtHints {
    // TaskMapNeedBackUp indicates that whether we need to back up taskMap during physical optimizing.
    // TaskMapNeedBackUp 只由 ForceNthPlan 控制；-1 表示禁用。
    /// 物理优化时是否需要备份 taskMap（由 ForceNthPlan 决定）。
    pub fn TaskMapNeedBackUp(&self) -> bool {
        self.ForceNthPlan != -1
    }

    // Clone the StmtHints struct and returns the pointer of the new one.
    // Clone 对 map 和 hint slice 做浅层元素复制；Go 实现有意不复制 HintedHypoIndexes。
    /// 克隆 StmtHints；有意不复制 HintedHypoIndexes。
    pub fn Clone(&self) -> StmtHints {
        StmtHints {
            QueryHasHints: self.QueryHasHints,
            MemQuotaQuery: self.MemQuotaQuery,
            MaxExecutionTime: self.MaxExecutionTime,
            ReplicaRead: self.ReplicaRead,
            AllowInSubqToJoinAndAgg: self.AllowInSubqToJoinAndAgg,
            NoIndexMergeHint: self.NoIndexMergeHint,
            StraightJoinOrder: self.StraightJoinOrder,
            EnableCascadesPlanner: self.EnableCascadesPlanner,
            ForceNthPlan: self.ForceNthPlan,
            ResourceGroup: self.ResourceGroup.clone(),
            IgnorePlanCache: self.IgnorePlanCache,
            UsePlanCache: self.UsePlanCache,
            WriteSlowLog: self.WriteSlowLog,
            HasAllowInSubqToJoinAndAggHint: self.HasAllowInSubqToJoinAndAggHint,
            HasMemQuotaHint: self.HasMemQuotaHint,
            HasReplicaReadHint: self.HasReplicaReadHint,
            HasMaxExecutionTime: self.HasMaxExecutionTime,
            HasEnableCascadesPlannerHint: self.HasEnableCascadesPlannerHint,
            HasResourceGroup: self.HasResourceGroup,
            SetVars: self.SetVars.clone(),
            HintedHypoIndexes: HashMap::new(),
            OriginalTableHints: self.OriginalTableHints.clone(),
        }
    }

    // addHypoIndex 懒初始化三层 map，保持 Go 中 db->table->index 的组织方式。
    /// 懒初始化三层 map 后写入假设索引。
    pub fn addHypoIndex(
        &mut self,
        db: String,
        tbl: String,
        idx: String,
        idxInfo: model::IndexInfo,
    ) {
        self.HintedHypoIndexes
            .entry(db)
            .or_default()
            .entry(tbl)
            .or_default()
            .insert(idx, idxInfo);
    }
}

/// 从 HintData 取出 SET_VAR 载荷。
fn hintDataSetVar(data: &ast::HintData) -> ast::HintSetVar {
    match data {
        ast::HintData::SetVar(value) => value.clone(),
        other => panic!("SET_VAR expects HintSetVar, got {other:?}"),
    }
}

/// 从 HintData 取出有符号整数。
fn hintDataI64(data: &ast::HintData) -> i64 {
    match data {
        ast::HintData::Signed(value) => *value,
        other => panic!("signed hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出无符号整数。
fn hintDataU64(data: &ast::HintData) -> u64 {
    match data {
        ast::HintData::Unsigned(value) => *value,
        other => panic!("unsigned hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出布尔值。
fn hintDataBool(data: &ast::HintData) -> bool {
    match data {
        ast::HintData::Boolean(value) => *value,
        other => panic!("boolean hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出字符串名。
fn hintDataString(data: &ast::HintData) -> String {
    match data {
        ast::HintData::Name(value) => value.clone(),
        other => panic!("string hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出大小写不敏感标识。
fn hintDataCIStr(data: &ast::HintData) -> ast::CIStr {
    match data {
        ast::HintData::CIStr(value) => value.clone(),
        other => panic!("CIStr hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出时间范围。
fn hintDataTimeRange(data: &ast::HintData) -> ast::HintTimeRange {
    match data {
        ast::HintData::TimeRange(value) => value.clone(),
        other => panic!("time range hint data expected, got {other:?}"),
    }
}

/// 从 HintData 取出 leading 列表（可空）。
fn hintDataLeadingList(data: &ast::HintData) -> Option<ast::LeadingList> {
    match data {
        ast::HintData::Leading(value) => Some(value.clone()),
        ast::HintData::None => None,
        _ => None,
    }
}

/// 构造冲突 hint 的标准 warning 错误。
fn newErrWarnConflictingHint(message: String) -> errors::Error {
    dbterror::ClassOptimizer
        .NewStd(mysql::ErrWarnConflictingHint)
        .FastGenByArgs(&[message.into()])
        .into()
}

// ParseStmtHints parses statement hints.
// ParseStmtHints 解析 statement-level hints，并返回生效 hint 的 offset 与 warning。
/// 解析语句级 hint，返回 StmtHints、生效 offset 与 warnings。
pub fn ParseStmtHints<SetVarChecker, HypoIndexChecker>(
    hints: Vec<ast::TableOptimizerHint>,
    mut setVarHintChecker: SetVarChecker,
    mut hypoIndexChecker: HypoIndexChecker,
    currentDB: String,
    replicaReadFollower: u8,
) -> (StmtHints, Vec<i32>, Vec<errors::Error>)
where
    SetVarChecker: FnMut(String, String) -> (bool, Option<errors::Error>),
    HypoIndexChecker: FnMut(ast::CIStr, ast::CIStr, ast::CIStr) -> (i32, Option<errors::Error>),
{
    let mut stmtHints = StmtHints::default();
    let mut offs = Vec::new();
    let mut warns = Vec::new();
    stmtHints.QueryHasHints = !hints.is_empty();
    let (hints, restrictedHintWarns) =
        filterRestrictedHints(hints, shouldWarnRestrictedHintInParseStmtHints);
    warns.extend(restrictedHintWarns);

    if hints.is_empty() {
        return (stmtHints, offs, warns);
    }

    let mut hintOffs: HashMap<String, i32> = HashMap::with_capacity(hints.len());
    let mut forceNthPlan: Option<ast::TableOptimizerHint> = None;
    let mut memoryQuotaHintCnt = 0;
    let mut useToJAHintCnt = 0;
    let mut useCascadesHintCnt = 0;
    let mut noIndexMergeHintCnt = 0;
    let mut readReplicaHintCnt = 0;
    let mut maxExecutionTimeCnt = 0;
    let mut forceNthPlanCnt = 0;
    let mut straightJoinHintCnt = 0;
    let mut resourceGroupHintCnt = 0;
    let mut setVars = HashMap::new();
    let mut setVarsOffs = Vec::with_capacity(hints.len());

    for (i, hint) in hints.iter().enumerate() {
        match hint.HintName.L.as_str() {
            HintMemoryQuota => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                memoryQuotaHintCnt += 1;
            }
            "resource_group" => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                resourceGroupHintCnt += 1;
            }
            HintUseToja => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                useToJAHintCnt += 1;
            }
            "use_cascades" => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                useCascadesHintCnt += 1;
            }
            HintNoIndexMerge => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                noIndexMergeHintCnt += 1;
            }
            "read_consistent_replica" => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                readReplicaHintCnt += 1;
            }
            HintMaxExecutionTime => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                maxExecutionTimeCnt += 1;
            }
            "nth_plan" => {
                forceNthPlanCnt += 1;
                forceNthPlan = Some(hint.clone());
            }
            HintStraightJoin => {
                hintOffs.insert(hint.HintName.L.clone(), i as i32);
                straightJoinHintCnt += 1;
            }
            "hypo_index" => {
                // to make it simpler, use Tables[0] as table, Tables[1] as index name, and Tables[2:] as column name.
                if hint.Tables.len() < 3 {
                    warns.push(errors::NewNoStackError(
                        "Invalid HYPO_INDEX hint, valid usage: HYPO_INDEX(tableName, indexName, cols...)",
                    ));
                    continue;
                }
                let mut db = hint.Tables[0].DBName.L.clone();
                if db.is_empty() {
                    db = currentDB.clone();
                }
                let tbl = hint.Tables[0].TableName.clone();
                let idx = hint.Tables[1].TableName.clone();
                let mut cols = Vec::new();
                let mut invalid = false;
                for table in hint.Tables.iter().skip(2) {
                    let (offset, err) =
                        hypoIndexChecker(ast::NewCIStr(&db), tbl.clone(), table.TableName.clone());
                    if let Some(err) = err {
                        invalid = true;
                        warns.push(errors::NewNoStackError(format!(
                            "invalid HYPO_INDEX hint: {}",
                            err
                        )));
                        break;
                    }
                    cols.push(model::IndexColumn {
                        Name: table.TableName.clone(),
                        Offset: offset as isize,
                        Length: types::UnspecifiedLength as isize,
                        ..Default::default()
                    });
                }
                if invalid {
                    continue;
                }
                let idxInfo = model::IndexInfo {
                    Name: idx.clone(),
                    Columns: cols,
                    State: model::StatePublic,
                    Tp: model::IndexType::Hypo,
                    ..Default::default()
                };
                stmtHints.addHypoIndex(db, tbl.L.clone(), idx.L.clone(), idxInfo);
            }
            "set_var" => {
                let setVarHint = hintDataSetVar(&hint.HintData);
                // Not all session variables are permitted for use with SET_VAR.
                let (ok, warning) =
                    setVarHintChecker(setVarHint.VarName.clone(), hint.HintName.O.clone());
                if let Some(warning) = warning {
                    warns.push(warning);
                }
                if !ok {
                    continue;
                }

                // 同一语句里同名 SET_VAR 只应用第一个，后续重复项保留 warning。
                if setVars.contains_key(&setVarHint.VarName) {
                    let msg = format!(
                        "{}({}={})",
                        hint.HintName.O, setVarHint.VarName, setVarHint.Value
                    );
                    warns.push(newErrWarnConflictingHint(msg));
                    continue;
                }
                setVars.insert(setVarHint.VarName.clone(), setVarHint.Value.clone());
                setVarsOffs.push(i as i32);
            }
            HintIgnorePlanCache => stmtHints.IgnorePlanCache = true,
            HintUsePlanCache => stmtHints.UsePlanCache = true,
            HintWriteSlowLog => stmtHints.WriteSlowLog = true,
            _ => {}
        }
    }
    stmtHints.OriginalTableHints = hints.clone();
    stmtHints.SetVars = setVars;

    // 以下各 Handle* 块按计数处理重复定义，只保留最后一次生效并生成 warning。
    // Handle MEMORY_QUOTA
    if memoryQuotaHintCnt != 0 {
        let memoryQuotaHint = &hints[*hintOffs.get(HintMemoryQuota).unwrap() as usize];
        if memoryQuotaHintCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "MEMORY_QUOTA() is defined more than once, only the last definition takes effect: MEMORY_QUOTA({})",
                hintDataI64(&memoryQuotaHint.HintData)
            )));
        }
        let memoryQuota = hintDataI64(&memoryQuotaHint.HintData);
        if memoryQuota < 0 {
            hintOffs.remove(HintMemoryQuota);
            warns.push(errors::NewNoStackError(
                "The use of MEMORY_QUOTA hint is invalid, valid usage: MEMORY_QUOTA(10 MB) or MEMORY_QUOTA(10 GB)",
            ));
        } else {
            stmtHints.HasMemQuotaHint = true;
            stmtHints.MemQuotaQuery = memoryQuota;
            if memoryQuota == 0 {
                warns.push(errors::NewNoStackError(
                    "Setting the MEMORY_QUOTA to 0 means no memory limit",
                ));
            }
        }
    }
    // Handle USE_TOJA
    if useToJAHintCnt != 0 {
        let useToJAHint = &hints[*hintOffs.get(HintUseToja).unwrap() as usize];
        if useToJAHintCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "USE_TOJA() is defined more than once, only the last definition takes effect: USE_TOJA({})",
                hintDataBool(&useToJAHint.HintData)
            )));
        }
        stmtHints.HasAllowInSubqToJoinAndAggHint = true;
        stmtHints.AllowInSubqToJoinAndAgg = hintDataBool(&useToJAHint.HintData);
    }
    // Handle USE_CASCADES
    if useCascadesHintCnt != 0 {
        let useCascadesHint = &hints[*hintOffs.get("use_cascades").unwrap() as usize];
        if useCascadesHintCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "USE_CASCADES() is defined more than once, only the last definition takes effect: USE_CASCADES({})",
                hintDataBool(&useCascadesHint.HintData)
            )));
        }
        stmtHints.HasEnableCascadesPlannerHint = true;
        stmtHints.EnableCascadesPlanner = hintDataBool(&useCascadesHint.HintData);
    }
    // Handle NO_INDEX_MERGE
    if noIndexMergeHintCnt != 0 {
        if noIndexMergeHintCnt > 1 {
            warns.push(errors::NewNoStackError(
                "NO_INDEX_MERGE() is defined more than once, only the last definition takes effect",
            ));
        }
        stmtHints.NoIndexMergeHint = true;
    }
    // Handle straight_join
    if straightJoinHintCnt != 0 {
        if straightJoinHintCnt > 1 {
            warns.push(errors::NewNoStackError(
                "STRAIGHT_JOIN() is defined more than once, only the last definition takes effect",
            ));
        }
        stmtHints.StraightJoinOrder = true;
    }
    // Handle READ_CONSISTENT_REPLICA
    if readReplicaHintCnt != 0 {
        if readReplicaHintCnt > 1 {
            warns.push(errors::NewNoStackError("READ_CONSISTENT_REPLICA() is defined more than once, only the last definition takes effect"));
        }
        stmtHints.HasReplicaReadHint = true;
        stmtHints.ReplicaRead = replicaReadFollower;
    }
    // Handle MAX_EXECUTION_TIME
    if maxExecutionTimeCnt != 0 {
        let maxExecutionTime = &hints[*hintOffs.get(HintMaxExecutionTime).unwrap() as usize];
        if maxExecutionTimeCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "MAX_EXECUTION_TIME() is defined more than once, only the last definition takes effect: MAX_EXECUTION_TIME({})",
                hintDataU64(&maxExecutionTime.HintData)
            )));
        }
        stmtHints.HasMaxExecutionTime = true;
        stmtHints.MaxExecutionTime = hintDataU64(&maxExecutionTime.HintData);
    }
    // Handle RESOURCE_GROUP
    if resourceGroupHintCnt != 0 {
        let resourceGroup = &hints[*hintOffs.get("resource_group").unwrap() as usize];
        if resourceGroupHintCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "RESOURCE_GROUP() is defined more than once, only the last definition takes effect: RESOURCE_GROUP({})",
                hintDataString(&resourceGroup.HintData)
            )));
        }
        stmtHints.HasResourceGroup = true;
        stmtHints.ResourceGroup = hintDataString(&resourceGroup.HintData);
    }
    // Handle NTH_PLAN
    if forceNthPlanCnt != 0 {
        let forceNthPlan = forceNthPlan.expect("forceNthPlanCnt checked above");
        if forceNthPlanCnt > 1 {
            warns.push(errors::NewNoStackError(format!(
                "NTH_PLAN() is defined more than once, only the last definition takes effect: NTH_PLAN({})",
                hintDataI64(&forceNthPlan.HintData)
            )));
        }
        stmtHints.ForceNthPlan = hintDataI64(&forceNthPlan.HintData);
        if stmtHints.ForceNthPlan < 1 {
            stmtHints.ForceNthPlan = -1;
            warns.push(errors::NewNoStackError(
                "the hintdata for NTH_PLAN() is too small, hint ignored",
            ));
        }
    } else {
        stmtHints.ForceNthPlan = -1;
    }

    offs.extend(hintOffs.values().copied());
    offs.extend(setVarsOffs);
    // let hint is always ordered, it is convenient to human compare and test.
    offs.sort();
    (stmtHints, offs, warns)
}

// isStmtHint checks whether this hint is a statement-level hint.
// isStmtHint 只识别 ParseStmtHints 里独立作用于整条语句的 hint。
/// 判断是否为语句级 hint。
pub fn isStmtHint(h: &ast::TableOptimizerHint) -> bool {
    matches!(
        h.HintName.L.as_str(),
        HintMaxExecutionTime | HintMemoryQuota | "resource_group"
    )
}

// shouldWarnRestrictedHintInParseStmtHints checks whether ParseStmtHints is the
// right owner for the restricted-hint warning. Some hints, like STRAIGHT_JOIN,
// are still filtered here but warned from ParsePlanHints because subquery
// occurrences only reliably reach that path.
// shouldWarnRestrictedHintInParseStmtHints 决定 restricted hint 的 warning 归属，避免同一限制重复告警。
/// 决定 restricted hint warning 是否由 ParseStmtHints 负责。
pub fn shouldWarnRestrictedHintInParseStmtHints(h: &ast::TableOptimizerHint) -> bool {
    matches!(
        h.HintName.L.as_str(),
        HintMemoryQuota
            | "resource_group"
            | HintUseToja
            | "use_cascades"
            | HintNoIndexMerge
            | "read_consistent_replica"
            | HintMaxExecutionTime
            | "nth_plan"
            | "hypo_index"
            | "set_var"
            | HintIgnorePlanCache
            | HintUsePlanCache
            | HintWriteSlowLog
    )
}

// RestrictedHintChecker returns a non-nil warning when the lower-case hint name
// is restricted and should be stripped.
// RestrictedHintChecker 对应 Go 函数类型；返回 Some(error) 表示该 hint 被限制并应过滤掉。
/// 受限 hint 检查器：返回 Some 表示应过滤并告警。
pub type RestrictedHintChecker = fn(String) -> Option<errors::Error>;

/// 全局受限 hint 检查器（ParseStmtHints/ParsePlanHints 共用）。
static RESTRICTED_HINT_CHECKER: RwLock<Option<RestrictedHintChecker>> = RwLock::new(None);

// RegisterRestrictedHintChecker registers the checker used by ParseStmtHints
// and ParsePlanHints.
/// 注册受限 hint 检查器。
pub fn RegisterRestrictedHintChecker(checker: RestrictedHintChecker) {
    *RESTRICTED_HINT_CHECKER
        .write()
        .expect("restricted hint checker lock poisoned") = Some(checker);
}

// filterRestrictedHints 过滤被外部策略限制的 hint，并按 shouldWarn 决定是否返回 warning。
/// 过滤受限 hint，并按 shouldWarn 收集 warnings。
pub fn filterRestrictedHints(
    hints: Vec<ast::TableOptimizerHint>,
    shouldWarn: fn(&ast::TableOptimizerHint) -> bool,
) -> (Vec<ast::TableOptimizerHint>, Vec<errors::Error>) {
    let checker = *RESTRICTED_HINT_CHECKER
        .read()
        .expect("restricted hint checker lock poisoned");
    let Some(checker) = checker else {
        return (hints, Vec::new());
    };
    if hints.is_empty() {
        return (hints, Vec::new());
    }

    let mut filtered = Vec::with_capacity(hints.len());
    let mut warns = Vec::new();
    for h in hints {
        if let Some(err) = checker(h.HintName.L.clone()) {
            if shouldWarn(&h) {
                warns.push(err);
            }
            continue;
        }
        filtered.push(h);
    }
    (filtered, warns)
}

// IndexJoinHints stores hint information about index nested loop join.
// IndexJoinHints 分别保存 INLJ/INLHJ/INLMJ 三类 join hint 的表列表。
#[derive(Clone, Debug, Default)]
/// Index Nested Loop 系列 join hint 的表列表集合。
pub struct IndexJoinHints {
    pub INLJTables: Vec<HintedTable>,
    pub INLHJTables: Vec<HintedTable>,
    pub INLMJTables: Vec<HintedTable>,
}

// PlanHints are hints that are used to control the optimizer plan choices like 'use_index', 'hash_join'.
// TODO: move ignore_plan_cache, straight_join, no_decorrelate here.
// PlanHints 保存影响物理计划选择的 hint 结果；字段顺序跟随 Go 原结构。
#[derive(Clone, Debug, Default)]
/// 影响物理计划选择的 hint 汇总（join/index/存储/聚合等）。
pub struct PlanHints {
    pub IndexJoin: IndexJoinHints,    // inlj_join, inlhj_join, inlmj_join
    pub NoIndexJoin: IndexJoinHints,  // no_inlj_join, no_inlhj_join, no_inlmj_join
    pub HashJoin: Vec<HintedTable>,   // hash_join
    pub NoHashJoin: Vec<HintedTable>, // no_hash_join
    pub SortMergeJoin: Vec<HintedTable>, // merge_join
    pub NoMergeJoin: Vec<HintedTable>, // no_merge_join
    pub BroadcastJoin: Vec<HintedTable>, // bcj_join
    pub ShuffleJoin: Vec<HintedTable>, // shuffle_join
    pub IndexHintList: Vec<HintedIndex>, // use_index, ignore_index
    pub IndexMergeHintList: Vec<HintedIndex>, // use_index_merge
    pub TiFlashTables: Vec<HintedTable>, // isolation_read_engines(xx=tiflash)
    pub TiKVTables: Vec<HintedTable>, // isolation_read_engines(xx=tikv)
    pub LeadingJoinOrder: Vec<HintedTable>, // leading
    pub LeadingList: Option<ast::LeadingList>, // leading recursive
    pub HJBuild: Vec<HintedTable>,    // hash_join_build
    pub HJProbe: Vec<HintedTable>,    // hash_join_probe
    pub NoIndexLookUpPushDown: Vec<HintedTable>, // no_index_lookup_pushdown

    // Hints belows are not associated with any particular table.
    pub PreferAggType: u32, // hash_agg, merge_agg, agg_to_cop and so on
    pub PreferAggToCop: bool,
    pub PreferLimitToCop: bool, // limit_to_cop
    pub CTEMerge: bool,         // merge
    pub TimeRangeHint: ast::HintTimeRange,
    pub StraightJoinOrder: bool, // straight_join
}

// HintedTable indicates which table this hint should take effect on.
// HintedTable 描述 hint 目标表、分区、query block offset，以及后续是否成功匹配。
#[derive(Clone, Debug, Default)]
/// hint 目标表：库/表/分区、query block offset、是否已匹配。
pub struct HintedTable {
    pub DBName: ast::CIStr,          // the database name
    pub TblName: ast::CIStr,         // the table name
    pub Partitions: Vec<ast::CIStr>, // partition information
    pub SelectOffset: i32,           // the select block offset of this hint
    pub Matched: bool,               // whether this hint is applied successfully
}

impl HintedTable {
    // Match checks whether the hint is matched with the given dbName and tblName.
    // Match 允许 DBName 为 "*" 的跨库 binding 通配，SelectOffset 和表名必须相同。
    /// 匹配表：允许 DBName="*" 通配，表名与 SelectOffset 须一致。
    pub fn Match(&self, other: &HintedTable) -> bool {
        self.SelectOffset == other.SelectOffset
            && self.TblName.L == other.TblName.L
            && (self.DBName.L == other.DBName.L || self.DBName.L == "*" || other.DBName.L == "*")
    }
}

// HintedIndex indicates which index this hint should take effect on.
// HintedIndex 保存 index hint 的原 AST、目标表/分区和是否要求 lookup pushdown。
#[derive(Clone, Debug, Default)]
/// hint 目标索引：表/分区、原 AST IndexHint、lookup pushdown、匹配标记。
pub struct HintedIndex {
    pub DBName: ast::CIStr,                // the database name
    pub TblName: ast::CIStr,               // the table name
    pub Partitions: Vec<ast::CIStr>,       // partition information
    pub IndexHint: Option<ast::IndexHint>, // the original parser index hint structure
    pub PushDownLookUp: bool,              // whether to push down the index lookup
    // Matched indicates whether this index hint
    // has been successfully applied to a DataSource.
    // If an HintedIndex is not Matched after building
    // a Select statement, we will generate a warning for it.
    pub Matched: bool,
}

impl HintedIndex {
    // Match checks whether the hint is matched with the given dbName and tblName.
    // Match 允许 hint DBName 为 "*" 的 universal binding。
    /// 匹配库表名，允许 hint DBName="*"。
    pub fn Match(&self, dbName: ast::CIStr, tblName: ast::CIStr) -> bool {
        self.TblName.L == tblName.L && (self.DBName.L == dbName.L || self.DBName.L == "*")
    }

    // ShouldPushDownIndexLookUp returns whether the hint indicates to push down index lookup.
    // ShouldPushDownIndexLookUp 只有 USE index 且 PushDownLookUp=true 时返回 true。
    /// USE index 且要求 pushdown 时返回 true。
    pub fn ShouldPushDownIndexLookUp(&self) -> bool {
        self.IndexHint
            .as_ref()
            .map(|hint| hint.HintType == ast::HintUse && self.PushDownLookUp)
            .unwrap_or(false)
    }

    // HintTypeString returns the string representation of the hint type.
    // HintTypeString 把 parser 的 IndexHintType 映射回 optimizer hint 名称。
    /// 将 IndexHintType 映射回 optimizer hint 名称。
    pub fn HintTypeString(&self) -> String {
        let Some(index_hint) = self.IndexHint.as_ref() else {
            return String::new();
        };
        match index_hint.HintType {
            ast::HintUse => {
                if self.PushDownLookUp {
                    HintIndexLookUpPushDown.to_string()
                } else {
                    HintUseIndex.to_string()
                }
            }
            ast::HintIgnore => HintIgnoreIndex.to_string(),
            ast::HintForce => HintForceIndex.to_string(),
            _ => String::new(),
        }
    }

    // IndexString formats the IndexHint as DBName.tableName[, indexNames].
    // IndexString 生成 warning 中展示的 DB.table[, idx...] 文本。
    /// 格式化为 warning 用的 `DB.table[, idx...]`。
    pub fn IndexString(&self) -> String {
        let mut indexList = Vec::new();
        if let Some(index_hint) = self.IndexHint.as_ref() {
            for indexName in index_hint.IndexNames.iter() {
                indexList.push(indexName.L.clone());
            }
        }
        let indexListString = if indexList.is_empty() {
            String::new()
        } else {
            format!(", {}", indexList.join(", "))
        };
        format!("{}.{}{}", self.DBName.O, self.TblName.O, indexListString)
    }
}

/// 任一侧命中则标记 Matched，返回是否有匹配。
fn matchTableNames(tables: &[HintedTable], hintTables: &mut [HintedTable]) -> bool {
    let mut hintMatched = false;
    for table in tables {
        for curEntry in hintTables.iter_mut() {
            if curEntry.Match(table) {
                curEntry.Matched = true;
                hintMatched = true;
                break;
            }
        }
    }
    hintMatched
}

/// 匹配单表到 TiKV/TiFlash hint 列表，命中则克隆返回。
fn matchTiKVOrTiFlashTable(
    tableName: &HintedTable,
    hintTables: &mut [HintedTable],
) -> Option<HintedTable> {
    for table in hintTables.iter_mut() {
        if table.Match(tableName) {
            table.Matched = true;
            return Some(table.clone());
        }
    }
    None
}

impl PlanHints {
    // 以下 IfPrefer* 方法都委托给 MatchTableName，保持 Go 中“任一侧命中 hint 表列表即可”的判断。
    /// 是否偏好 Merge Join。
    pub fn IfPreferMergeJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.SortMergeJoin)
    }
    /// 是否偏好 Broadcast Join。
    pub fn IfPreferBroadcastJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.BroadcastJoin)
    }
    /// 是否偏好 Shuffle Join。
    pub fn IfPreferShuffleJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.ShuffleJoin)
    }
    /// 是否偏好 Hash Join。
    pub fn IfPreferHashJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.HashJoin)
    }
    /// 是否命中禁止 Hash Join。
    pub fn IfPreferNoHashJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.NoHashJoin)
    }
    /// 是否命中禁止 Merge Join。
    pub fn IfPreferNoMergeJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.NoMergeJoin)
    }
    /// 是否偏好作为 HJ build。
    pub fn IfPreferHJBuild(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.HJBuild)
    }
    /// 是否偏好作为 HJ probe。
    pub fn IfPreferHJProbe(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.HJProbe)
    }
    /// 是否偏好 INLJ。
    pub fn IfPreferINLJ(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.IndexJoin.INLJTables)
    }
    /// 是否偏好 INLHJ。
    pub fn IfPreferINLHJ(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.IndexJoin.INLHJTables)
    }
    /// 是否偏好 INLMJ。
    pub fn IfPreferINLMJ(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.IndexJoin.INLMJTables)
    }
    /// 是否命中禁止 Index Join。
    pub fn IfPreferNoIndexJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.NoIndexJoin.INLJTables)
    }
    /// 是否命中禁止 Index Hash Join。
    pub fn IfPreferNoIndexHashJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.NoIndexJoin.INLHJTables)
    }
    /// 是否命中禁止 Index Merge Join。
    pub fn IfPreferNoIndexMergeJoin(&mut self, tableNames: Vec<HintedTable>) -> bool {
        matchTableNames(&tableNames, &mut self.NoIndexJoin.INLMJTables)
    }

    // IfPreferTiFlash checks whether the hint hit the need of TiFlash.
    /// 是否命中 TiFlash 读偏好。
    pub fn IfPreferTiFlash(&mut self, tableName: &HintedTable) -> Option<HintedTable> {
        matchTiKVOrTiFlashTable(tableName, &mut self.TiFlashTables)
    }

    // IfPreferTiKV checks whether the hint hit the need of TiKV.
    /// 是否命中 TiKV 读偏好。
    pub fn IfPreferTiKV(&mut self, tableName: &HintedTable) -> Option<HintedTable> {
        matchTiKVOrTiFlashTable(tableName, &mut self.TiKVTables)
    }

    fn matchTiKVOrTiFlash(
        &mut self,
        tableName: &HintedTable,
        hintTables: &mut Vec<HintedTable>,
    ) -> Option<HintedTable> {
        matchTiKVOrTiFlashTable(tableName, hintTables)
    }

    // MatchTableName checks whether the hint hit the need.
    // Only need either side matches one on the list.
    // Even though you can put 2 tables on the list,
    // it doesn't mean optimizer will reorder to make them
    // join directly.
    // Which it joins on with depend on sequence of traverse
    // and without reorder, user might adjust themselves.
    // This is similar to MySQL hints.
    // MatchTableName 遇到匹配项会把 hintTables 里的 Matched 标记为 true，供后续 unmatched warning 使用。
    /// 任一侧命中 hint 表列表则返回 true，并标记 Matched。
    pub fn MatchTableName(
        &mut self,
        tables: Vec<HintedTable>,
        hintTables: &mut Vec<HintedTable>,
    ) -> bool {
        matchTableNames(&tables, hintTables)
    }
}

// ParsePlanHints parses *ast.TableOptimizerHint to PlanHints.
// ParsePlanHints 分派 plan-level hint，产出 PlanHints、subquery hint flags 和错误。
/// 解析计划级 hint，产出 PlanHints 与子查询 hint flags。
pub fn ParsePlanHints(
    hints: Vec<ast::TableOptimizerHint>,
    currentLevel: i32,
    currentDB: String,
    hintProcessor: &mut QBHintHandler,
    straightJoinOrder: bool,
    handlingInSubquery: bool,
    handlingExistsSubquery: bool,
    notHandlingSubquery: bool,
    warnHandler: &mut dyn hintWarnHandler,
) -> Result<(PlanHints, u64), errors::Error> {
    let (hints, restrictedHintWarns) =
        filterRestrictedHints(hints, |h| !shouldWarnRestrictedHintInParseStmtHints(h));
    for warn in restrictedHintWarns {
        warnHandler.SetHintWarningFromError(&warn);
    }

    let mut p = PlanHints::default();
    let mut subQueryHintFlags = 0_u64;
    let mut leadingHintCnt = 0;

    for hint in hints.iter() {
        // Set warning for the hint that requires the table name.
        match hint.HintName.L.as_str() {
            TiDBMergeJoin
            | HintSMJ
            | TiDBIndexNestedLoopJoin
            | HintINLJ
            | HintINLHJ
            | HintINLMJ
            | HintNoHashJoin
            | HintNoMergeJoin
            | TiDBHashJoin
            | HintHJ
            | HintUseIndex
            | HintIgnoreIndex
            | HintForceIndex
            | HintOrderIndex
            | HintNoOrderIndex
            | HintIndexLookUpPushDown
            | HintIndexMerge
            | HintLeading => {
                if hint.Tables.is_empty() {
                    let restored = RestoreTableOptimizerHint(hint);
                    warnHandler.SetHintWarning(format!(
                        "Hint {} is inapplicable. Please specify the table names in the arguments.",
                        restored
                    ));
                    continue;
                }
            }
            _ => {}
        }

        match hint.HintName.L.as_str() {
            TiDBMergeJoin | HintSMJ => {
                p.SortMergeJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            TiDBBroadCastJoin | HintBCJ => {
                p.BroadcastJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintShuffleJoin => {
                p.ShuffleJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            TiDBIndexNestedLoopJoin | HintINLJ => {
                p.IndexJoin.INLJTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintINLHJ => {
                p.IndexJoin.INLHJTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintINLMJ => {
                if !hint.Tables.is_empty() {
                    warnHandler.SetHintWarning(
                        "The INDEX MERGE JOIN hint is deprecated for usage, try other hints."
                            .to_string(),
                    );
                    continue;
                }
            }
            TiDBHashJoin | HintHJ => {
                p.HashJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintNoHashJoin => {
                p.NoHashJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintNoMergeJoin => {
                p.NoMergeJoin.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintNoIndexJoin => {
                p.NoIndexJoin.INLJTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintNoIndexHashJoin => {
                p.NoIndexJoin.INLHJTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintNoIndexMergeJoin => {
                p.NoIndexJoin.INLMJTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintMPP1PhaseAgg => p.PreferAggType |= PreferMPP1PhaseAgg,
            HintMPP2PhaseAgg => p.PreferAggType |= PreferMPP2PhaseAgg,
            HintHashJoinBuild => {
                p.HJBuild.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintHashJoinProbe => {
                p.HJProbe.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                ));
            }
            HintHashAgg => p.PreferAggType |= PreferHashAgg,
            HintStreamAgg => p.PreferAggType |= PreferStreamAgg,
            HintAggToCop => p.PreferAggToCop = true,
            HintNoIndexLookUpPushDown => {
                if !hint.Indexes.is_empty() {
                    warnHandler.SetHintWarning(
                        "hint NO_INDEX_LOOKUP_PUSH_DOWN is inapplicable, only table name without indexes is supported".to_string(),
                    );
                    continue;
                }
                let mut dbName = hint.Tables[0].DBName.clone();
                if dbName.L.is_empty() {
                    dbName = ast::NewCIStr(&currentDB);
                }
                p.NoIndexLookUpPushDown.push(HintedTable {
                    DBName: dbName,
                    TblName: hint.Tables[0].TableName.clone(),
                    ..Default::default()
                });
            }
            HintUseIndex
            | HintIgnoreIndex
            | HintForceIndex
            | HintOrderIndex
            | HintNoOrderIndex
            | HintIndexLookUpPushDown => {
                let mut dbName = hint.Tables[0].DBName.clone();
                if dbName.L.is_empty() {
                    dbName = ast::NewCIStr(&currentDB);
                }
                let mut hintType = ast::HintUse;
                let mut pushDownLookUp = false;
                match hint.HintName.L.as_str() {
                    HintUseIndex => hintType = ast::HintUse,
                    HintIgnoreIndex => hintType = ast::HintIgnore,
                    HintForceIndex => hintType = ast::HintForce,
                    HintOrderIndex => hintType = ast::HintOrderIndex,
                    HintNoOrderIndex => hintType = ast::HintNoOrderIndex,
                    HintIndexLookUpPushDown => {
                        if hint.Indexes.is_empty() {
                            warnHandler.SetHintWarning("hint INDEX_LOOKUP_PUSH_DOWN is inapplicable, the index names should be specified".to_string());
                            continue;
                        }
                        hintType = ast::HintUse;
                        pushDownLookUp = true;
                    }
                    _ => {}
                }
                p.IndexHintList.push(HintedIndex {
                    DBName: dbName,
                    TblName: hint.Tables[0].TableName.clone(),
                    Partitions: hint.Tables[0].PartitionList.clone(),
                    IndexHint: Some(ast::IndexHint {
                        IndexNames: hint.Indexes.clone(),
                        HintType: hintType,
                        HintScope: ast::HintForScan,
                        ..Default::default()
                    }),
                    PushDownLookUp: pushDownLookUp,
                    ..Default::default()
                });
            }
            HintReadFromStorage => match hintDataCIStr(&hint.HintData).L.as_str() {
                HintTiFlash => p.TiFlashTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                )),
                HintTiKV => p.TiKVTables.extend(tableNames2HintTableInfo(
                    &currentDB,
                    &hint.HintName.L,
                    hint.Tables.clone(),
                    hintProcessor,
                    currentLevel,
                    warnHandler,
                )),
                _ => {}
            },
            HintIndexMerge => {
                let mut dbName = hint.Tables[0].DBName.clone();
                if dbName.L.is_empty() {
                    dbName = ast::NewCIStr(&currentDB);
                }
                p.IndexMergeHintList.push(HintedIndex {
                    DBName: dbName,
                    TblName: hint.Tables[0].TableName.clone(),
                    Partitions: hint.Tables[0].PartitionList.clone(),
                    IndexHint: Some(ast::IndexHint {
                        IndexNames: hint.Indexes.clone(),
                        HintType: ast::HintUse,
                        HintScope: ast::HintForScan,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            HintTimeRange => p.TimeRangeHint = hintDataTimeRange(&hint.HintData),
            HintLimitToCop => p.PreferLimitToCop = true,
            HintMerge => {
                if !hint.Tables.is_empty() {
                    warnHandler.SetHintWarning(
                        "The MERGE hint is not used correctly, maybe it inputs a table name."
                            .to_string(),
                    );
                    continue;
                }
                p.CTEMerge = true;
            }
            HintLeading => {
                if leadingHintCnt == 0 {
                    p.LeadingJoinOrder.extend(tableNames2HintTableInfo(
                        &currentDB,
                        &hint.HintName.L,
                        hint.Tables.clone(),
                        hintProcessor,
                        currentLevel,
                        warnHandler,
                    ));
                    if let Some(list) = hintDataLeadingList(&hint.HintData) {
                        p.LeadingList = Some(list);
                    }
                }
                leadingHintCnt += 1;
            }
            HintSemiJoinRewrite => {
                if !handlingExistsSubquery && !handlingInSubquery {
                    warnHandler.SetHintWarning("The SEMI_JOIN_REWRITE hint is not used correctly, maybe it's not in a subquery or the subquery is not IN/EXISTS clause.".to_string());
                    continue;
                }
                subQueryHintFlags |= HintFlagSemiJoinRewrite;
            }
            HintNoDecorrelate => {
                if notHandlingSubquery {
                    warnHandler.SetHintWarning("NO_DECORRELATE() is inapplicable because it's not in an IN subquery, an EXISTS subquery, an ANY/ALL/SOME subquery or a scalar subquery.".to_string());
                    continue;
                }
                subQueryHintFlags |= HintFlagNoDecorrelate;
            }
            HintStraightJoin => p.StraightJoinOrder = true,
            _ => {
                // ignore hints that not implemented
            }
        }
    }

    // leading 与 straight_join 互斥：多条 leading 或并存时清空 LeadingJoinOrder 并告警。
    if leadingHintCnt > 1 || (leadingHintCnt > 0 && straightJoinOrder) {
        // If there are more leading hints or the straight_join hint existes, all leading hints will be invalid.
        p.LeadingJoinOrder.clear();
        if leadingHintCnt > 1 {
            warnHandler.SetHintWarning("We can only use one leading hint at most, when multiple leading hints are used, all leading hints will be invalid".to_string());
        } else if straightJoinOrder {
            warnHandler.SetHintWarning("We can only use the straight_join hint, when we use the leading hint and straight_join hint at the same time, all leading hints will be invalid".to_string());
        }
    }
    Ok((p, subQueryHintFlags))
}

// RemoveDuplicatedHints removes duplicated hints in this hit list.
// RemoveDuplicatedHints 通过恢复后的 hint 字符串去重，保留第一次出现的 hint。
/// 按恢复字符串去重，保留首次出现。
pub fn RemoveDuplicatedHints(hints: Vec<ast::TableOptimizerHint>) -> Vec<ast::TableOptimizerHint> {
    if hints.len() < 2 {
        return hints;
    }
    let mut m = HashSet::with_capacity(hints.len());
    let mut res = Vec::with_capacity(hints.len());
    for hint in hints {
        let key = RestoreTableOptimizerHint(&hint);
        if m.contains(&key) {
            continue;
        }
        m.insert(key);
        res.push(hint);
    }
    res
}

// tableNames2HintTableInfo converts table names to HintedTable.
// tableNames2HintTableInfo 补默认库名、计算 query block offset，并拒绝不支持分区的 join hint。
/// 表名转 HintedTable：补默认库、算 offset，拒绝不支持分区的 join hint。
pub fn tableNames2HintTableInfo(
    currentDB: &str,
    hintName: &str,
    hintTables: Vec<ast::HintTable>,
    p: &QBHintHandler,
    currentOffset: i32,
    warnHandler: &mut dyn hintWarnHandler,
) -> Vec<HintedTable> {
    if hintTables.is_empty() {
        return Vec::new();
    }
    let mut hintTableInfos = Vec::with_capacity(hintTables.len());
    let defaultDBName = ast::NewCIStr(currentDB);
    let mut isInapplicable = false;
    for hintTable in hintTables {
        let mut tableInfo = HintedTable {
            DBName: hintTable.DBName.clone(),
            TblName: hintTable.TableName.clone(),
            Partitions: hintTable.PartitionList.clone(),
            SelectOffset: p.GetHintOffset(&hintTable.QBName, currentOffset),
            ..Default::default()
        };
        if tableInfo.DBName.L.is_empty() {
            tableInfo.DBName = defaultDBName.clone();
        }
        match hintName {
            TiDBMergeJoin
            | HintSMJ
            | TiDBIndexNestedLoopJoin
            | HintINLJ
            | HintINLHJ
            | HintINLMJ
            | TiDBHashJoin
            | HintHJ
            | HintLeading => {
                if !tableInfo.Partitions.is_empty() {
                    isInapplicable = true;
                }
            }
            _ => {}
        }
        hintTableInfos.push(tableInfo);
    }
    if isInapplicable {
        warnHandler.SetHintWarningFromError(&errors::NewNoStackError(format!(
            "Optimizer Hint {} is inapplicable on specified partitions",
            Restore2JoinHint(hintName, hintTableInfos.clone())
        )));
        return Vec::new();
    }
    hintTableInfos
}

// restore2TableHint 格式化 table/partition 列表，是 Join/Index/Storage hint 恢复字符串的公共片段。
/// 格式化表/分区列表片段。
pub fn restore2TableHint(hintTables: Vec<HintedTable>) -> String {
    let mut buffer = String::new();
    for (i, table) in hintTables.iter().enumerate() {
        buffer.push_str(&table.TblName.L);
        if !table.Partitions.is_empty() {
            buffer.push_str(" PARTITION(");
            for (j, partition) in table.Partitions.iter().enumerate() {
                if j > 0 {
                    buffer.push_str(", ");
                }
                buffer.push_str(&partition.L);
            }
            buffer.push(')');
        }
        if i < hintTables.len() - 1 {
            buffer.push_str(", ");
        }
    }
    buffer
}

// Restore2JoinHint restores join hint to string.
// Restore2JoinHint 在无表参数时只返回大写 hint 名，有表参数时生成完整 `/*+ HINT(t) */`。
/// 恢复 join hint 为 `/*+ HINT(t) */` 或大写名。
pub fn Restore2JoinHint(hintType: &str, hintTables: Vec<HintedTable>) -> String {
    if hintTables.is_empty() {
        return hintType.to_uppercase();
    }
    format!(
        "/*+ {}({}) */",
        hintType.to_uppercase(),
        restore2TableHint(hintTables)
    )
}

// Restore2IndexHint restores index hint to string.
// Restore2IndexHint 恢复 index hint，并把 index 名列表附在 table hint 后面。
/// 恢复 index hint 字符串。
pub fn Restore2IndexHint(hintType: &str, hintIndex: HintedIndex) -> String {
    let mut buffer = format!(
        "/*+ {}({}",
        hintType.to_uppercase(),
        restore2TableHint(vec![HintedTable {
            DBName: hintIndex.DBName.clone(),
            TblName: hintIndex.TblName.clone(),
            Partitions: hintIndex.Partitions.clone(),
            ..Default::default()
        }])
    );
    if let Some(index_hint) = hintIndex.IndexHint.as_ref() {
        for (i, indexName) in index_hint.IndexNames.iter().enumerate() {
            if i > 0 {
                buffer.push(',');
            }
            buffer.push_str(" ");
            buffer.push_str(&indexName.L);
        }
    }
    buffer.push_str(") */");
    buffer
}

// Restore2StorageHint restores storage hint to string.
// Restore2StorageHint 按 tiflash/tikv 两组表恢复 READ_FROM_STORAGE hint。
/// 恢复 READ_FROM_STORAGE hint 字符串。
pub fn Restore2StorageHint(
    tiflashTables: Vec<HintedTable>,
    tikvTables: Vec<HintedTable>,
) -> String {
    let mut buffer = format!("/*+ {}(", HintReadFromStorage.to_uppercase());
    if !tiflashTables.is_empty() {
        buffer.push_str("tiflash[");
        buffer.push_str(&restore2TableHint(tiflashTables.clone()));
        buffer.push(']');
        if !tikvTables.is_empty() {
            buffer.push_str(", ");
        }
    }
    if !tikvTables.is_empty() {
        buffer.push_str("tikv[");
        buffer.push_str(&restore2TableHint(tikvTables.clone()));
        buffer.push(']');
    }
    buffer.push_str(") */");
    buffer
}

// ExtractUnmatchedTables extracts unmatched tables from hintTables.
// ExtractUnmatchedTables 返回未被 DataSource/Join 匹配到的原始表名，用于 warning 文案。
/// 提取未匹配表的原始表名。
pub fn ExtractUnmatchedTables(hintTables: Vec<HintedTable>) -> Vec<String> {
    let mut tableNames = Vec::new();
    for table in hintTables {
        if !table.Matched {
            tableNames.push(table.TblName.O.clone());
        }
    }
    tableNames
}

// CollectUnmatchedHintWarnings collects warnings for unmatched hints from this TableHintInfo.
// CollectUnmatchedHintWarnings 汇总 index、join、storage 三类未命中 hint warning。
/// 汇总 index/join/storage 未命中 warning。
pub fn CollectUnmatchedHintWarnings(hintInfo: &PlanHints) -> Vec<String> {
    let mut warnings = Vec::new();
    warnings.extend(collectUnmatchedIndexHintWarning(
        hintInfo.IndexHintList.clone(),
        false,
    ));
    warnings.extend(collectUnmatchedIndexHintWarning(
        hintInfo.IndexMergeHintList.clone(),
        true,
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintINLJ,
        TiDBIndexNestedLoopJoin,
        hintInfo.IndexJoin.INLJTables.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintINLHJ,
        "",
        hintInfo.IndexJoin.INLHJTables.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintINLMJ,
        "",
        hintInfo.IndexJoin.INLMJTables.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintSMJ,
        TiDBMergeJoin,
        hintInfo.SortMergeJoin.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintBCJ,
        TiDBBroadCastJoin,
        hintInfo.BroadcastJoin.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintShuffleJoin,
        HintShuffleJoin,
        hintInfo.ShuffleJoin.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintHJ,
        TiDBHashJoin,
        hintInfo.HashJoin.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintHashJoinBuild,
        "",
        hintInfo.HJBuild.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintHashJoinProbe,
        "",
        hintInfo.HJProbe.clone(),
    ));
    warnings.extend(collectUnmatchedJoinHintWarning(
        HintLeading,
        "",
        hintInfo.LeadingJoinOrder.clone(),
    ));
    warnings.extend(collectUnmatchedStorageHintWarning(
        hintInfo.TiFlashTables.clone(),
        hintInfo.TiKVTables.clone(),
    ));
    warnings
}

/// 收集未匹配的 index hint warning。
pub fn collectUnmatchedIndexHintWarning(
    indexHints: Vec<HintedIndex>,
    usedForIndexMerge: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for hint in indexHints {
        if !hint.Matched {
            let hintTypeString = if usedForIndexMerge {
                HintIndexMerge.to_string()
            } else {
                hint.HintTypeString()
            };
            let errMsg = format!(
                "{}({}) is inapplicable, check whether the table({}.{}) exists",
                hintTypeString,
                hint.IndexString(),
                hint.DBName.O,
                hint.TblName.O,
            );
            warnings.push(errMsg);
        }
    }
    warnings
}

/// 收集未匹配的 join hint warning。
pub fn collectUnmatchedJoinHintWarning(
    joinType: &str,
    joinTypeAlias: &str,
    hintTables: Vec<HintedTable>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let unMatchedTables = ExtractUnmatchedTables(hintTables.clone());
    if unMatchedTables.is_empty() {
        return warnings;
    }
    let joinTypeAlias = if !joinTypeAlias.is_empty() {
        format!(
            " or {}",
            Restore2JoinHint(joinTypeAlias, hintTables.clone())
        )
    } else {
        String::new()
    };

    let errMsg = format!(
        "There are no matching table names for ({}) in optimizer hint {}{}. Maybe you can use the table alias name",
        unMatchedTables.join(", "),
        Restore2JoinHint(joinType, hintTables),
        joinTypeAlias
    );
    warnings.push(errMsg);
    warnings
}

/// 收集未匹配的存储引擎 hint warning。
pub fn collectUnmatchedStorageHintWarning(
    tiflashTables: Vec<HintedTable>,
    tikvTables: Vec<HintedTable>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let unMatchedTiFlashTables = ExtractUnmatchedTables(tiflashTables.clone());
    let unMatchedTiKVTables = ExtractUnmatchedTables(tikvTables.clone());
    if unMatchedTiFlashTables.len() + unMatchedTiKVTables.len() == 0 {
        return warnings;
    }
    let mut all_unmatched = unMatchedTiFlashTables;
    all_unmatched.extend(unMatchedTiKVTables);
    let errMsg = format!(
        "There are no matching table names for ({}) in optimizer hint {}. Maybe you can use the table alias name",
        all_unmatched.join(", "),
        Restore2StorageHint(tiflashTables, tikvTables)
    );
    warnings.push(errMsg);
    warnings
}
