// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// runaway syncer 扫描测试，覆盖受限 session stub、watch/watch_done 行构造、checkpoint 推进和点查状态保护。
// testing/testify、mock、JWK/JWT、TiDB session、metrics、infoschema 等外部依赖均按 Go 调用形状保留。

// Syncer 扫描与点查测试：列布局解码、游标不变式。
//
// 前半保留 Go 参考（checkpoint 推进、满批/同键活锁防护等）；
// 后半用 RowExecutor stub 验证 SELECT 形状与点查不改写扫描游标。

const _GO_SYNCER_TEST_REFERENCE: &str = r###"
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

// stubSessionPool 对应 Go 的同名辅助类型，字段和嵌入接口按原测试语义保留。
// Go 类型声明: type stubSessionPool struct {
/*
type stubSessionPool struct {
	resource pools.Resource
}
*/

// Get 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (p *stubSessionPool) Get() (pools.Resource, error) {
pub fn get() {
		return p.resource, nil
}

// Put 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (*stubSessionPool) Put(pools.Resource) {
pub fn put() {
}

// Close 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (*stubSessionPool) Close() {
pub fn close() {
}

// stubRestrictedSession 对应 Go 的同名辅助类型，字段和嵌入接口按原测试语义保留。
// Go 类型声明: type stubRestrictedSession struct {
/*
type stubRestrictedSession struct {
	*mockctx.Context
	rows []chunk.Row
	err  error

	sql  string
	args []any
}
*/

// newStubRestrictedSession 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newStubRestrictedSession(rows []chunk.Row) *stubRestrictedSession {
pub fn new_stub_restricted_session() {
		return &stubRestrictedSession{
			Context: mockctx.NewContext(),
			rows:    rows,
		}
}

// Close 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (*stubRestrictedSession) Close() {
pub fn close() {
}

// GetRestrictedSQLExecutor 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func (s *stubRestrictedSession) GetRestrictedSQLExecutor() sqlexec.RestrictedSQLExecutor {
pub fn get_restricted_sqlexecutor() {
		return s
}

// ParseWithParams 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；错误分支和错误文本按 Go 测试保留。
// Go 签名: func (*stubRestrictedSession) ParseWithParams(context.Context, string, ...any) (ast.StmtNode, error) {
pub fn parse_with_params() {
    // 受限 SQL/session 接口属于外部依赖；保留调用参数捕获和错误返回形状。
		return nil, errors.New("unexpected ParseWithParams call")
}

// ExecRestrictedStmt 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；错误分支和错误文本按 Go 测试保留。
// Go 签名: func (*stubRestrictedSession) ExecRestrictedStmt(context.Context, ast.StmtNode, ...sqlexec.OptionFuncAlias) ([]chunk.Row, []*resolve.ResultField, error) {
pub fn exec_restricted_stmt() {
    // 受限 SQL/session 接口属于外部依赖；保留调用参数捕获和错误返回形状。
		return nil, nil, errors.New("unexpected ExecRestrictedStmt call")
}

// ExecRestrictedSQL 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；错误分支和错误文本按 Go 测试保留。
// Go 签名: func (s *stubRestrictedSession) ExecRestrictedSQL(_ context.Context, _ []sqlexec.OptionFuncAlias, sql string, args ...any) ([]chunk.Row, []*resolve.ResultField, error) {
pub fn exec_restricted_sql() {
		s.sql = sql
		s.args = append([]any(nil), args...)
		return s.rows, nil, s.err
}

// makeWatchRows generates n watch rows with start_time starting from baseTime,
// incremented by step per row. step=0 gives identical timestamps.
// makeWatchRows 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；时间/休眠边界按原测试断言保留。
// Go 签名: func makeWatchRows(n int, baseTime time.Time, step time.Duration) []chunk.Row {
pub fn make_watch_rows() {
		rows := make([]chunk.Row, n)
		for i := range rows {
			st := baseTime.Add(time.Duration(i) * step)
			rows[i] = newWatchRow(int64(i+1), "rg", st, nil)
		}
		return rows
}

// makeWatchDoneRows generates n watch_done rows with done_time starting from
// baseDoneTime, incremented by step per row. start_time is fixed.
// makeWatchDoneRows 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；时间/休眠边界按原测试断言保留。
// Go 签名: func makeWatchDoneRows(n int, baseDoneTime time.Time, step time.Duration) []chunk.Row {
pub fn make_watch_done_rows() {
		fixedStart := baseDoneTime.Add(-time.Hour)
		rows := make([]chunk.Row, n)
		for i := range rows {
			dt := baseDoneTime.Add(time.Duration(i) * step)
			rows[i] = newWatchDoneRow(int64(i+1), int64(i+1), "rg", fixedStart, nil, dt)
		}
		return rows
}

