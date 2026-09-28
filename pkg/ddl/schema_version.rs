// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Schema 版本（schema version）管理。
//
// Schema 版本是集群内信息模式（InfoSchema）的单调递增序号：每次 DDL 提交会
// 生成新版本，各 TiDB 实例通过同步该版本感知元数据变更。本模块提供：
// - 变更动作枚举与影响范围描述（`SchemaDiff`）；
// - 带作业锁的版本递增管理器（`SchemaVersionManager`）；
// - 是否需要校验假定服务端、以及等待版本同步的辅助函数。

use crate::ddl::Job;

/// 会写入 SchemaDiff 的 DDL 动作类别，决定下游如何增量/全量刷新 InfoSchema。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaAction {
    /// 创建单表。
    CreateTable,
    /// 批量创建多表。
    CreateTables,
    /// 截断表（清空数据但保留定义）。
    TruncateTable,
    /// 创建视图。
    CreateView,
    /// 重命名单表。
    RenameTable,
    /// 批量重命名多表。
    RenameTables,
    /// 交换分区：将分区表的某个分区与普通表互换。
    ExchangePartition,
    /// 截断分区。
    TruncatePartition,
    /// 删除表。
    DropTable,
    /// 删除分区。
    DropPartition,
    /// 恢复已删除的表。
    RecoverTable,
    /// 恢复已删除的库（schema）。
    RecoverSchema,
    /// 重组分区定义。
    ReorganizePartition,
    /// 其它分区修改。
    PartitionModify,
    /// 集群闪回（flashback）：将元数据回退到历史时间点。
    FlashbackCluster,
    /// 未归类的其它动作。
    Other,
}
/// SchemaDiff 中描述的单个受影响对象（库/表及其旧 ID）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AffectedOption {
    /// 当前 schema（数据库）ID。
    pub schema_id: i64,
    /// 当前表 ID。
    pub table_id: i64,
    /// 变更前的 schema ID（重命名/恢复等场景使用）。
    pub old_schema_id: i64,
    /// 变更前的表 ID。
    pub old_table_id: i64,
}
/// 一次 DDL 产生的 schema 差分摘要，供 InfoSchema 增量应用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDiff {
    /// 新的 schema 版本号。
    pub version: i64,
    /// 触发本次差分的动作类型。
    pub action: SchemaAction,
    /// 主 schema ID。
    pub schema_id: i64,
    /// 主表 ID。
    pub table_id: i64,
    /// 额外受影响的库表列表。
    pub affected: Vec<AffectedOption>,
    /// 是否需要重建 schema 名到 ID 的映射（恢复库、闪回等）。
    pub regenerate_schema_map: bool,
}
impl Default for SchemaAction {
    fn default() -> Self {
        Self::Other
    }
}
/// 根据动作、版本与作业构造 `SchemaDiff`。
pub fn set_schema_diff(
    action: SchemaAction,
    version: i64,
    job: &Job,
    affected: Vec<AffectedOption>,
) -> SchemaDiff {
    SchemaDiff {
        version,
        action,
        schema_id: job.schema_id,
        table_id: job.table_id,
        // 恢复库与集群闪回会打乱名称映射，需全量重建。
        regenerate_schema_map: matches!(
            action,
            SchemaAction::RecoverSchema | SchemaAction::FlashbackCluster
        ),
        affected,
    }
}
/// 持有当前 schema 版本，并用作业 ID 做互斥锁，避免并发更新交错。
#[derive(Debug, Default)]
pub struct SchemaVersionManager {
    /// 当前已分配的最大版本号。
    current: i64,
    /// 持有锁的 DDL 作业 ID；`None` 表示未锁定。
    locked_by: Option<i64>,
}
impl SchemaVersionManager {
    /// 由指定作业获取版本锁；已被其它作业占用则失败。
    pub fn lock(&mut self, job_id: i64) -> Result<(), String> {
        if self.locked_by.is_some_and(|id| id != job_id) {
            return Err("schema version is locked".into());
        }
        self.locked_by = Some(job_id);
        Ok(())
    }
    /// 加锁后递增版本并生成对应 `SchemaDiff`。
    pub fn update(
        &mut self,
        job: &Job,
        action: SchemaAction,
        affected: Vec<AffectedOption>,
    ) -> Result<SchemaDiff, String> {
        self.lock(job.id)?;
        self.current += 1;
        Ok(set_schema_diff(action, self.current, job, affected))
    }
    /// 若当前锁属于该作业则释放。
    pub fn unlock(&mut self, job_id: i64) {
        if self.locked_by == Some(job_id) {
            self.locked_by = None;
        }
    }
    /// 返回当前 schema 版本号。
    pub fn current(&self) -> i64 {
        self.current
    }
}
/// 物理 schema 对象 ID 的全局上界，与 `metadef.ReservedGlobalIDUpperBound` 一致。
pub const RESERVED_GLOBAL_ID_UPPER_BOUND: i64 = 0x0000_FFFF_FFFF_FFFF;
/// 系统保留 ID 区间的开区间下界，与 `metadef.ReservedGlobalIDLowerBound` 一致。
pub const RESERVED_GLOBAL_ID_LOWER_BOUND: i64 = RESERVED_GLOBAL_ID_UPPER_BOUND - 1000;

/// 按 Go `shouldCheckAssumedServer` 的规则判断是否需要检查假定服务端。
///
/// Classic 内核始终不检查；其它内核仅检查系统保留表 ID。保留区间为
/// `(RESERVED_GLOBAL_ID_LOWER_BOUND, RESERVED_GLOBAL_ID_UPPER_BOUND]`。
pub(crate) fn should_check_assumed_server_for_kernel(job: &Job, is_classic: bool) -> bool {
    !is_classic
        && job.table_id > RESERVED_GLOBAL_ID_LOWER_BOUND
        && job.table_id <= RESERVED_GLOBAL_ID_UPPER_BOUND
}

/// 使用当前编译内核判断作业是否需要检查假定服务端。
#[cfg(windows)]
pub fn should_check_assumed_server(job: &Job) -> bool {
    debug_assert_eq!(
        RESERVED_GLOBAL_ID_LOWER_BOUND,
        astersql_meta_metadef::ReservedGlobalIDLowerBound
    );
    debug_assert_eq!(
        RESERVED_GLOBAL_ID_UPPER_BOUND,
        astersql_meta_metadef::ReservedGlobalIDUpperBound
    );
    should_check_assumed_server_for_kernel(job, astersql_config_kerneltype::IsClassic())
}

/// 精简的非 Windows 构建不链接 kerneltype；其默认配置对应 Classic 内核。
#[cfg(not(windows))]
pub fn should_check_assumed_server(job: &Job) -> bool {
    should_check_assumed_server_for_kernel(job, true)
}
/// 等待观测到的各副本 schema 版本均达到 `target`；否则返回未同步错误。
pub fn wait_version_synced(
    target: i64,
    observed: impl IntoIterator<Item = i64>,
) -> Result<(), String> {
    if observed.into_iter().all(|version| version >= target) {
        Ok(())
    } else {
        Err(format!("schema version {target} is not synced"))
    }
}
