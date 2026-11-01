// Copyright 2026 AsterSQL.

// IndexReader 的 requiredRows（父算子每次请求行数）透传测试。
//
// 用脚本化 [`SelectResult`] 记录每次 `next(capacity)` 的 capacity，
// 断言 IndexReader 把父侧 `Next(n)` 原样下推到 DistSQL 结果流。

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::distsql::{
    ByItem, Datum, DistSqlBackend, DistSqlError, Handle, IndexReaderExecutor, KeyRange, Request,
    Row, SelectResult, newIndexReaderExecutorContext,
};
use crate::table_reader::{
    KeyRange as TableKeyRange, RequestKeySource, StoreType, TableReaderBackend,
    TableReaderExecutor, TableReaderRequestOptions, newTableReaderExecutorContext,
    selectResultHook,
};

/// 可脚本化的 SelectResult：按请求容量切分行，并记录每次 capacity。
pub(crate) struct ScriptedSelectResult {
    rows: VecDeque<Row>,
    requests: Arc<Mutex<Vec<usize>>>,
    delay: Duration,
}

impl SelectResult for ScriptedSelectResult {
    fn next(&mut self, capacity: usize) -> Result<Vec<Row>, DistSqlError> {
        // 记录父算子下推的 requiredRows。
        self.requests.lock().unwrap().push(capacity);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let count = capacity.min(self.rows.len());
        Ok(self.rows.drain(..count).collect())
    }

    fn close(&mut self) -> Result<(), DistSqlError> {
        self.rows.clear();
        Ok(())
    }

    fn in_flight_cost(&self) -> usize {
        self.rows.len()
    }
}

#[derive(Clone)]
/// Mock DistSQL 后端：持有预设行，select 时包装为 ScriptedSelectResult。
pub(crate) struct RequiredRowsBackend {
    rows: Arc<Mutex<Vec<Row>>>,
    requests: Arc<Mutex<Vec<usize>>>,
    delay: Duration,
}

impl RequiredRowsBackend {
    pub(crate) fn new(rows: Vec<Row>, delay: Duration) -> Self {
        Self {
            rows: Arc::new(Mutex::new(rows)),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay,
        }
    }

    /// 返回观测到的历次 next(capacity)。
    pub(crate) fn requests(&self) -> Vec<usize> {
        self.requests.lock().unwrap().clone()
    }
}

impl DistSqlBackend for RequiredRowsBackend {
    fn select(&self, _request: Request) -> Result<Box<dyn SelectResult>, DistSqlError> {
        Ok(Box::new(ScriptedSelectResult {
            rows: self.rows.lock().unwrap().clone().into(),
            requests: Arc::clone(&self.requests),
            delay: self.delay,
        }))
    }

    fn rebuild_index_ranges(
        &self,
        _access_conditions: &[String],
        _index_columns: &[i64],
        _column_lengths: &[i32],
    ) -> Result<Vec<KeyRange>, DistSqlError> {
        Ok(vec![KeyRange {
            start: vec![0],
            end: vec![255],
        }])
    }

    fn index_ranges(
        &self,
        _table_ids: &[i64],
        _index_id: i64,
        ranges: &[KeyRange],
    ) -> Result<Vec<KeyRange>, DistSqlError> {
        Ok(ranges.to_vec())
    }

    fn table_ranges(
        &self,
        _table_id: i64,
        handles: &[Handle],
    ) -> Result<Vec<KeyRange>, DistSqlError> {
        Ok(handles
            .iter()
            .enumerate()
            .map(|(index, _)| KeyRange {
                start: vec![index as u8],
                end: vec![index as u8 + 1],
            })
            .collect())
    }

    fn index_scan(
        &self,
        _ranges: &[crate::distsql::kvRangesWithPhysicalTblID],
        _request: &Request,
    ) -> Result<Vec<Row>, DistSqlError> {
        Ok(self.rows.lock().unwrap().clone())
    }

    fn table_scan(
        &self,
        _table_id: i64,
        _handles: &[Handle],
        _request: &Request,
    ) -> Result<Vec<Row>, DistSqlError> {
        Ok(self.rows.lock().unwrap().clone())
    }

    fn decode_handle(
        &self,
        row: &[Datum],
        offsets: &[usize],
        _common: bool,
        _partition: bool,
    ) -> Result<Handle, DistSqlError> {
        self.row_handle(row, offsets, false)
    }

    fn row_handle(
        &self,
        row: &[Datum],
        offsets: &[usize],
        _common: bool,
    ) -> Result<Handle, DistSqlError> {
        match offsets.first().and_then(|offset| row.get(*offset)) {
            Some(Datum::Signed(value)) => Ok(Handle::Int(*value)),
            _ => Err(DistSqlError::Decode("missing signed handle".to_owned())),
        }
    }