// newInvalidWatchDoneRow 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；时间/休眠边界按原测试断言保留。
// Go 签名: func newInvalidWatchDoneRow(doneID int64) chunk.Row {
pub fn new_invalid_watch_done_row() {
		return buildRow(
			types.NewIntDatum(doneID),
			types.NewIntDatum(doneID),
			types.NewStringDatum("rg"),
			types.NewTimeDatum(types.ZeroDatetime),
			types.NewDatum(nil),
			types.NewIntDatum(1),
			types.NewStringDatum("select 1"),
			types.NewStringDatum("manual"),
			types.NewIntDatum(2),
			types.NewStringDatum("rg_dst"),
			types.NewStringDatum("watch-rule"),
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
			timeDatum(time.Date(2026, 4, 14, 12, 0, 0, 0, time.UTC)),
		)
}

// scanTestCase parameterizes watch vs watch_done so each test function
// covers one code path without duplicating the two-table variant.
// scanTestCase 对应 Go 的同名辅助类型，字段和嵌入接口按原测试语义保留。
// Go 类型声明: type scanTestCase struct {
/*
type scanTestCase struct {
	name        string
	makeReader  func(time.Time) *systemTableReader
	makeRows    func(int, time.Time, time.Duration) []chunk.Row
	makeInvalid func(int64) chunk.Row
	setup       func(*syncer, *systemTableReader)
	scan        func(*syncer) ([]*QuarantineRecord, error)
}
*/

// scanCases 把 watch 与 watch_done 两条扫描路径参数化，后续测试复用同一批 checkpoint 断言。
// Go 声明: var scanCases = []scanTestCase{

// newTestWatchReader 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newTestWatchReader(checkpoint time.Time) *systemTableReader {
pub fn new_test_watch_reader() {
		r := newSystemTableReader(
			runawayWatchFullTableName, "start_time", watchColStartTime, watchRecordColumns,
		)
		r.CheckPoint = checkpoint
		return r
}

// newTestWatchDoneReader 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newTestWatchDoneReader(checkpoint time.Time) *systemTableReader {
pub fn new_test_watch_done_reader() {
		r := newSystemTableReader(
			runawayWatchDoneFullTableName, "done_time", watchDoneColDoneTime, watchDoneRecordColumns,
		)
		r.CheckPoint = checkpoint
		return r
}

// timeDatum 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func timeDatum(t time.Time) types.Datum {
pub fn time_datum() {
		return types.NewTimeDatum(types.NewTime(types.FromGoTime(t.UTC()), mysql.TypeDatetime, 6))
}

// buildRow 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func buildRow(datums ...types.Datum) chunk.Row {
pub fn build_row() {
		return chunk.MutRowFromDatums(datums).ToRow()
}

// newWatchRow 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newWatchRow(id int64, groupName string, startTime time.Time, endTime *time.Time) chunk.Row {
pub fn new_watch_row() {
		endDatum := types.NewDatum(nil)
		if endTime != nil {
			endDatum = timeDatum(*endTime)
		}
		return buildRow(
			types.NewIntDatum(id),
			types.NewStringDatum(groupName),
			timeDatum(startTime),
			endDatum,
			types.NewIntDatum(1),
			types.NewStringDatum("select 1"),
			types.NewStringDatum("manual"),
			types.NewIntDatum(2),
			types.NewStringDatum("rg_dst"),
			types.NewStringDatum("watch-rule"),
		)
}

// newInvalidWatchRow 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newInvalidWatchRow(id int64, groupName string) chunk.Row {
pub fn new_invalid_watch_row() {
		return buildRow(
			types.NewIntDatum(id),
			types.NewStringDatum(groupName),
			types.NewTimeDatum(types.ZeroDatetime),
			types.NewDatum(nil),
			types.NewIntDatum(1),
			types.NewStringDatum("select 1"),
			types.NewStringDatum("manual"),
			types.NewIntDatum(2),
			types.NewStringDatum("rg_dst"),
			types.NewStringDatum("watch-rule"),
		)
}

// newWatchDoneRow 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状。
// Go 签名: func newWatchDoneRow(doneID, recordID int64, groupName string, startTime time.Time, endTime *time.Time, doneTime time.Time) chunk.Row {
pub fn new_watch_done_row() {
		endDatum := types.NewDatum(nil)
		if endTime != nil {
			endDatum = timeDatum(*endTime)
		}
		return buildRow(
			types.NewIntDatum(doneID),
			types.NewIntDatum(recordID),
			types.NewStringDatum(groupName),
			timeDatum(startTime),
			endDatum,
			types.NewIntDatum(1),
			types.NewStringDatum("select 1"),
			types.NewStringDatum("manual"),
			types.NewIntDatum(2),
			types.NewStringDatum("rg_dst"),
			types.NewStringDatum("watch-rule"),
			timeDatum(doneTime),
		)
}

