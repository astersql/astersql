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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/snap_client/export_test.rs`对应的测试导出面：Go export_test.go 对等包装，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client/export_test.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 本任务要求至少32行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `GetMinUserTableID 等 re-export`：把 crate 私有辅助提升为测试可见符号，对齐 Go 包级导出。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `MockClient`：注入假 databases 映射的 SnapClient，免真实备份元数据。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `SetDomain`：测试注入 DomainLike，驱动建表/权限路径。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `CreateTablesTest`：AllocTableIDs→CreateTables 串联，并按原表名排序结果。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `ReplaceTables`：暴露 replaceTables：统计/系统表物理加载与 checksum 钩子。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `MockCallSetSpeedLimit`：装配限速回调 + SnapFileImporter + SimpleRestorer 供限速单测。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `NewSnapFileImporterOptionsForTest`：简化 Options 构造：空 cipher/回调，固定扫描并发 0。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `SetRegionScanConcurrency`：测试侧调整 paginateScanRegion 并发。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `PaginateScanRegionForTest`：暴露私有 paginateScanRegion，断言 region 分页边界。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! 中文注释索引结束

//! Go `export_test.go` helpers: re-exports + test-only wrappers
//! (no kv/domain/kvproto/grpcio; Mem* stubs for PD/TiKV/DB).

use std::collections::HashMap;
use std::sync::Arc;

use crate::client::{SetSpeedLimitCallbacks, SnapClient};
use crate::import::{
    KvMode, NewSnapFileImporter, NewSnapFileImporterOptions, RewriteMode, SnapFileImporter,
    SnapFileImporterOptions,
};
use crate::stubs::{
    Context, CreatedTable, DomainLike, ImporterClient, Result, RewriteRules, SimpleRestorer,
    SplitClient, metapb, metautil, model,
};

pub use crate::client::getMinUserTableID as GetMinUserTableID;
pub use crate::import::{GetKeyRangeByMode, GetSSTMetaFromFile};
pub use crate::placement_rule_manager::{
    restoreLabelKey as RestoreLabelKey, restoreLabelValue as RestoreLabelValue,
};
pub use crate::systable_restore::{NewTemporaryTableChecker, NotifyUpdateAllUsersPrivilege};
pub use crate::systable_schema_update::{
    getSchemaVersionFromStatsMeta as GetSchemaVersionFromStatsMeta,
    updateStatsMetaSchema as UpdateStatsMetaSchema,
};
pub use crate::tikv_sender::{
    getFileRangeKey as GetFileRangeKey, getSortedPhysicalTables as GetSortedPhysicalTables,
};

/// Corresponds to Go `MockClient`.
// 仅填充 databases；PD/importer 仍用 ForTest 默认桩。
pub fn MockClient(dbs: HashMap<String, metautil::Database>) -> SnapClient {
    let mut c = crate::client::NewRestoreClientForTest();
    c.databases = dbs;
    c
}

impl SnapClient {
    /// Corresponds to Go `SetDomain`.
    pub fn SetDomain(&mut self, dom: Arc<dyn DomainLike>) {
        self.dom = Some(dom);
    }

    /// Corresponds to Go `CreateTablesTest`.
    // 二次 AllocTableIDs 模拟 Go 测试里重复调用的幂等期望。
    pub fn CreateTablesTest(
        &mut self,
        dom: Arc<dyn DomainLike>,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<(RewriteRules, Vec<model::TableInfo>)> {
        self.dom = Some(dom);
        let _ = self.AllocTableIDs(tables, false, false, None)?;
        let mut tb_mapping = HashMap::<String, usize>::new();
        for (i, table) in tables.iter().enumerate() {
            tb_mapping.insert(table.Info.Name.O.clone(), i);
        }
        let _ = self.AllocTableIDs(tables, false, false, None)?;
        let ctx = Context::Background();
        let created_tables = self.CreateTables(&ctx, tables, new_ts)?;
        let mut rewrite_rules = RewriteRules::default();
        let mut new_tables = Vec::with_capacity(created_tables.len());
        for table in created_tables {
            if let Some(rules) = table.RewriteRule {
                rewrite_rules.Data.extend(rules.Data);
            }
            new_tables.push(table.Table);
        }
        new_tables.sort_by_key(|t| *tb_mapping.get(&t.Name.O).unwrap_or(&usize::MAX));
        Ok((rewrite_rules, new_tables))
    }

    /// Corresponds to Go `ReplaceTables` export wrapping `replaceTables`.
    // execution/notifier 回调由单测注入，用于观察 SQL 执行顺序。
    pub fn ReplaceTables(
        &mut self,
        ctx: &Context,
        created_tables: &[CreatedTable],
        restore_ts: u64,
        load_stats_physical: bool,
        load_sys_table_physical: bool,
        checksum: bool,
        info_schema: &dyn crate::systable_restore::InfoSchema,
        mut execution: impl FnMut(&str) -> Result<()>,
        notifier: impl FnOnce() -> Result<()>,
    ) -> Result<i32> {
        self.replaceTables(
            ctx,
            created_tables,
            restore_ts,
            load_stats_physical,
            load_sys_table_physical,
            checksum,
            info_schema,
            &mut execution,
            notifier,
        )
    }
}

/// Corresponds to Go `MockCallSetSpeedLimit`.
// rateLimit 固定 42，便于 RecordingImporter 断言。
pub fn MockCallSetSpeedLimit(
    ctx: &Context,
    fake_import_client: Arc<dyn ImporterClient>,
    rc: &mut SnapClient,
    concurrency: u32,
) -> Result<()> {
    rc.SetRateLimit(42);
    let closed = std::sync::Arc::new(std::sync::Mutex::new(false));
    let (create_cbs, close_cbs) = SetSpeedLimitCallbacks(
        ctx,
        rc.pdStore.clone(),
        fake_import_client.clone(),
        rc.rateLimit,
        concurrency as usize,
        closed,
    )?;
    let split = rc
        .meta_client
        .clone()
        .unwrap_or_else(|| Arc::new(crate::stubs::MemSplitClient::default()));
    let stores = rc.pdStore.GetAllStores(ctx).unwrap_or_default();
    let opt = NewSnapFileImporterOptions(
        None,
        split,
        fake_import_client,
        None,
        rc.rewriteMode,
        stores,
        128,
        128,
        false,
        create_cbs,
        close_cbs,
    );
    let file_importer = NewSnapFileImporter(ctx, 0, KvMode::TiDBFull, opt)?;
    rc.importer = Some(file_importer);
    rc.restorer = Some(Box::new(SimpleRestorer::new()));
    Ok(())
}

/// Corresponds to Go `NewSnapFileImporterOptionsForTest`.
// create/close 回调为空切片，避免测试误触真实限速 RPC。
pub fn NewSnapFileImporterOptionsForTest(
    split_client: Arc<dyn SplitClient>,
    import_client: Arc<dyn ImporterClient>,
    tikv_stores: Vec<metapb::Store>,
    rewrite_mode: RewriteMode,
    concurrency_per_store: u32,
) -> SnapFileImporterOptions {
    NewSnapFileImporterOptions(
        None,
        split_client,
        import_client,
        None,
        rewrite_mode,
        tikv_stores,
        concurrency_per_store,
        0,
        false,
        Vec::new(),
        Vec::new(),
    )
}

impl SnapFileImporterOptions {
    /// Corresponds to Go `SetRegionScanConcurrency`.
    pub fn SetRegionScanConcurrency(&mut self, concurrency: u32) {
        self.scanConcurrency = concurrency;
    }
}

impl SnapFileImporter {
    /// Corresponds to Go `PaginateScanRegionForTest`.
    // 直接转发私有扫描，保持与生产分页参数一致。
    pub fn PaginateScanRegionForTest(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<Vec<crate::stubs::RegionInfo>> {
        self.paginateScanRegion(ctx, start_key, end_key)
    }
}
