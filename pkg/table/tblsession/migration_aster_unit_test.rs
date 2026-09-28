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

// 迁移期单元测试：对齐 Go tblsession.MutateContext 的会话字段转发与可选支持接口。
//
// 覆盖表达式上下文透传、行编码配置、分配器替代路径、统计/缓存表/临时表
// 以及缺失 reserved allocator 时的安全返回。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use contextutil::{SQLWarn, WarnAppender, WarnHandler};
use exprctx::{BuildContext, EvalContext, ExprContext, ParamError, ParamValues, UserVarsReader};
use tblctx::{
    AllocatorContext as _, CachedTableSupport as _, ExchangePartitionDMLSupport as _,
    MutateContext as _, StatisticsSupport as _, TemporaryTable as _, TemporaryTableSupport as _,
};

use super::*;

#[derive(Default)]
/// 空用户变量读取器桩。
struct EmptyUserVars;

impl UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<exprctx::types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<exprctx::types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn UserVarsReader> {
        Box::new(Self)
    }
}

/// 测试用求值上下文（类型/错误上下文与警告收集）。
struct TestEvalContext {
    type_ctx: exprctx::types::Context,
    err_ctx: exprctx::errctx::Context,
    warnings: contextutil::StaticWarnHandler,
    user_vars: EmptyUserVars,
}

impl WarnAppender for TestEvalContext {
    fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil::errors::SharedError) {
        self.warnings.AppendNote(error);
    }
}

impl WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        self.warnings.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn> {
        self.warnings.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<SQLWarn>) -> Vec<SQLWarn> {
        self.warnings.CopyWarnings(destination)
    }
}

impl ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<exprctx::types::Datum, ParamError> {
        Err(ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        1
    }
    fn SQLMode(&self) -> exprctx::mysql::SQLMode {
        exprctx::mysql::SQLMode::default()
    }
    fn TypeCtx(&self) -> exprctx::types::Context {
        self.type_ctx.clone()
    }
    fn ErrCtx(&self) -> exprctx::errctx::Context {
        self.err_ctx.clone()
    }
    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }
    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        Ok(chrono::Utc::now().with_timezone(&chrono_tz::UTC))
    }
    fn CurrentDB(&self) -> String {
        "test".to_owned()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }
    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }
    fn GetUserVarsReader(&self) -> &dyn UserVarsReader {
        &self.user_vars
    }
    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        Default::default()
    }
    fn GetOptionalPropProvider(
        &self,
        _key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        None
    }
}

/// 测试用表达式构建/求值上下文。
struct TestExprContext {
    eval: TestEvalContext,
    rng: Box<exprctx::mathutil::MysqlRng>,
}

impl BuildContext for TestExprContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.eval
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".into(), "utf8mb4_bin".into())
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".into()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        "aes-128-ecb".into()
    }
    fn GetSysdateIsNow(&self) -> bool {
        false
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }
    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        &self.rng
    }
    fn IsUseCache(&self) -> bool {
        true
    }
    fn SetSkipPlanCache(&self, _reason: &str) {}
    fn AllocPlanColumnID(&self) -> i64 {
        1
    }
    fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    fn ConnectionID(&self) -> u64 {
        12345
    }
    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }
}

impl ExprContext for TestExprContext {
    fn GetWindowingUseHighPrecision(&self) -> bool {
        true
    }
    fn GetGroupConcatMaxLen(&self) -> u64 {
        1024
    }
}

#[derive(Default)]
/// 空 infoschema 桩，仅满足 MetaOnlyInfoSchema。
struct TestInfoSchema;

impl infoschema::SchemaAndTable for TestInfoSchema {
    type Context = ();
    type Error = Infallible;

    fn AllSchemas(&self) -> Vec<Arc<infoschema::model::DBInfo>> {
        Vec::new()
    }
    fn SchemaTableInfos(
        &self,
        _ctx: &(),
        _schema: &infoschema::ast::CIStr,
    ) -> Result<Vec<Arc<infoschema::model::TableInfo>>, Infallible> {
        Ok(Vec::new())
    }
}

