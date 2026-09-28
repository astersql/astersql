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

//! Ingest index recorder matching `br/pkg/restore/ingestrec/ingest_recorder.go`.
//!
//! 中文模块概述：日志备份无法备份 ingest 模式写出的索引 KV，恢复后需据此重放
//! 删/建索引 SQL。`IngestRecorder` 从已同步的 DDL job 收集索引 ID，再结合最新
//! InfoSchema 填充列列表与外键；`Iterate`/`IterateForeignKeys` 供修复阶段消费。
//! 过滤三条件与 Go 一致：必须是 Ingest 重组、AddIndex/AddPrimaryKey/ModifyColumn、
//! 主 job 为 Synced（子 job 允许 Done）。`RewriteTableID` 用于表 ID 重映射。
//! 数据流：TryAddJob 写入粗粒度 IndexID → UpdateIndexInfo 补全可执行 SQL 片段 →
//! 修复阶段先 IterateForeignKeys 再 Iterate 产出索引。本文件仅注释，不改行为。
//! 与 Go 对齐点包括：子 job Done、缺库硬错误、隐藏列表达式内嵌、前缀长度括号。
//! Instant 计时字段保留以对齐 Go 埋点形状，当前未向外暴露耗时。
//! foreignKeyRecordManager 在首次 UpdateIndexInfo 前为 None，IterateForeignKeys 空成功。

use std::collections::HashMap;
use std::time::Instant;

use astersql_errors::SharedError;

use crate::foreign_key::{
    ForeignKeyRecord, ForeignKeyRecordManager, NewForeignKeyRecordManagerForTables,
};
use crate::model_stub::{
    ActionType, CIStr, ColumnArg, Context, GetFinishedModifyColumnArgs, GetFinishedModifyIndexArgs,
    IndexInfo, InfoSchema, Job, JobState, ReorgType, UnspecifiedLength, annotatef, trace,
};

/// IngestIndexInfo records the information used to generate index drop/re-add SQL.
/// 生成 DROP/ADD INDEX SQL 所需快照：库表名、列占位串与参数、是否主键、IndexInfo。
/// `Updated` 仅在 `UpdateIndexInfo` 成功填充后为 true；`Iterate` 会跳过未更新项。
#[derive(Clone, Debug, Default)]
pub struct IngestIndexInfo {
    pub SchemaName: CIStr,
    pub TableName: CIStr,
    // ColumnList 用 `%n` 占位普通列，隐藏生成列则内嵌表达式括号串。
    pub ColumnList: String,
    pub ColumnArgs: Vec<ColumnArg>,
    pub IsPrimary: bool,
    pub IndexInfo: Option<IndexInfo>,
    pub Updated: bool,
}

/// IngestRecorder records indexes that use ingest mode to construct KVs.
/// items：表 ID → 索引 ID → 索引信息；外键管理器在 UpdateIndexInfo 后写入，
/// 修复索引前需先按 IterateForeignKeys 删除约束（与 Go 注释一致）。
#[derive(Default)]
pub struct IngestRecorder {
    items: HashMap<i64, HashMap<i64, IngestIndexInfo>>,
    /// pub(crate) so same-package export_test helpers can read FK maps (Go export_test.go).
    /// 测试用 GetFKRecordMap 依赖该可见性，勿改为私有。
    pub(crate) foreignKeyRecordManager: Option<ForeignKeyRecordManager>,
}

/// Go `New()` package constructor.
/// 包级构造，返回空录制器（items 空、外键管理器未初始化）。
pub fn New() -> IngestRecorder {
    IngestRecorder::New()
}

impl IngestRecorder {
    // 与 Go New 相同：仅分配空 map，不预填外键管理器。
    pub fn New() -> Self {
        Self {
            items: HashMap::new(),
            foreignKeyRecordManager: None,
        }
    }

    // 无 ReorgMeta 或非 Ingest 重组类型的 job 一律忽略。
    fn notIngestJob(job: &Job) -> bool {
        match &job.ReorgMeta {
            None => true,
            Some(meta) => meta.ReorgTp != ReorgType::Ingest,
        }
    }