// TestGenSelectStmts 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本。
// Go 签名: func TestGenSelectStmts(t *testing.T) {
#[test]
pub fn test_gen_select_stmts() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		checkpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)
		upperBound := checkpoint.Add(10 * time.Second)
		reader := newTestWatchReader(checkpoint)
		reader.UpperBound = upperBound

		sql, params := reader.genSelectStmt()
		require.Equal(t, "select * from mysql.tidb_runaway_watch where start_time >= %? and start_time < %? order by start_time limit %?", sql)
		require.Equal(t, []any{checkpoint, upperBound, watchSyncBatchLimit}, params)

		sqlGenFn := reader.genSelectByIDStmt(42)
		sql, params = sqlGenFn()
		require.Equal(t, "select * from mysql.tidb_runaway_watch where id = %?", sql)
		require.Equal(t, []any{int64(42)}, params)

		sqlGenFn = reader.genSelectByGroupStmt("rg_bulk")
		sql, params = sqlGenFn()
		require.Equal(t, "select * from mysql.tidb_runaway_watch where resource_group_name = %?", sql)
		require.Equal(t, []any{"rg_bulk"}, params)
}

// TestWatchAdvanceCheckpoint 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestWatchAdvanceCheckpoint(t *testing.T) {
#[test]
pub fn test_watch_advance_checkpoint() {
		// issue:67754
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		startTime := time.Date(2026, 4, 14, 10, 0, 0, 123000000, time.UTC)
		endTime := startTime.Add(5 * time.Minute)
		initialCheckpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)
		session := newStubRestrictedSession([]chunk.Row{newWatchRow(30005, "rg_bulk", startTime, &endTime)})
		reader := newTestWatchReader(initialCheckpoint)
		s := &syncer{
			sysSessionPool: &stubSessionPool{resource: session},
			newWatchReader: reader,
		}

		before := time.Now().UTC()
		records, err := s.getNewWatchRecords()
		after := time.Now().UTC()

		require.NoError(t, err)
		require.Len(t, records, 1)
		require.Equal(t, int64(30005), records[0].ID)
		require.Equal(t, "rg_bulk", records[0].ResourceGroupName)
		require.Equal(t, endTime, records[0].EndTime)
		require.Contains(t, session.sql, "where start_time >= %? and start_time < %? order by start_time limit %?")
		require.Len(t, session.args, 3)

		checkpointArg, ok := session.args[0].(time.Time)
		require.True(t, ok)
		require.True(t, checkpointArg.Equal(initialCheckpoint))
		upperBoundArg, ok := session.args[1].(time.Time)
		require.True(t, ok)
		require.False(t, upperBoundArg.Before(before))
		require.False(t, upperBoundArg.After(after))
		require.Equal(t, watchSyncBatchLimit, session.args[2])
		// Partial batch (1 < 2048): checkpoint advances to UpperBound - overlap
		require.True(t, reader.CheckPoint.Equal(upperBoundArg.Add(-watchSyncOverlap)))
}

// TestWatchHoldCheckpoint 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestWatchHoldCheckpoint(t *testing.T) {
#[test]
pub fn test_watch_hold_checkpoint() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		initialCheckpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)

		t.Run("empty result", func(t *testing.T) {
			session := newStubRestrictedSession(nil)
			reader := newTestWatchReader(initialCheckpoint)
			s := &syncer{
				sysSessionPool: &stubSessionPool{resource: session},
				newWatchReader: reader,
			}

			records, err := s.getNewWatchRecords()
			require.NoError(t, err)
			require.Empty(t, records)
			require.True(t, reader.CheckPoint.Equal(initialCheckpoint))
		})

		t.Run("all rows dropped during decode", func(t *testing.T) {
			session := newStubRestrictedSession([]chunk.Row{newInvalidWatchRow(30006, "rg_bulk")})
			reader := newTestWatchReader(initialCheckpoint)
			s := &syncer{
				sysSessionPool: &stubSessionPool{resource: session},
				newWatchReader: reader,
			}

			records, err := s.getNewWatchRecords()
			require.NoError(t, err)
			require.Empty(t, records)
			require.True(t, reader.CheckPoint.Equal(initialCheckpoint))
		})

		t.Run("sql error", func(t *testing.T) {
			session := newStubRestrictedSession(nil)
			session.err = errors.New("read failed")
			reader := newTestWatchReader(initialCheckpoint)
			s := &syncer{
				sysSessionPool: &stubSessionPool{resource: session},
				newWatchReader: reader,
			}

			records, err := s.getNewWatchRecords()
    // 错误断言保留 Go 的错误文本，方便后续接线时逐项对齐。
			require.ErrorContains(t, err, "read failed")
			require.Nil(t, records)
			require.True(t, reader.CheckPoint.Equal(initialCheckpoint))
		})
}