impl infoschema::Misc for TestInfoSchema {
    fn PolicyByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::PolicyInfo>> {
        None
    }
    fn ResourceGroupByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::ResourceGroupInfo>> {
        None
    }
    fn MaskingPolicyByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::MaskingPolicyInfo>> {
        None
    }
    fn MaskingPolicyByTableColumn(
        &self,
        _table_id: i64,
        _column_id: i64,
    ) -> Option<Arc<infoschema::model::MaskingPolicyInfo>> {
        None
    }
    fn PlacementBundleByPhysicalTableID(
        &self,
        _id: i64,
    ) -> Option<Arc<infoschema::placement::Bundle>> {
        None
    }
    fn AllPlacementBundles(&self) -> Vec<Arc<infoschema::placement::Bundle>> {
        Vec::new()
    }
    fn AllPlacementPolicies(&self) -> Vec<Arc<infoschema::model::PolicyInfo>> {
        Vec::new()
    }
    fn ClonePlacementPolicies(&self) -> HashMap<String, Arc<infoschema::model::PolicyInfo>> {
        HashMap::new()
    }
    fn AllMaskingPolicies(&self) -> Vec<Arc<infoschema::model::MaskingPolicyInfo>> {
        Vec::new()
    }
    fn AllResourceGroups(&self) -> Vec<Arc<infoschema::model::ResourceGroupInfo>> {
        Vec::new()
    }
    fn CloneResourceGroups(&self) -> HashMap<String, Arc<infoschema::model::ResourceGroupInfo>> {
        HashMap::new()
    }
    fn HasTemporaryTable(&self) -> bool {
        false
    }
}

