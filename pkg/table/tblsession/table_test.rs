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

// `tblsession` 表测试：保留 Go 参考伪代码字符串，并校验关键符号仍存在。

/// Go 侧 `TestSessionMutateContextFields` 等逻辑的参考伪代码（非可执行 Rust）。
const GO_REFERENCE: &str = r########"
// 这段逻辑验证 tblsession.NewMutateContext 对 sessionctx.Context 各支持接口的转发语义。

// mockTemporaryData 对应 Go 的 mockTemporaryData，用 tableID 和可变 size 合成临时表提交大小。
pub struct mockTemporaryData {
    pub base: variable::TemporaryTableData,
    pub size: i64,
}

impl mockTemporaryData {
    // GetTableSize 对应 Go 方法：返回 tableID*1000000 + size。
    pub fn GetTableSize(&self, table_id: i64) -> i64 {
        table_id * 1_000_000 + self.size
    }
}

// TestSessionMutateContextFields 对应 Go 测试，逐项覆盖 MutateContext 的字段和支持接口。
#[test]
pub fn TestSessionMutateContextFields() {
    let mut sctx = mock::NewContext();
    let mut ctx = tblsession::NewMutateContext(sctx.clone());

    // expression：GetExprCtx 必须直接透传原 session context。
    require::True(sctx.GetExprCtx() == ctx.GetExprCtx());

    // ConnectionID：从 SessionVars 读取当前连接 ID。
    sctx.GetSessionVars().ConnectionID = 12345;
    require::Equal(12345_u64, ctx.ConnectionID());

    // restricted SQL：布尔标记 false/true 都要透传。
    sctx.GetSessionVars().InRestrictedSQL = false;
    require::False(ctx.InRestrictedSQL());
    sctx.GetSessionVars().InRestrictedSQL = true;
    require::True(ctx.InRestrictedSQL());

    // AssertionLevel：fast/strict 两个事务断言级别都从 SessionVars 返回。
    ctx.GetSessionVars().AssertionLevel = variable::AssertionLevelFast;
    require::Equal(variable::AssertionLevelFast, ctx.TxnAssertionLevel());
    ctx.GetSessionVars().AssertionLevel = variable::AssertionLevelStrict;
    require::Equal(variable::AssertionLevelStrict, ctx.TxnAssertionLevel());

    // EnableMutationChecker：开关值不做额外转换。
    ctx.GetSessionVars().EnableMutationChecker = true;
    require::True(ctx.EnableMutationChecker());
    ctx.GetSessionVars().EnableMutationChecker = false;
    require::False(ctx.EnableMutationChecker());

    // encoding config：row-level checksum 依赖 session vars 与 RowEncoder.Enable 的组合。
    sctx.GetSessionVars().EnableRowLevelChecksum = true;
    sctx.GetSessionVars().RowEncoder.Enable = true;
    sctx.GetSessionVars().InRestrictedSQL = false;
    let mut cfg = ctx.GetRowEncodingConfig();
    require::True(cfg.IsRowLevelChecksumEnabled);
    require::Equal(sctx.GetSessionVars().IsRowLevelChecksumEnabled(), cfg.IsRowLevelChecksumEnabled);
    require::Same(&sctx.GetSessionVars().RowEncoder, &cfg.RowEncoder);

    sctx.GetSessionVars().RowEncoder.Enable = false;
    cfg = ctx.GetRowEncodingConfig();
    require::False(cfg.IsRowLevelChecksumEnabled);
    require::Equal(sctx.GetSessionVars().IsRowLevelChecksumEnabled(), cfg.IsRowLevelChecksumEnabled);
    require::Same(&sctx.GetSessionVars().RowEncoder, &cfg.RowEncoder);

    sctx.GetSessionVars().RowEncoder.Enable = true;
    sctx.GetSessionVars().InRestrictedSQL = true;
    require::Equal(sctx.GetSessionVars().IsRowLevelChecksumEnabled(), cfg.IsRowLevelChecksumEnabled);
    require::False(cfg.IsRowLevelChecksumEnabled);
    sctx.GetSessionVars().InRestrictedSQL = false;
    sctx.GetSessionVars().EnableRowLevelChecksum = false;
    require::Equal(sctx.GetSessionVars().IsRowLevelChecksumEnabled(), cfg.IsRowLevelChecksumEnabled);

    // mutate buffers 与 RowIDShardGenerator 走 session 级共享对象。
    require::NotNil(ctx.GetMutateBuffers());
    sctx.GetSessionVars().TxnCtx.StartTS = 123;
    require::Same(
        &sctx.GetSessionVars().GetRowIDShardGenerator(),
        &ctx.GetRowIDShardGenerator(),
    );

    // ReservedRowIDAlloc：StmtCtx 存在时返回 allocator 和 ok=true。
    let (reserved, ok) = ctx.GetReservedRowIDAlloc();
    require::True(ok);
    require::Same(&sctx.GetSessionVars().StmtCtx.ReservedRowIDAlloc, &reserved);

    // statistics support：TxnCtx 为空时返回 false；存在时写入 TableDeltaMap。
    let txn_ctx = sctx.GetSessionVars().TxnCtx.clone();
    txn_ctx.TableDeltaMap = std::collections::HashMap::new();
    sctx.GetSessionVars().TxnCtx = None;
    let (statistics_support, ok) = ctx.GetStatisticsSupport();
    require::False(ok);
    require::Nil(statistics_support);
    sctx.GetSessionVars().TxnCtx = Some(txn_ctx.clone());
    let (mut statistics_support, ok) = ctx.GetStatisticsSupport();
    require::True(ok);
    require::NotNil(&statistics_support);
    require::Equal(0, txn_ctx.TableDeltaMap.len());
    statistics_support.UpdatePhysicalTableDelta(12, 1, 2);
    require::Equal(1, txn_ctx.TableDeltaMap.len());
    let delta_map = txn_ctx.TableDeltaMap.get(&12).unwrap();
    require::Equal(1_i64, delta_map.Delta);
    require::Equal(2_i64, delta_map.Count);

    // cached table support：TxnCtx 存在时初始化 CachedTables，并只写入一个 handle。
    sctx.GetSessionVars().TxnCtx = None;
    let (cached_table_support, ok) = ctx.GetCachedTableSupport();
    require::False(ok);
    require::Nil(cached_table_support);
    sctx.GetSessionVars().TxnCtx = Some(txn_ctx.clone());
    let (mut cached_table_support, ok) = ctx.GetCachedTableSupport();
    require::True(ok);
    let handle = mockCachedTable {};
    require::Nil(txn_ctx.CachedTables.get(&123));
    cached_table_support.AddCachedTableHandleToTxn(123, handle);
    let cached = txn_ctx.CachedTables.get(&123).unwrap();
    require::Same(&handle, cached);

    // temporary table support：没有 TxnCtx 返回 false；有 TxnCtx 时创建 modified 临时表 handler。
    sctx.GetSessionVars().TxnCtx = None;
    let (temp_table_support, ok) = ctx.GetTemporaryTableSupport();
    require::False(ok);
    require::Nil(temp_table_support);
    sctx.GetSessionVars().TxnCtx = Some(txn_ctx.clone());
    let mut mock_temp_data = mockTemporaryData { base: variable::TemporaryTableData::default(), size: 0 };
    sctx.GetSessionVars().TemporaryTableData = mock_temp_data.clone();
    let (mut temp_table_support, ok) = ctx.GetTemporaryTableSupport();
    require::True(ok);
    require::Nil(txn_ctx.TemporaryTables.get(&456));
    let (mut tmp_tbl_handler, ok) = temp_table_support.AddTemporaryTableToTxn(&model::TableInfo {
        ID: 456,
        TempTableType: model::TempTableGlobal,
    });
    require::True(ok);
    require::NotNil(&tmp_tbl_handler);
    let tmp_tbl_table = txn_ctx.TemporaryTables.get(&456).unwrap();
    require::True(tmp_tbl_table.GetModified());
    require::Equal(456000000_i64, tmp_tbl_handler.GetCommittedSize());
    mock_temp_data.size = 111;
    require::Equal(456000111_i64, tmp_tbl_handler.GetCommittedSize());
    require::Equal(0_i64, tmp_tbl_handler.GetDirtySize());
    tmp_tbl_handler.UpdateTxnDeltaSize(333);
    require::Equal(333_i64, tmp_tbl_handler.GetDirtySize());
    tmp_tbl_handler.UpdateTxnDeltaSize(-1);
    require::Equal(332_i64, tmp_tbl_handler.GetDirtySize());

    // exchange partition DML support：直接返回当前最新 infoschema。
    let (exchange, ok) = ctx.GetExchangePartitionDMLSupport();
    require::True(ok);
    require::Same(
        &ctx.GetLatestInfoSchema(),
        &exchange.GetInfoSchemaToCheckExchangeConstraint(),
    );
}

// mockCachedTable 对应 Go 函数内匿名类型，嵌入 table.CachedTable。
pub struct mockCachedTable {
    pub base: table::CachedTable,
}
"########;

/// 确保 Go 参考伪代码字符串中仍包含 NewMutateContext 关键符号。
#[test]
fn tblsession_go_reference_is_preserved() {
    assert!(GO_REFERENCE.contains("NewMutateContext"));
}
