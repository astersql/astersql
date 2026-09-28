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

use super::sort::VecRowSource;
use super::{DataChunk, Row, SortError, SortExec, SortKey, SortValue};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SortCase {
    ndvs: [usize; 2],
    order_by_idx: Vec<usize>,
}

fn derived_cases() -> Vec<SortCase> {
    let mut cases = vec![SortCase {
        ndvs: [0, 0],
        order_by_idx: vec![0, 1],
    }];
    for ndv in [1, 10_000] {
        for order_by_idx in [vec![0, 1], vec![0], vec![1]] {
            cases.push(SortCase {
                ndvs: [ndv, 0],
                order_by_idx,
            });
        }
    }
    cases
}

fn expected_rows(case: &SortCase) -> Vec<Row> {
    let mut rows = (0..128)
        .rev()
        .map(|row| {
            Row(vec![
                SortValue::Int(if case.ndvs[0] == 0 {
                    row
                } else {
                    row % case.ndvs[0]
                } as i64),
                SortValue::Int(row as i64),
            ])
        })
        .collect::<Vec<_>>();
    let keys = case
        .order_by_idx
        .iter()
        .copied()
        .map(SortKey::asc)
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| super::sort_util::compare_rows(left, right, &keys));
    rows
}

fn assert_case_output(case: &SortCase, actual: &[Row]) {
    let keys = case
        .order_by_idx
        .iter()
        .copied()
        .map(SortKey::asc)
        .collect::<Vec<_>>();
    assert!(
        actual
            .windows(2)
            .all(|rows| { super::sort_util::compare_rows(&rows[0], &rows[1], &keys).is_le() }),
        "rows are not ordered for case {case:?}"
    );

    // Go 的 sort benchmark 不规定相等排序键之间的稳定顺序；用全部列归一化后
    // 比较多重集，确保执行器既不丢行也不重复行。
    let all_columns = [SortKey::asc(0), SortKey::asc(1)];
    let mut expected = expected_rows(case);
    expected.sort_by(|left, right| super::sort_util::compare_rows(left, right, &all_columns));
    let mut normalized_actual = actual.to_vec();
    normalized_actual
        .sort_by(|left, right| super::sort_util::compare_rows(left, right, &all_columns));
    assert_eq!(normalized_actual, expected, "case {case:?}");
}

#[derive(Debug)]
struct SortOutcome {
    rows: Vec<Row>,
    spilled: bool,
    closed: bool,
}

/// 对应 Go benchmarkSortExec 的单次 b.N 迭代：每次重建输入，随后完整执行
/// Open/Next-until-empty/Close 生命周期。Rust 稳定测试框架没有 Go testing.B
/// 的计时 API，因此这里只验证计时区间内的执行语义。
fn run_sort_case(case: &SortCase, mem_limit: i64) -> Result<SortOutcome, SortError> {
    let rows = expected_rows(case);
    let chunks = rows
        .chunks(16)
        .rev()
        .map(|chunk| DataChunk::new(chunk.iter().rev().cloned().collect()))
        .collect();
    let keys = case
        .order_by_idx
        .iter()
        .copied()
        .map(SortKey::asc)
        .collect();
    // Go benchmark 的 BaseExecutor concurrency 为 4。
    let mut executor = SortExec::new(Box::new(VecRowSource::new(chunks)), keys, 4, 32, mem_limit);

    executor.Open()?;
    let mut output = Vec::with_capacity(rows.len());
    loop {
        let chunk = executor.Next(32)?;
        if chunk.is_empty() {
            break;
        }
        output.extend(chunk.rows);
    }
    let spilled = executor.IsSpillTriggered();
    executor.Close()?;
    let closed = executor.GetMemTracker().bytes_consumed() == 0
        && executor.GetDiskTracker().bytes_consumed() == 0;
    Ok(SortOutcome {
        rows: output,
        spilled,
        closed,
    })
}

#[test]
fn benchmark_sort_exec_runs_all_go_derived_cases() {
    let cases = derived_cases();
    assert_eq!(cases.len(), 7);
    for case in cases {
        let outcome = run_sort_case(&case, -1).expect("ordinary benchmark case must execute");
        assert_case_output(&case, &outcome.rows);
        assert!(
            !outcome.spilled,
            "unlimited case unexpectedly spilled: {case:?}"
        );
        assert!(outcome.closed, "executor was not closed: {case:?}");
    }
}

#[test]
fn benchmark_sort_exec_spill_to_disk_runs_all_go_derived_cases() {
    for case in derived_cases() {
        let outcome = run_sort_case(&case, 1).expect("spill benchmark case must execute");
        assert_case_output(&case, &outcome.rows);
        assert!(
            outcome.spilled,
            "memory-limited case did not spill: {case:?}"
        );
        assert!(outcome.closed, "executor was not closed: {case:?}");
    }
}