// TestWatchDoneAdvanceCheckpoint 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestWatchDoneAdvanceCheckpoint(t *testing.T) {
#[test]
pub fn test_watch_done_advance_checkpoint() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		startTime := time.Date(2026, 4, 14, 10, 0, 0, 123000000, time.UTC)
		endTime := startTime.Add(5 * time.Minute)
		doneTime := time.Date(2026, 4, 14, 10, 1, 0, 123000000, time.UTC)
		initialCheckpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)
		session := newStubRestrictedSession([]chunk.Row{newWatchDoneRow(1, 30005, "rg_bulk", startTime, &endTime, doneTime)})
		reader := newTestWatchDoneReader(initialCheckpoint)
		s := &syncer{
			sysSessionPool:      &stubSessionPool{resource: session},
			deletionWatchReader: reader,
		}

		before := time.Now().UTC()
		records, err := s.getNewWatchDoneRecords()
		after := time.Now().UTC()

		require.NoError(t, err)
		require.Len(t, records, 1)
		require.Equal(t, int64(30005), records[0].ID)
		require.Equal(t, endTime, records[0].EndTime)
		require.Contains(t, session.sql, "where done_time >= %? and done_time < %? order by done_time limit %?")
		require.Len(t, session.args, 3)

		checkpointArg, ok := session.args[0].(time.Time)
		require.True(t, ok)
		require.True(t, checkpointArg.Equal(initialCheckpoint))
		upperBoundArg, ok := session.args[1].(time.Time)
		require.True(t, ok)
		require.False(t, upperBoundArg.Before(before))
		require.False(t, upperBoundArg.After(after))
		require.Equal(t, watchSyncBatchLimit, session.args[2])
		require.True(t, reader.CheckPoint.Equal(upperBoundArg.Add(-watchSyncOverlap)))
}

// TestWatchDoneHoldCheckpoint 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestWatchDoneHoldCheckpoint(t *testing.T) {
#[test]
pub fn test_watch_done_hold_checkpoint() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		initialCheckpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)

		t.Run("empty result", func(t *testing.T) {
			session := newStubRestrictedSession(nil)
			reader := newTestWatchDoneReader(initialCheckpoint)
			s := &syncer{
				sysSessionPool:      &stubSessionPool{resource: session},
				deletionWatchReader: reader,
			}

			records, err := s.getNewWatchDoneRecords()
			require.NoError(t, err)
			require.Empty(t, records)
			require.True(t, reader.CheckPoint.Equal(initialCheckpoint))
		})

		t.Run("sql error", func(t *testing.T) {
			session := newStubRestrictedSession(nil)
			session.err = errors.New("read failed")
			reader := newTestWatchDoneReader(initialCheckpoint)
			s := &syncer{
				sysSessionPool:      &stubSessionPool{resource: session},
				deletionWatchReader: reader,
			}

			records, err := s.getNewWatchDoneRecords()
    // 错误断言保留 Go 的错误文本，方便后续接线时逐项对齐。
			require.ErrorContains(t, err, "read failed")
			require.Nil(t, records)
			require.True(t, reader.CheckPoint.Equal(initialCheckpoint))
		})
}

// TestScanFullBatch 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestScanFullBatch(t *testing.T) {
#[test]
pub fn test_scan_full_batch() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		baseTime := time.Date(2026, 4, 14, 10, 0, 0, 0, time.UTC)
		initialCP := baseTime.Add(-time.Minute)

		for _, tc := range scanCases {
			t.Run(tc.name, func(t *testing.T) {
				rows := tc.makeRows(watchSyncBatchLimit, baseTime, time.Microsecond)
				session := newStubRestrictedSession(rows)
				reader := tc.makeReader(initialCP)
				s := &syncer{sysSessionPool: &stubSessionPool{resource: session}}
				tc.setup(s, reader)

				before := time.Now().UTC()
				records, err := tc.scan(s)
				after := time.Now().UTC()

				require.NoError(t, err)
				require.Len(t, records, watchSyncBatchLimit)

				lastKeyTime := baseTime.Add(time.Duration(watchSyncBatchLimit-1) * time.Microsecond)
				require.True(t, reader.CheckPoint.Equal(lastKeyTime),
					"full batch: checkpoint must be last row's key time, got %v want %v", reader.CheckPoint, lastKeyTime)
				require.True(t, reader.lastScanKeyTime.Equal(lastKeyTime))
				// Must differ from the partial-batch path.
				require.False(t, reader.CheckPoint.Equal(reader.UpperBound.Add(-watchSyncOverlap)))
				require.False(t, reader.UpperBound.Before(before))
				require.False(t, reader.UpperBound.After(after))
			})
		}
}

