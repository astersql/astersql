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

//! 批量 meta KV 处理器，对齐 Go `batch_meta_processor.go`。
//! 两条路径：真正恢复重写（RestoreMetaKVProcessor）与只读构建映射（MetaKVInfoProcessor）。
//! 均实现 BatchMetaKVProcessor，供 LoadAndProcessMetaKVFilesInBatch 回调。
//! CF 分离后批处理，保证 default/write 依赖顺序与 Go 一致。

// 本文件仅编排批处理与收尾策略，具体写/解析逻辑在 LogClient。
// ProcessBatch 返回值供流水线跨批传递未消费条目。
// hasExplicitFilter 决定 schema 刷新粒度，避免误做全量 reload。
// MetaKVInfoProcessor 与 Restore 路径共享批加载框架，差异仅在回调。
use crate::client::{LoadAndProcessMetaKVFilesInBatch, LogClient, SeparateAndSortFilesByCF};
use crate::log_file_manager::KvEntryWithTS;
use crate::stubs::backuppb::DataFileInfo;
use crate::stubs::log;
use crate::stubs::stream::{
    LogBackupTableHistoryManager, NewTableHistoryManager, NewTableMappingManager, SchemasReplace,
    TableMappingManager,
};
use crate::stubs::{Context, Result};

/// 批处理回调：按 CF 消费一批 meta 文件条目，返回需留给后续批次的残留 entries。
pub trait BatchMetaKVProcessor {
    fn ProcessBatch(
        &mut self,
        ctx: &Context,
        files: &[DataFileInfo],
        entries: Vec<KvEntryWithTS>,
        filterTS: u64,
        cf: &str,
    ) -> Result<Vec<KvEntryWithTS>>;
}

/// 恢复路径处理器：携带 schema 替换规则与进度/统计回调。
pub struct RestoreMetaKVProcessor<'a> {
    pub client: &'a mut LogClient,
    pub schemasReplace: SchemasReplace,
    // 统计回调：(kv 数, 大小) 增量。
    pub updateStats: Box<dyn FnMut(u64, u64) + Send>,
    // 进度条步进回调。
    pub progressInc: Box<dyn FnMut() + Send>,
}

/// 构造恢复处理器；schemasReplace 在整次 restore 中复用。
pub fn NewRestoreMetaKVProcessor<'a>(
    client: &'a mut LogClient,
    schemasReplace: SchemasReplace,
    updateStats: Box<dyn FnMut(u64, u64) + Send>,
    progressInc: Box<dyn FnMut() + Send>,
) -> RestoreMetaKVProcessor<'a> {
    RestoreMetaKVProcessor {
        client,
        schemasReplace,
        updateStats,
        progressInc,
    }
}

impl RestoreMetaKVProcessor<'_> {
    /// 按 CF 分离排序后批量恢复 meta；收尾按是否显式过滤选择全量 reload 或按表刷新。
    pub fn RestoreAndRewriteMetaKVFiles(
        &mut self,
        ctx: &Context,
        hasExplicitFilter: bool,
        files: &[DataFileInfo],
        schemasReplace: &SchemasReplace,
    ) -> Result<()> {
        // 先启动 GC rows loader，与 Go 恢复前准备一致。
        self.client.RunGCRowsLoader(ctx);
        // default/write CF 分开批处理，保证跨 CF 依赖顺序。
        let (filesInDefaultCF, filesInWriteCF) = SeparateAndSortFilesByCF(files);
        log::Info(&format!(
            "start to restore meta files total={} default={} write={}",
            files.len(),
            filesInDefaultCF.len(),
            filesInWriteCF.len()
        ));
        LoadAndProcessMetaKVFilesInBatch(ctx, &filesInDefaultCF, &filesInWriteCF, self)?;
        if !hasExplicitFilter {
            // 无显式过滤：抬升 schema 版本触发全量 reload。
            log::Info("updating schema version to do full reload");
            self.client.UpdateSchemaVersionFullReload(ctx)?;
        } else {
            // 有过滤：仅刷新涉及表的 meta，避免全库抖动。
            log::Info("refreshing schema meta");
            self.client.RefreshMetaForTables(ctx, schemasReplace)?;
        }
        Ok(())
    }
}

impl BatchMetaKVProcessor for RestoreMetaKVProcessor<'_> {
    // 委托 LogClient.RestoreBatchMetaKVFiles：写回并应用 schemasReplace。
    fn ProcessBatch(
        &mut self,
        ctx: &Context,
        files: &[DataFileInfo],
        entries: Vec<KvEntryWithTS>,
        filterTS: u64,
        cf: &str,
    ) -> Result<Vec<KvEntryWithTS>> {
        self.client.RestoreBatchMetaKVFiles(
            ctx,
            files,
            &self.schemasReplace,
            entries,
            filterTS,
            &mut self.updateStats,
            &mut self.progressInc,
            cf,
        )
    }
}

/// 只读信息构建器：解析 meta KV，填充表映射与历史，不写回集群。
pub struct MetaKVInfoProcessor<'a> {
    pub client: &'a mut LogClient,
    pub tableHistoryManager: LogBackupTableHistoryManager,
    pub tableMappingManager: TableMappingManager,
}

/// 初始化空的 history/mapping 管理器。
pub fn NewMetaKVInfoProcessor<'a>(client: &'a mut LogClient) -> MetaKVInfoProcessor<'a> {
    MetaKVInfoProcessor {
        client,
        tableHistoryManager: NewTableHistoryManager(),
        tableMappingManager: NewTableMappingManager(),
    }
}

impl MetaKVInfoProcessor<'_> {
    /// 扫描 meta 文件构建映射；结束后清理临时 KV，避免泄漏中间状态。
    pub fn ReadMetaKVFilesAndBuildInfo(
        &mut self,
        ctx: &Context,
        files: &[DataFileInfo],
    ) -> Result<()> {
        let (filesInDefaultCF, filesInWriteCF) = SeparateAndSortFilesByCF(files);
        LoadAndProcessMetaKVFilesInBatch(ctx, &filesInDefaultCF, &filesInWriteCF, self)?;
        // 批处理可能留下临时键，显式清理。
        self.tableMappingManager.CleanTempKV();
        Ok(())
    }

    /// 暴露表 ID 映射，供上层 rewrite 决策读取。
    pub fn GetTableMappingManager(&self) -> &TableMappingManager {
        &self.tableMappingManager
    }

    /// 暴露表历史管理器，供 DDL 轨迹查询。
    pub fn GetTableHistoryManager(&self) -> &LogBackupTableHistoryManager {
        &self.tableHistoryManager
    }
}

impl BatchMetaKVProcessor for MetaKVInfoProcessor<'_> {
    // 过滤排序后解析 meta，更新 ID 映射与历史；返回 filtered 残留给下游。
    fn ProcessBatch(
        &mut self,
        ctx: &Context,
        files: &[DataFileInfo],
        entries: Vec<KvEntryWithTS>,
        filterTS: u64,
        cf: &str,
    ) -> Result<Vec<KvEntryWithTS>> {
        // filterTS 丢弃过新条目；跨文件残留 entries 继续参与排序。
        let (curSortedEntries, filteredEntries) = self
            .client
            .filterAndSortKvEntriesFromFiles(ctx, files, entries, filterTS)?;
        for entry in &curSortedEntries {
            // 按 CF 与 TS 解析，维护 backup→restore 表 ID 映射。
            self.tableMappingManager.ParseMetaKvAndUpdateIdMapping(
                &entry.E,
                cf,
                entry.Ts,
                &mut self.tableHistoryManager,
            )?;
        }
        Ok(filteredEntries)
    }
}