    fn compare_rows(
        &self,
        left: &[Datum],
        right: &[Datum],
        _by: &[ByItem],
    ) -> Result<Ordering, DistSqlError> {
        Ok(format!("{left:?}").cmp(&format!("{right:?}")))
    }

    fn checksum(&self, row: &[Datum]) -> Result<u64, DistSqlError> {
        Ok(row.len() as u64)
    }

    fn report_inconsistency(
        &self,
        _expected: &[Handle],
        _obtained: &[Handle],
        _missing: &[Handle],
    ) -> Result<(), DistSqlError> {
        Ok(())
    }
}

/// 构造绑定给定后端的 IndexReaderExecutor 测试夹具。
pub(crate) fn required_rows_reader(
    backend: RequiredRowsBackend,
) -> IndexReaderExecutor<RequiredRowsBackend> {
    let backend = Arc::new(backend);
    IndexReaderExecutor {
        scan: None,
        context: newIndexReaderExecutorContext(Arc::clone(&backend), 4, false),
        table_id: 7,
        index_id: 9,
        plans: vec![11],
        ranges: vec![KeyRange {
            start: vec![0],
            end: vec![255],
        }],
        access_conditions: Vec::new(),
        index_columns: vec![1],
        column_lengths: vec![-1],
        table_ids: vec![7],
        by_items: Vec::new(),
        descending: false,
        keep_order: true,
        dummy: false,
        result: None,
        merged_rows: VecDeque::new(),
        runtime_rows: 0,
        range_mem_tracker: None,
    }
}

/// 生成 `count` 行单列 Signed 数据。
pub(crate) fn signed_rows(count: usize) -> Vec<Row> {
    (0..count)
        .map(|value| vec![Datum::Signed(value as i64)])
        .collect()
}

/// Go `TestIndexReaderRequiredRows` 的三组非均匀请求矩阵。
#[test]
fn index_reader_required_rows_match_go_request_matrix() {
    const MAX_CHUNK_SIZE: usize = 1024;
    let test_cases = [
        (10, vec![1, 5, 3, 10], vec![1, 5, 3, 1]),
        (
            MAX_CHUNK_SIZE + 1,
            vec![1, 5, 3, 10, MAX_CHUNK_SIZE],
            vec![1, 5, 3, 10, MAX_CHUNK_SIZE + 1 - 1 - 5 - 3 - 10],
        ),
        (
            3 * MAX_CHUNK_SIZE + 1,
            vec![3, 10, MAX_CHUNK_SIZE],
            vec![3, 10, MAX_CHUNK_SIZE],
        ),
    ];

    for (total_rows, required_rows, expected_rows) in test_cases {
        let backend = RequiredRowsBackend::new(signed_rows(total_rows), Duration::ZERO);
        let observed = backend.clone();
        let mut reader = required_rows_reader(backend);
        reader.Open().unwrap();

        for (&required, &expected) in required_rows.iter().zip(&expected_rows) {
            assert_eq!(reader.Next(required).unwrap().len(), expected);
        }
        assert_eq!(observed.requests(), required_rows);
        assert_eq!(
            reader.runtime_rows,
            expected_rows.iter().map(|rows| *rows as u64).sum::<u64>()
        );
        reader.Close().unwrap();
    }
}

#[derive(Clone, Debug)]
struct Port96Request {
    key_ranges: Vec<TableKeyRange>,
}

#[derive(Clone, Default)]
struct Port96Backend {
    request_count: Arc<AtomicUsize>,
    fail_request: Option<usize>,
}

impl Port96Backend {
    fn key_range(value: u64) -> TableKeyRange {
        TableKeyRange {
            start_key: value.to_be_bytes().to_vec(),
            end_key: value.saturating_add(1).to_be_bytes().to_vec(),
        }
    }
}

impl TableReaderBackend for Port96Backend {
    type Context = ();
    type Error = String;
    type Chunk = ();
    type DistSQLContext = ();
    type RangerContext = ();
    type BuildPBContext = ();
    type ExprBuildContext = ();
    type InfoSchema = ();
    type ServerInfo = ();
    type MemoryTracker = ();
    type Table = ();
    type Range = u64;
    type DagRequest = ();
    type Plan = ();
    type ColumnInfo = ();
    type FieldType = ();
    type Schema = ();
    type ByItem = ();
    type Request = Port96Request;
    type SelectResult = ();
    type TraceGuard = ();