// TestScanFullBatchSameKey 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestScanFullBatchSameKey(t *testing.T) {
#[test]
pub fn test_scan_full_batch_same_key() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		baseTime := time.Date(2026, 4, 14, 10, 0, 0, 0, time.UTC)

		for _, tc := range scanCases {
			t.Run(tc.name, func(t *testing.T) {
				rows := tc.makeRows(watchSyncBatchLimit, baseTime, 0)
				session := newStubRestrictedSession(rows)
				reader := tc.makeReader(baseTime) // CheckPoint == all rows' key time
				s := &syncer{sysSessionPool: &stubSessionPool{resource: session}}
				tc.setup(s, reader)

				records, err := tc.scan(s)

				require.NoError(t, err)
				require.Len(t, records, watchSyncBatchLimit)
				require.True(t, reader.lastScanKeyTime.Equal(baseTime))
				// Livelock guard: must fall back to UpperBound - overlap.
				require.True(t, reader.CheckPoint.Equal(reader.UpperBound.Add(-watchSyncOverlap)),
					"same-key full batch: checkpoint must fall back to UpperBound-overlap")
			})
		}
}

// TestScanPagination 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestScanPagination(t *testing.T) {
#[test]
pub fn test_scan_pagination() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		baseTime := time.Date(2026, 4, 14, 10, 0, 0, 0, time.UTC)
		initialCP := baseTime.Add(-time.Minute)
		page2Count := 500

		for _, tc := range scanCases {
			t.Run(tc.name, func(t *testing.T) {
				page1 := tc.makeRows(watchSyncBatchLimit, baseTime, time.Microsecond)
				page2Start := baseTime.Add(time.Duration(watchSyncBatchLimit) * time.Microsecond)
				page2 := tc.makeRows(page2Count, page2Start, time.Microsecond)

				session := newStubRestrictedSession(page1)
				reader := tc.makeReader(initialCP)
				s := &syncer{sysSessionPool: &stubSessionPool{resource: session}}
				tc.setup(s, reader)

				// Page 1: full batch.
				records1, err := tc.scan(s)
				require.NoError(t, err)
				require.Len(t, records1, watchSyncBatchLimit)
				cp1 := reader.CheckPoint
				lastPage1Key := baseTime.Add(time.Duration(watchSyncBatchLimit-1) * time.Microsecond)
				require.True(t, cp1.Equal(lastPage1Key))

				// Page 2: partial batch.
				session.rows = page2
				records2, err := tc.scan(s)
				require.NoError(t, err)
				require.Len(t, records2, page2Count)
				cp2 := reader.CheckPoint

				require.True(t, cp2.After(cp1), "checkpoint must advance monotonically")
				require.True(t, cp2.Equal(reader.UpperBound.Add(-watchSyncOverlap)))
			})
		}
}

// TestScanInvalidTail 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestScanInvalidTail(t *testing.T) {
#[test]
pub fn test_scan_invalid_tail() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		baseTime := time.Date(2026, 4, 14, 10, 0, 0, 0, time.UTC)
		initialCP := baseTime.Add(-time.Minute)
		validCount := watchSyncBatchLimit - 2

		for _, tc := range scanCases {
			t.Run(tc.name, func(t *testing.T) {
				valid := tc.makeRows(validCount, baseTime, time.Microsecond)
				rows := append(valid,
					tc.makeInvalid(int64(validCount+1)),
					tc.makeInvalid(int64(validCount+2)),
				)
				require.Len(t, rows, watchSyncBatchLimit) // raw rows hit LIMIT

				session := newStubRestrictedSession(rows)
				reader := tc.makeReader(initialCP)
				s := &syncer{sysSessionPool: &stubSessionPool{resource: session}}
				tc.setup(s, reader)

				records, err := tc.scan(s)

				require.NoError(t, err)
				require.Len(t, records, validCount) // invalid rows dropped
				// len(records) < watchSyncBatchLimit → partial-batch path.
				require.True(t, reader.CheckPoint.Equal(reader.UpperBound.Add(-watchSyncOverlap)))
				// lastScanKeyTime reflects the last *valid* row.
				lastValidKey := baseTime.Add(time.Duration(validCount-1) * time.Microsecond)
				require.True(t, reader.lastScanKeyTime.Equal(lastValidKey))
			})
		}
}