    // 仅关心会产出 ingest 索引 KV 的三类 DDL；其余 Action 直接跳过。
    fn notReorgTypeJob(job: &Job) -> bool {
        job.Type != ActionType::AddIndex
            && job.Type != ActionType::AddPrimaryKey
            && job.Type != ActionType::ModifyColumn
    }

    // 主 job 要求 Synced；子 job 最终态是 Done（Go 注释：sub jobs Done not Synced）。
    fn notSynced(job: &Job, isSubJob: bool) -> bool {
        (job.State != JobState::Synced) && !(isSubJob && job.State == JobState::Done)
    }

    /// TryAddJob filters ingest index add jobs and records them.
    /// job 为 None 时静默成功；通过三过滤器后按类型解析 finished args 写入 items。
    /// AddPrimaryKey 标记 IsPrimary；ModifyColumn 用 NewIndexIDs；缺 args 则报错。
    /// 写入时 Updated=false，必须再经 UpdateIndexInfo 才能被 Iterate 看见。
    pub fn TryAddJob(&mut self, job: Option<&Job>, isSubJob: bool) -> Result<(), SharedError> {
        let Some(job) = job else {
            // 空 job：与 Go 传 nil 一致，直接成功返回。
            return Ok(());
        };
        // 任一过滤条件成立即忽略，避免误录非 ingest / 未完成 job。
        if Self::notIngestJob(job) || Self::notReorgTypeJob(job) || Self::notSynced(job, isSubJob) {
            return Ok(());
        }

        match job.Type {
            ActionType::AddIndex | ActionType::AddPrimaryKey => {
                let args = GetFinishedModifyIndexArgs(job).map_err(trace)?;
                let tableindexes = self.items.entry(job.TableID).or_default();
                for a in &args.IndexArgs {
                    tableindexes.insert(
                        a.IndexID,
                        IngestIndexInfo {
                            IsPrimary: job.Type == ActionType::AddPrimaryKey,
                            Updated: false,
                            ..Default::default()
                        },
                    );
                }
            }
            ActionType::ModifyColumn => {
                let args = GetFinishedModifyColumnArgs(job).map_err(trace)?;
                let tableindexes = self.items.entry(job.TableID).or_default();
                for idx_id in &args.NewIndexIDs {
                    tableindexes.insert(
                        *idx_id,
                        IngestIndexInfo {
                            IsPrimary: false,
                            Updated: false,
                            ..Default::default()
                        },
                    );
                }
            }
            // Other/DropIndex：过滤器已排除，匹配臂保持完备。
            ActionType::Other | ActionType::DropIndex => {}
        }
        Ok(())
    }

    /// RewriteTableID rewrites the table id of the items.
    /// rewriteFunc 返回 (新 ID, skip)；skip=true 丢弃该项。失败用 annotatef 附带旧表 ID。
    /// 先完整构建新 map 再替换；回调失败时保留原 map，与 Go 语义一致。
    pub fn RewriteTableID<F>(&mut self, mut rewriteFunc: F) -> Result<(), SharedError>
    where
        F: FnMut(i64) -> Result<(i64, bool), SharedError>,
    {
        let mut newItems = HashMap::new();
        for (&tableID, item) in &self.items {
            let (newTableID, skip) = rewriteFunc(tableID)
                .map_err(|err| annotatef(err, format!("failed to rewrite table id: {tableID}")))?;
            if skip {
                // skip=true：恢复侧决定丢弃该表的 ingest 索引记录。
                continue;
            }
            newItems.insert(newTableID, item.clone());
        }
        self.items = newItems;
        Ok(())
    }