    fn error(&self, message: String) -> Self::Error {
        message
    }
    fn trace_error(&self, error: Self::Error) -> Self::Error {
        error
    }
    fn dist_sql_context(&self) -> Self::DistSQLContext {}
    fn ranger_context(&self) -> Self::RangerContext {}
    fn build_pb_context(&self) -> Self::BuildPBContext {}
    fn expression_build_context(&self) -> Self::ExprBuildContext {}
    fn statement_memory_tracker(&self) -> Self::MemoryTracker {}
    fn info_schema(&self) -> Self::InfoSchema {}
    fn ddl_owner_available(&self) -> bool {
        false
    }
    fn ddl_owner(&self, _: &Self::Context) -> Result<Self::ServerInfo, Self::Error> {
        Ok(())
    }
    fn server_id(&self, _: &Self::ServerInfo) -> String {
        String::new()
    }
    fn executor_id(&self) -> i32 {
        1
    }
    fn new_memory_tracker(&self, _: i32) -> Self::MemoryTracker {}
    fn reset_memory_tracker(&self, _: &Self::MemoryTracker) {}
    fn attach_memory_tracker(&self, _: &Self::MemoryTracker, _: &Self::MemoryTracker) {}
    fn start_trace_region(&self, _: &Self::Context, _: &str) -> (Self::TraceGuard, Self::Context) {
        ((), ())
    }
    fn open_failpoint_delay(&self) -> Option<Duration> {
        None
    }
    fn runtime_stats_enabled(&self, _: &Self::DistSQLContext) -> bool {
        false
    }
    fn set_collect_execution_summaries(&self, _: &mut Self::DagRequest, _: bool) {}
    fn rebuild_tree_based_dag(
        &self,
        _: &Self::BuildPBContext,
        _: &Self::Plan,
        _: &mut Self::DagRequest,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn rebuild_list_based_dag(
        &self,
        _: &Self::BuildPBContext,
        _: &[Self::Plan],
        _: &mut Self::DagRequest,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn resolve_correlated_ranges(&self, _: &Self::Plan) -> Result<Vec<Self::Range>, Self::Error> {
        Ok(Vec::new())
    }
    fn group_ranges_by_columns(
        &self,
        ranges: &[Self::Range],
        _: &[usize],
    ) -> Result<Vec<Vec<Self::Range>>, Self::Error> {
        Ok(vec![ranges.to_vec()])
    }
    fn split_ranges_across_int64_boundary(
        &self,
        ranges: Vec<Self::Range>,
        _: bool,
        _: bool,
        _: bool,
    ) -> (Vec<Self::Range>, Vec<Self::Range>) {
        ranges
            .into_iter()
            .partition(|value| *value <= i64::MAX as u64)
    }
    fn table_id(&self, _: &Self::Table) -> i64 {
        1
    }
    fn table_name(&self, _: &Self::Table) -> Option<String> {
        None
    }
    fn table_is_common_handle(&self, _: &Self::Table) -> bool {
        false
    }
    fn table_is_cluster_table(&self, _: &Self::Table) -> bool {
        false
    }
    fn cluster_table_uses_ddl_owner(&self, _: &Self::Table) -> bool {
        false
    }
    fn range_memory_usage(&self, _: &Self::Range) -> i64 {
        0
    }
    fn key_range_slice_memory_usage(&self, _: &[TableKeyRange]) -> i64 {
        0
    }
    fn dag_size(&self, _: &Self::DagRequest) -> usize {
        0
    }
    fn update_executor_table_ids(
        &self,
        _: &Self::Context,
        _: &mut Self::DagRequest,
        _: bool,
        _: &[i64],
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn build_request(
        &self,
        _: &Self::DistSQLContext,
        source: RequestKeySource<Self::Range>,
        _: TableReaderRequestOptions<Self::DagRequest, Self::InfoSchema, Self::MemoryTracker>,
    ) -> Result<Self::Request, Self::Error> {
        let request = self.request_count.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        if self.fail_request == Some(request) {
            return Err(format!("request {request} failed"));
        }
        let ranges = match source {
            RequestKeySource::HandleRanges { ranges, .. } => ranges,
            _ => return Err("unexpected request source".to_owned()),
        };
        Ok(Port96Request {
            key_ranges: ranges.into_iter().map(Self::key_range).collect(),
        })
    }
    fn request_key_ranges(&self, request: &Self::Request) -> Vec<TableKeyRange> {
        request.key_ranges.clone()
    }
    fn sort_request_key_ranges(&self, request: &mut Self::Request) -> Vec<TableKeyRange> {
        request
            .key_ranges
            .sort_by(|left, right| left.start_key.cmp(&right.start_key));
        request.key_ranges.clone()
    }
    fn return_field_types(&self) -> Vec<Self::FieldType> {
        Vec::new()
    }
    fn physical_plan_ids(&self, _: &[Self::Plan]) -> Vec<i32> {
        vec![1]
    }
    fn plan_id(&self, _: &Self::Plan) -> i32 {
        1
    }
    fn select_with_runtime_stats(
        &self,
        _: &Self::Context,
        _: &Self::DistSQLContext,
        _: Self::Request,
        _: Vec<Self::FieldType>,
        _: Vec<i32>,
        _: i32,
    ) -> Result<Self::SelectResult, Self::Error> {
        Ok(())
    }
    fn serial_select_results(&self, _: Vec<Self::SelectResult>) -> Self::SelectResult {}
    fn sorted_select_results(
        &self,
        _: &Self::ExprBuildContext,
        _: Vec<Self::SelectResult>,
        _: &Self::Schema,
        _: &[Self::ByItem],
        _: &Self::MemoryTracker,
    ) -> Self::SelectResult {
    }
    fn reset_chunk(&self, _: &mut Self::Chunk) {}
    fn chunk_num_rows(&self, _: &Self::Chunk) -> usize {
        0
    }
    fn select_result_next(
        &self,
        _: &mut Self::SelectResult,
        _: &Self::Context,
        _: &mut Self::Chunk,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn select_result_next_raw(
        &self,
        _: &mut Self::SelectResult,
        _: &Self::Context,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(None)
    }
    fn close_select_result(&self, _: Self::SelectResult) -> Result<(), Self::Error> {
        Ok(())
    }
    fn normalize_context_error(&self, _: &Self::Context, error: Self::Error) -> Self::Error {
        error
    }
    fn log_table_scan(&self, _: &Self::Context, _: Option<&str>, _: &[Self::Range]) {}
    fn fill_virtual_column_values(
        &self,
        _: &[Self::FieldType],
        _: &[usize],
        _: &Self::Schema,
        _: &[Self::ColumnInfo],
        _: &Self::ExprBuildContext,
        _: &mut Self::Chunk,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn schema_column_count(&self, _: &Self::Schema) -> usize {
        0
    }
    fn schema_column_is_virtual(&self, _: &Self::Schema, _: usize) -> bool {
        false
    }
    fn schema_column_id(&self, _: &Self::Schema, _: usize) -> i64 {
        0
    }
    fn schema_column_return_type(&self, _: &Self::Schema, _: usize) -> Self::FieldType {}
    fn column_offset_by_id(&self, _: &[Self::ColumnInfo], _: i64) -> usize {
        0
    }
    fn report_cop_index_usage_for_handle(&self, _: &Self::Table, _: i32) {}
}

fn port_96f2b0da32_reader(
    backend: Port96Backend,
    desc: bool,
) -> TableReaderExecutor<Port96Backend> {
    let backend = Arc::new(backend);
    TableReaderExecutor {
        tableReaderExecutorContext: newTableReaderExecutorContext(Arc::clone(&backend)),
        backend,
        indexUsageReporter: false,
        table: (),
        kvRangeBuilder: None,
        ranges: vec![9, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX],
        groupedRanges: Vec::new(),
        groupByColIdxs: Vec::new(),
        kvRanges: Vec::new(),
        dagPB: (),
        startTS: 1,
        txnScope: String::new(),
        readReplicaScope: String::new(),
        isStaleness: false,
        netDataSize: 0.0,
        columns: Vec::new(),
        resultHandler: None,
        plans: vec![()],
        tablePlan: (),
        schema: (),
        memTracker: None,
        selectResultHook: selectResultHook {
            selectResultFunc: None,
        },
        keepOrder: true,
        desc,
        byItems: Vec::new(),
        paging: false,
        storeType: StoreType::TiKV,
        corColInFilter: false,
        corColInAccess: false,
        virtualColumnIndex: Vec::new(),
        virtualColumnRetFieldTypes: Vec::new(),
        batchCop: false,
        dummy: false,
    }
}

fn port_96f2b0da32_values(ranges: &[TableKeyRange]) -> Vec<u64> {
    ranges
        .iter()
        .map(|range| u64::from_be_bytes(range.start_key.clone().try_into().unwrap()))
        .collect()
}

#[test]
fn port_96f2b0da32_preserves_both_unsigned_clustered_primary_key_range_parts() {
    for (desc, expected) in [
        (
            false,
            vec![9, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX],
        ),
        (
            true,
            vec![i64::MAX as u64 + 1, u64::MAX, 9, i64::MAX as u64],
        ),
    ] {
        let mut reader = port_96f2b0da32_reader(Port96Backend::default(), desc);
        reader.Open(&()).unwrap();
        assert_eq!(port_96f2b0da32_values(&reader.kvRanges), expected);
    }
}

#[test]
fn port_96f2b0da32_propagates_second_range_part_build_error() {
    let backend = Port96Backend {
        fail_request: Some(2),
        ..Port96Backend::default()
    };
    let mut reader = port_96f2b0da32_reader(backend, false);
    assert_eq!(reader.Open(&()).unwrap_err(), "request 2 failed");
}