// TestPointQueryPreservesScanState guards the invariant that
// getWatchRecordByID / getWatchRecordByGroup leave scan-cursor state
// (lastScanKeyTime, CheckPoint, UpperBound) untouched, so manual
// RemoveRunawayWatch* calls between sync ticks can't perturb the cursor.
// TestPointQueryPreservesScanState 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；时间/休眠边界按原测试断言保留；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestPointQueryPreservesScanState(t *testing.T) {
#[test]
pub fn test_point_query_preserves_scan_state() {
    // 时间相关断言依赖 Go time.Time；这里保留边界和比较语义。
		startTime := time.Date(2026, 4, 14, 10, 0, 0, 123000000, time.UTC)
		endTime := startTime.Add(5 * time.Minute)
		initialCheckpoint := time.Date(2026, 4, 14, 9, 59, 0, 0, time.UTC)
		initialUpperBound := time.Date(2026, 4, 14, 9, 59, 30, 0, time.UTC)
		initialLastScanKey := time.Date(2026, 4, 14, 9, 58, 45, 0, time.UTC)

		newReader := func() *systemTableReader {
			r := newTestWatchReader(initialCheckpoint)
			r.UpperBound = initialUpperBound
			r.lastScanKeyTime = initialLastScanKey
			return r
		}

		cases := []struct {
			name string
			call func(*syncer) ([]*QuarantineRecord, error)
		}{
			{"by_id", func(s *syncer) ([]*QuarantineRecord, error) { return s.getWatchRecordByID(30005) }},
			{"by_group", func(s *syncer) ([]*QuarantineRecord, error) { return s.getWatchRecordByGroup("rg_bulk") }},
		}
		for _, tc := range cases {
			t.Run(tc.name, func(t *testing.T) {
				session := newStubRestrictedSession([]chunk.Row{newWatchRow(30005, "rg_bulk", startTime, &endTime)})
				reader := newReader()
				s := &syncer{sysSessionPool: &stubSessionPool{resource: session}, newWatchReader: reader}

				records, err := tc.call(s)
				require.NoError(t, err)
				require.Len(t, records, 1)

				require.True(t, reader.CheckPoint.Equal(initialCheckpoint), "point query must not advance CheckPoint")
				require.True(t, reader.UpperBound.Equal(initialUpperBound), "point query must not rewrite UpperBound")
				require.True(t, reader.lastScanKeyTime.Equal(initialLastScanKey), "point query must not touch lastScanKeyTime")
			})
		}
}
"###;

use std::sync::{Arc, Mutex};

use crate::record::SqlValue;
use crate::syncer::{
    AllSystemTables, SqlRow, Syncer, WATCH_DONE_RECORD_COLUMNS, WATCH_RECORD_COLUMNS,
    WATCH_SYNC_BATCH_LIMIT, WATCH_SYNC_OVERLAP_MICROS, decodeQuarantineRecord,
};
use crate::{Error, ExecutorRef, RestrictedSqlExecutor, Result, RunawayAction, RunawayWatchType};

/// 返回预设行并记录 SQL，用于 syncer 扫描/点查断言。
#[derive(Default)]
struct RowExecutor {
    rows: Mutex<Vec<SqlRow>>,
    error: Mutex<Option<Error>>,
    statements: Mutex<Vec<(String, Vec<SqlValue>)>>,
}

impl RestrictedSqlExecutor for RowExecutor {
    fn Execute(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<SqlRow>> {
        self.statements
            .lock()
            .unwrap()
            .push((sql.to_owned(), params.to_vec()));
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self.rows.lock().unwrap().clone())
    }
}

/// 构造符合 WATCH_RECORD_COLUMNS 布局的样例行。
fn watch_row(id: i64, start: i64) -> SqlRow {
    SqlRow(vec![
        SqlValue::Int(id),
        SqlValue::Text("rg".into()),
        SqlValue::Time(start),
        SqlValue::Null,
        SqlValue::Int(RunawayWatchType::Similar as i64),
        SqlValue::Text("digest".into()),
        SqlValue::Text("server-1".into()),
        SqlValue::Int(RunawayAction::Kill as i64),
        SqlValue::Text(String::new()),
        SqlValue::Text("RequestUnit".into()),
    ])
}

