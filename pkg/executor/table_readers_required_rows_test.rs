// Copyright 2026 AsterSQL.

// IndexReader 的 requiredRows（父算子每次请求行数）透传测试。
//
// 用脚本化 [`SelectResult`] 记录每次 `next(capacity)` 的 capacity，
// 断言 IndexReader 把父侧 `Next(n)` 原样下推到 DistSQL 结果流。

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::distsql::{
    ByItem, Datum, DistSqlBackend, DistSqlError, Handle, IndexReaderExecutor, KeyRange, Request,
    Row, SelectResult, newIndexReaderExecutorContext,
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