    /// UpdateIndexInfo uses the newest schemas to update ingest index information.
    /// 按最新 InfoSchema 填充列列表/库表名，并合并每表 FK 管理器。
    /// 表存在但库缺失视为硬错误；索引不在录制集合时 RemoveForeignKeys 清理无关 FK。
    /// 隐藏列写入 `(expr)`；普通列写 `%n` 并可带前缀长度；完成后 Updated=true。
    pub fn UpdateIndexInfo(
        &mut self,
        ctx: &Context,
        infoSchema: &dyn InfoSchema,
    ) -> Result<(), SharedError> {
        let _start = Instant::now();
        let mut finalForeignKeyManager = ForeignKeyRecordManager::New();
        for (&tableID, tableIndexes) in &mut self.items {
            let Some(tblInfo) = infoSchema.TableInfoByID(tableID) else {
                // 表已消失：跳过，与 Go 一致不报错。
                continue;
            };
            let Some(dbInfo) = infoSchema.SchemaByID(tblInfo.DBID) else {
                return Err(astersql_errors::New(format!(
                    "failed to repair ingest index because table exists but cannot find database.[table-id:{tableID}][db-id:{}]",
                    tblInfo.DBID
                )));
            };
            let mut tableForeignKeyManager =
                NewForeignKeyRecordManagerForTables(ctx, infoSchema, &dbInfo.Name, &tblInfo)
                    .map_err(trace)?;
            for indexInfo in &tblInfo.Indices {
                let Some(index) = tableIndexes.get_mut(&indexInfo.ID) else {
                    // 非录制索引：移除外键，避免误删仍需保留的约束。
                    tableForeignKeyManager.RemoveForeignKeys(&tblInfo, indexInfo);
                    continue;
                };
                let mut columnListBuilder = String::new();
                let mut columnListArgs: Vec<ColumnArg> =
                    Vec::with_capacity(indexInfo.Columns.len());
                let mut isFirst = true;
                for column in &indexInfo.Columns {
                    if !isFirst {
                        columnListBuilder.push(',');
                    }
                    isFirst = false;
                    let col = &tblInfo.Columns[column.Offset as usize];
                    if col.Hidden {
                        // 生成列：SQL 侧直接内嵌表达式，不进入 ColumnArgs。
                        columnListBuilder.push('(');
                        columnListBuilder.push_str(&col.GeneratedExprString);
                        columnListBuilder.push(')');
                    } else {
                        columnListBuilder.push_str("%n");
                        columnListArgs.push(column.Name.O.clone());
                        if column.Length != UnspecifiedLength {
                            // 前缀索引长度写入列列表，与 Go strings.Builder 语义对齐。
                            columnListBuilder.push_str(&format!("({})", column.Length));
                        }
                    }
                }
                index.ColumnList = columnListBuilder;
                index.ColumnArgs = columnListArgs;
                index.IndexInfo = Some(indexInfo.clone());
                index.SchemaName = dbInfo.Name.clone();
                index.TableName = tblInfo.Name.clone();
                index.Updated = true;
            }
            finalForeignKeyManager.Merge(&tableForeignKeyManager);
        }
        self.foreignKeyRecordManager = Some(finalForeignKeyManager);
        Ok(())
    }

    /// Iterate iterates all updated ingest indexes.
    /// 仅回调 Updated=true 的项；回调错误经 trace 包装后向上返回。
    pub fn Iterate<F>(&self, mut f: F) -> Result<(), SharedError>
    where
        F: FnMut(i64, i64, &IngestIndexInfo) -> Result<(), SharedError>,
    {
        for (&tableID, is) in &self.items {
            for (&indexID, info) in is {
                if !info.Updated {
                    continue;
                }
                f(tableID, indexID, info).map_err(trace)?;
            }
        }
        Ok(())
    }

    /// IterateForeignKeys iterates FKs that need to be dropped before repairing indexes.
    /// 管理器未初始化时空操作；否则遍历 fkRecordMap 全部记录。
    pub fn IterateForeignKeys<F>(&self, mut f: F) -> Result<(), SharedError>
    where
        F: FnMut(&ForeignKeyRecord) -> Result<(), SharedError>,
    {
        let Some(mgr) = &self.foreignKeyRecordManager else {
            return Ok(());
        };
        for fkRecord in mgr.fkRecordMap.values() {
            f(fkRecord).map_err(trace)?;
        }
        Ok(())
    }
}