/// 构造符合 WATCH_DONE_RECORD_COLUMNS 布局的样例行。
fn watch_done_row(id: i64, start: i64, done: i64) -> SqlRow {
    SqlRow(vec![
        SqlValue::Int(id + 10_000),
        SqlValue::Int(id),
        SqlValue::Text("rg".into()),
        SqlValue::Time(start),
        SqlValue::Null,
        SqlValue::Int(RunawayWatchType::Exact as i64),
        SqlValue::Text("select 1".into()),
        SqlValue::Text("server-1".into()),
        SqlValue::Int(RunawayAction::CoolDown as i64),
        SqlValue::Text("rg_dst".into()),
        SqlValue::Text("watch-rule".into()),
        SqlValue::Time(done),
    ])
}

fn invalid_watch_row(id: i64) -> SqlRow {
    let mut row = watch_row(id, id);
    row.0[2] = SqlValue::Text("invalid time".into());
    row
}

fn invalid_watch_done_row(id: i64) -> SqlRow {
    let mut row = watch_done_row(id, id, id);
    row.0[3] = SqlValue::Text("invalid time".into());
    row
}

/// 窗口 SELECT 含时间范围与 limit，且列解码与 Go 枚举编号一致。
#[test]
fn select_statements_and_row_decode_match_go_columns() {
    let executor: ExecutorRef = Arc::new(RowExecutor::default());
    let syncer = Syncer::new(executor, Arc::new(AllSystemTables));
    let (sql, params) = syncer.new_watch_reader.genSelectStmt();
    assert!(sql.contains("start_time >= %? and start_time < %?"));
    assert!(sql.ends_with("order by start_time limit %?"));
    assert_eq!(
        params.last(),
        Some(&SqlValue::Int(WATCH_SYNC_BATCH_LIMIT as i64))
    );
    let record = decodeQuarantineRecord(&watch_row(5, 100), WATCH_RECORD_COLUMNS).unwrap();
    assert_eq!(record.ID, 5);
    assert_eq!(record.ResourceGroupName, "rg");
    assert_eq!(record.EndTime, 0);
    assert_eq!(record.Watch, RunawayWatchType::Similar);
    assert_eq!(record.Action, RunawayAction::Kill);
}

/// 点查不得改写 check_point / upper_bound（对齐 Go TestPointQueryPreservesScanState）。
#[test]
fn point_queries_preserve_scan_cursor_state() {
    let executor = Arc::new(RowExecutor::default());
    executor
        .rows
        .lock()
        .unwrap()
        .push(watch_row(30_005, 123_000));
    let reference: ExecutorRef = executor.clone();
    let mut syncer = Syncer::new(reference, Arc::new(AllSystemTables));
    // 预设游标后做点查，断言游标保持不变。
    syncer.new_watch_reader.check_point = 10;
    syncer.new_watch_reader.upper_bound = 20;

    let by_id = syncer.getWatchRecordByID(30_005).unwrap();
    let by_group = syncer.getWatchRecordByGroup("rg").unwrap();
    assert_eq!(by_id.len(), 1);
    assert_eq!(by_group.len(), 1);
    assert_eq!(syncer.new_watch_reader.check_point, 10);
    assert_eq!(syncer.new_watch_reader.upper_bound, 20);

    let statements = executor.statements.lock().unwrap();
    assert!(statements[0].0.ends_with("where id = %?"));
    assert!(statements[1].0.ends_with("where resource_group_name = %?"));
}

/// 对齐 Go TestWatchAdvanceCheckpoint/TestWatchDoneAdvanceCheckpoint。
#[test]
fn partial_scans_advance_both_checkpoints_from_captured_upper_bound() {
    let executor = Arc::new(RowExecutor::default());
    let reference: ExecutorRef = executor.clone();
    let mut syncer = Syncer::new(reference, Arc::new(AllSystemTables));
    syncer.new_watch_reader.check_point = 10;
    syncer.deletion_watch_reader.check_point = 20;

    executor.rows.lock().unwrap().push(watch_row(30_005, 100));
    let watch = syncer.getNewWatchRecords().unwrap();
    assert_eq!(watch.len(), 1);
    assert_eq!(watch[0].ID, 30_005);
    assert_eq!(
        syncer.new_watch_reader.check_point,
        syncer.new_watch_reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS
    );

    *executor.rows.lock().unwrap() = vec![watch_done_row(30_005, 100, 200)];
    let done = syncer.getNewWatchDoneRecords().unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].ID, 30_005);
    assert_eq!(
        syncer.deletion_watch_reader.check_point,
        syncer.deletion_watch_reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS
    );
    assert_eq!(
        decodeQuarantineRecord(&watch_done_row(7, 100, 200), WATCH_DONE_RECORD_COLUMNS)
            .unwrap()
            .ID,
        7
    );
}