impl infoschema::MetaOnlyInfoSchema for TestInfoSchema {
    fn SchemaMetaVersion(&self) -> i64 {
        7
    }
    fn SchemaByName(
        &self,
        _schema: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::DBInfo>> {
        None
    }
    fn SchemaExists(&self, _schema: &infoschema::ast::CIStr) -> bool {
        false
    }
    fn TableInfoByName(
        &self,
        _schema: &infoschema::ast::CIStr,
        _table: &infoschema::ast::CIStr,
    ) -> Result<Arc<infoschema::model::TableInfo>, Infallible> {
        unreachable!()
    }
    fn TableInfoByID(&self, _id: i64) -> Option<Arc<infoschema::model::TableInfo>> {
        None
    }
    fn FindTableInfoByPartitionID(
        &self,
        _partition_id: i64,
    ) -> Option<(
        Arc<infoschema::model::TableInfo>,
        Arc<infoschema::model::DBInfo>,
        Arc<infoschema::model::PartitionDefinition>,
    )> {
        None
    }
    fn TableExists(
        &self,
        _schema: &infoschema::ast::CIStr,
        _table: &infoschema::ast::CIStr,
    ) -> bool {
        false
    }
    fn SchemaByID(&self, _id: i64) -> Option<Arc<infoschema::model::DBInfo>> {
        None
    }
    fn AllSchemaNames(&self) -> Vec<infoschema::ast::CIStr> {
        Vec::new()
    }
    fn SchemaSimpleTableInfos(
        &self,
        _ctx: &(),
        _schema: &infoschema::ast::CIStr,
    ) -> Result<Vec<Arc<infoschema::model::TableNameInfo>>, Infallible> {
        Ok(Vec::new())
    }
    fn ListTablesWithSpecialAttribute(
        &self,
        _filter: infoschema::SpecialAttributeFilter,
    ) -> Vec<infoschema::TableInfoResult> {
        Vec::new()
    }
    fn GetTableReferredForeignKeys(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Vec<Arc<infoschema::model::ReferredFKInfo>> {
        Vec::new()
    }
}

/// 临时表共享状态：元数据、脏大小、modified 与可选分配器。
struct SharedTempState {
    meta: model::TableInfo,
    size: AtomicI64,
    modified: AtomicBool,
    allocator: Option<Arc<dyn autoid::Allocator>>,
}

#[derive(Clone)]
/// 可 Clone 的会话临时表桩（共享底层状态）。
struct TestTemporaryTable(Arc<SharedTempState>);

impl tblctx::TemporaryTable for TestTemporaryTable {
    fn GetMeta(&self) -> &model::TableInfo {
        &self.0.meta
    }
    fn GetSize(&self) -> i64 {
        self.0.size.load(Ordering::SeqCst)
    }
    fn SetSize(&mut self, size: i64) {
        self.0.size.store(size, Ordering::SeqCst);
    }
}

impl SessionTemporaryTable for TestTemporaryTable {
    fn GetAutoIDAllocator(&self) -> Option<Arc<dyn autoid::Allocator>> {
        self.0.allocator.clone()
    }
    fn SetModified(&mut self, modified: bool) {
        self.0.modified.store(modified, Ordering::SeqCst);
    }
    fn GetModified(&self) -> bool {
        self.0.modified.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
/// 已提交大小 = tableID*1_000_000 + 可变 size。
struct TestTemporaryData(Arc<AtomicI64>);

impl tblctx::TemporaryTableData for TestTemporaryData {
    fn GetTableSize(&self, table_id: i64) -> i64 {
        table_id * 1_000_000 + self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
/// RowID 分片生成器桩。
struct TestShardGenerator {
    value: u64,
}

/// 实现 SessionContext 的完整测试会话。
struct TestSessionContext {
    expr: TestExprContext,
    info_schema: TestInfoSchema,
    write_bufs: Option<variable::WriteStmtBufs>,
    connection_id: u64,
    restricted_sql: bool,
    assertion_level: variable::AssertionLevel,
    mutation_checker: bool,
    row_checksum: bool,
    row_encoder: bool,
    shard: TestShardGenerator,
    reserved: Option<stmtctx::ReservedRowIDAlloc>,
    txn: Option<TransactionContext>,
    temp_table: Option<TestTemporaryTable>,
    temp_data: TestTemporaryData,
    temp_size_limit: i64,
}

impl SessionContext for TestSessionContext {
    type ExprContext = TestExprContext;
    type InfoSchema = TestInfoSchema;
    type RowIDShardGenerator = TestShardGenerator;
    type TemporaryTable = TestTemporaryTable;

    fn GetExprCtx(&self) -> &Self::ExprContext {
        &self.expr
    }
    fn GetLatestInfoSchema(&self) -> &Self::InfoSchema {
        &self.info_schema
    }
    fn TakeWriteStmtBufs(&mut self) -> variable::WriteStmtBufs {
        self.write_bufs.take().unwrap()
    }
    fn ConnectionID(&self) -> u64 {
        self.connection_id
    }
    fn InRestrictedSQL(&self) -> bool {
        self.restricted_sql
    }
    fn TxnAssertionLevel(&self) -> variable::AssertionLevel {
        self.assertion_level
    }
    fn EnableMutationChecker(&self) -> bool {
        self.mutation_checker
    }
    fn EnableRowLevelChecksum(&self) -> bool {
        self.row_checksum
    }
    fn RowEncoderEnabled(&self) -> bool {
        self.row_encoder
    }
    fn GetRowIDShardGenerator(&mut self) -> &mut Self::RowIDShardGenerator {
        &mut self.shard
    }
    fn GetReservedRowIDAlloc(&mut self) -> Option<&mut stmtctx::ReservedRowIDAlloc> {
        self.reserved.as_mut()
    }
    fn HasTxnContext(&self) -> bool {
        self.txn.is_some()
    }
    fn GetTxnContextMut(&mut self) -> Option<&mut TransactionContext> {
        self.txn.as_mut()
    }
    fn GetTemporaryTable(&self, tbl: &model::TableInfo) -> Option<Self::TemporaryTable> {
        self.temp_table
            .as_ref()
            .filter(|temp| temp.GetMeta().ID == tbl.ID)
            .cloned()
    }
    fn TemporaryTableDataForHandler(&self) -> Option<Box<dyn tblctx::TemporaryTableData>> {
        Some(Box::new(self.temp_data.clone()))
    }
    fn GetTemporaryTableSizeLimit(&self) -> i64 {
        self.temp_size_limit
    }
}

/// 构造带全局临时表与 TxnCtx 的默认测试会话。
fn new_session() -> TestSessionContext {
    // 组装类型/错误上下文与带自增分配器的全局临时表。
    let type_warnings: Arc<dyn WarnAppender + Send + Sync> = Arc::new(contextutil::ignoreWarn {});
    let type_ctx = types_crate::scalar::NewContext(
        exprctx::types::DefaultStmtFlags,
        chrono_tz::UTC,
        type_warnings,
    );
    let levels = [exprctx::errctx::Level::LevelError; exprctx::errctx::errGroupCount];
    let err_warnings: errctx_crate::errctx::WarnAppenderRef =
        Arc::new(errctx_crate::warn::ignoreWarn {});
    let err_ctx = errctx_crate::errctx::NewContextWithLevels(levels, err_warnings);
    let allocator: Arc<dyn autoid::Allocator> = Arc::new(autoid::InMemoryAllocator::new(
        false,
        autoid::AllocatorType::RowId,
    ));
    let temp_table = TestTemporaryTable(Arc::new(SharedTempState {
        meta: model::TableInfo {
            ID: 456,
            TempTableType: model::TempTableGlobal,
            ..Default::default()
        },
        size: AtomicI64::new(0),
        modified: AtomicBool::new(false),
        allocator: Some(allocator),
    }));
    TestSessionContext {
        expr: TestExprContext {
            eval: TestEvalContext {
                type_ctx,
                err_ctx,
                warnings: contextutil::NewStaticWarnHandler(0),
                user_vars: EmptyUserVars,
            },
            rng: mathutil_crate::NewWithSeed(1),
        },
        info_schema: TestInfoSchema,
        write_bufs: Some(variable::WriteStmtBufs::default()),
        connection_id: 12345,
        restricted_sql: false,
        assertion_level: variable::AssertionLevel::AssertionLevelFast,
        mutation_checker: true,
        row_checksum: true,
        row_encoder: true,
        shard: TestShardGenerator::default(),
        reserved: Some(stmtctx::ReservedRowIDAlloc::default()),
        txn: Some(TransactionContext::default()),
        temp_table: Some(temp_table),
        temp_data: TestTemporaryData(Arc::new(AtomicI64::new(0))),
        temp_size_limit: 64 << 20,
    }
}

#[test]
/// 对齐 Go：会话字段透传、行编码配置与全局临时表替代分配器。
fn session_fields_and_allocator_paths_match_go() {
    let mut ctx = NewMutateContext(new_session());

    assert!(std::ptr::eq(ctx.Context.GetExprCtx(), ctx.GetExprCtx()));
    assert_eq!(ctx.ConnectionID(), 12345);
    assert!(!ctx.InRestrictedSQL());
    ctx.Context.restricted_sql = true;
    assert!(ctx.InRestrictedSQL());
    ctx.Context.restricted_sql = false;
    assert_eq!(
        ctx.TxnAssertionLevel(),
        variable::AssertionLevel::AssertionLevelFast
    );
    ctx.Context.assertion_level = variable::AssertionLevel::AssertionLevelStrict;
    assert_eq!(
        ctx.TxnAssertionLevel(),
        variable::AssertionLevel::AssertionLevelStrict
    );
    assert!(ctx.EnableMutationChecker());
    ctx.Context.mutation_checker = false;
    assert!(!ctx.EnableMutationChecker());
    assert!(ctx.GetRowEncodingConfig().IsRowLevelChecksumEnabled);
    assert!(ctx.GetRowEncodingConfig().RowEncoder.unwrap().Enable);
    assert_eq!(ctx.GetMutateBuffers().GetWriteStmtBufs().RowValBuf.len(), 0);
    ctx.GetRowIDShardGenerator().value = 9;
    assert_eq!(ctx.Context.shard.value, 9);
    assert!(ctx.GetReservedRowIDAlloc().1);

    // 全局临时表应返回替代 Allocators；普通表与缺失表返回 false。
    let global = model::TableInfo {
        ID: 456,
        TempTableType: model::TempTableGlobal,
        ..Default::default()
    };
    let (allocators, ok) = ctx.AlternativeAllocators(&global);
    assert!(ok);
    assert_eq!(allocators.len(), 1);

    let normal = model::TableInfo {
        ID: 456,
        TempTableType: model::TempTableNone,
        ..Default::default()
    };
    assert!(!ctx.AlternativeAllocators(&normal).1);
    let missing = model::TableInfo {
        ID: 999,
        TempTableType: model::TempTableGlobal,
        ..Default::default()
    };
    assert!(!ctx.AlternativeAllocators(&missing).1);

    ctx.Context.restricted_sql = true;
    assert!(!ctx.GetRowEncodingConfig().IsRowLevelChecksumEnabled);
    ctx.Context.restricted_sql = false;
    ctx.Context.row_encoder = false;
    assert!(!ctx.GetRowEncodingConfig().IsRowLevelChecksumEnabled);
}

#[test]
/// 对齐 Go：统计/缓存表/临时表支持及交换分区 infoschema。
fn optional_supports_and_mutations_match_go() {
    let mut ctx = NewMutateContext(new_session());

    // 无 TxnCtx 时可选支持均不可用。
    ctx.Context.txn = None;
    assert!(!ctx.GetStatisticsSupport().1);
    assert!(!ctx.GetCachedTableSupport().1);
    assert!(!ctx.GetTemporaryTableSupport().1);

    ctx.Context.txn = Some(TransactionContext::default());
    ctx.GetStatisticsSupport()
        .0
        .unwrap()
        .UpdatePhysicalTableDelta(12, 1, 2);
    ctx.GetStatisticsSupport()
        .0
        .unwrap()
        .UpdatePhysicalTableDelta(12, 3, 4);
    let delta = &ctx.Context.txn.as_ref().unwrap().TableDeltaMap[&12];
    assert_eq!((delta.Delta, delta.Count), (4, 6));

    // CachedTables：第二次插入同 ID 不覆盖首次 handle。
    ctx.GetCachedTableSupport()
        .0
        .unwrap()
        .AddCachedTableHandleToTxn(123, Box::new("first".to_owned()));
    ctx.GetCachedTableSupport()
        .0
        .unwrap()
        .AddCachedTableHandleToTxn(123, Box::new("second".to_owned()));
    let cached = ctx.Context.txn.as_ref().unwrap().CachedTables[&123]
        .downcast_ref::<String>()
        .unwrap();
    assert_eq!(cached, "first");

    // 临时表加入事务：标记 modified，并验证 committed/dirty 大小语义。
    let temp = ctx.Context.temp_table.as_ref().unwrap().clone();
    let data_size = Arc::clone(&ctx.Context.temp_data.0);
    let table_info = temp.GetMeta().clone();
    let (mut handler, ok) = ctx
        .GetTemporaryTableSupport()
        .0
        .unwrap()
        .AddTemporaryTableToTxn(&table_info);
    assert!(ok);
    let mut handler = handler.take().unwrap();
    assert!(temp.GetModified());
    assert_eq!(handler.GetCommittedSize(), 456_000_000);
    data_size.store(111, Ordering::SeqCst);
    assert_eq!(handler.GetCommittedSize(), 456_000_111);
    assert_eq!(handler.GetDirtySize(), 0);
    handler.UpdateTxnDeltaSize(333);
    handler.UpdateTxnDeltaSize(-1);
    assert_eq!(handler.GetDirtySize(), 332);
    assert_eq!(ctx.GetTemporaryTableSizeLimit(), 64 << 20);

    let context_info_schema = ctx.Context.GetLatestInfoSchema() as *const _;
    let (exchange, ok) = ctx.GetExchangePartitionDMLSupport();
    assert!(ok);
    assert_eq!(
        exchange.unwrap().GetInfoSchemaToCheckExchangeConstraint() as *const _,
        context_info_schema
    );
}

#[test]
/// 缺失 reserved allocator 或临时表时安全返回 false/None。
fn missing_reserved_allocator_and_temporary_table_are_safe() {
    let mut session = new_session();
    session.reserved = None;
    session.temp_table = None;
    let mut ctx = NewMutateContext(session);

    let (reserved, ok) = ctx.GetReservedRowIDAlloc();
    assert!(reserved.is_none());
    assert!(!ok);
    let table = model::TableInfo {
        ID: 456,
        TempTableType: model::TempTableGlobal,
        ..Default::default()
    };
    assert!(!ctx.AddTemporaryTableToTxn(&table).1);
}