/// 对齐 Go TestScanFullBatch/TestScanFullBatchSameKey，覆盖两张系统表。
#[test]
fn full_batches_advance_to_last_key_and_same_key_batches_avoid_livelock() {
    for done_table in [false, true] {
        let executor = Arc::new(RowExecutor::default());
        let reference: ExecutorRef = executor.clone();
        let mut syncer = Syncer::new(reference, Arc::new(AllSystemTables));
        let base = 1_000_000;

        *executor.rows.lock().unwrap() = (0..WATCH_SYNC_BATCH_LIMIT)
            .map(|offset| {
                let key = base + offset as i64;
                if done_table {
                    watch_done_row(offset as i64, base, key)
                } else {
                    watch_row(offset as i64, key)
                }
            })
            .collect();
        let records = if done_table {
            syncer.getNewWatchDoneRecords().unwrap()
        } else {
            syncer.getNewWatchRecords().unwrap()
        };
        assert_eq!(records.len(), WATCH_SYNC_BATCH_LIMIT);
        let reader = if done_table {
            &syncer.deletion_watch_reader
        } else {
            &syncer.new_watch_reader
        };
        assert_eq!(reader.check_point, base + WATCH_SYNC_BATCH_LIMIT as i64 - 1);

        let repeated_key = reader.check_point;
        *executor.rows.lock().unwrap() = (0..WATCH_SYNC_BATCH_LIMIT)
            .map(|offset| {
                if done_table {
                    watch_done_row(offset as i64, base, repeated_key)
                } else {
                    watch_row(offset as i64, repeated_key)
                }
            })
            .collect();
        if done_table {
            syncer.getNewWatchDoneRecords().unwrap();
        } else {
            syncer.getNewWatchRecords().unwrap();
        }
        let reader = if done_table {
            &syncer.deletion_watch_reader
        } else {
            &syncer.new_watch_reader
        };
        assert_eq!(
            reader.check_point,
            reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS
        );
    }
}

/// 对齐 Go 的空结果、解码丢弃与 SQL 错误均保持 checkpoint 不变。
#[test]
fn empty_invalid_and_error_scans_hold_checkpoint() {
    let executor = Arc::new(RowExecutor::default());
    let reference: ExecutorRef = executor.clone();
    let mut syncer = Syncer::new(reference, Arc::new(AllSystemTables));
    syncer.new_watch_reader.check_point = 101;
    syncer.deletion_watch_reader.check_point = 202;

    assert!(syncer.getNewWatchRecords().unwrap().is_empty());
    assert_eq!(syncer.new_watch_reader.check_point, 101);

    *executor.rows.lock().unwrap() = vec![invalid_watch_row(1)];
    assert!(syncer.getNewWatchRecords().unwrap().is_empty());
    assert_eq!(syncer.new_watch_reader.check_point, 101);

    *executor.rows.lock().unwrap() = vec![invalid_watch_done_row(1)];
    assert!(syncer.getNewWatchDoneRecords().unwrap().is_empty());
    assert_eq!(syncer.deletion_watch_reader.check_point, 202);

    *executor.error.lock().unwrap() = Some(Error::Storage("read failed".into()));
    assert_eq!(
        syncer.getNewWatchRecords().unwrap_err(),
        Error::Storage("read failed".into())
    );
    assert_eq!(syncer.new_watch_reader.check_point, 101);
    assert_eq!(syncer.deletion_watch_reader.check_point, 202);
}

/// 对齐 Go TestScanInvalidTail：原始行满 LIMIT 但有效记录不足时走部分批次。
#[test]
fn invalid_tail_uses_partial_batch_checkpoint_rule() {
    let executor = Arc::new(RowExecutor::default());
    let reference: ExecutorRef = executor.clone();
    let mut syncer = Syncer::new(reference, Arc::new(AllSystemTables));
    let valid_count = WATCH_SYNC_BATCH_LIMIT - 2;
    let mut rows: Vec<_> = (0..valid_count)
        .map(|offset| watch_row(offset as i64, 10_000 + offset as i64))
        .collect();
    rows.push(invalid_watch_row(valid_count as i64));
    rows.push(invalid_watch_row(valid_count as i64 + 1));
    *executor.rows.lock().unwrap() = rows;

    let records = syncer.getNewWatchRecords().unwrap();
    assert_eq!(records.len(), valid_count);
    assert_eq!(
        syncer.new_watch_reader.check_point,
        syncer.new_watch_reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS
    );
}
