// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::sort::VecRowSource;
use super::{DataChunk, Row, SortExec, SortKey, SortValue};

fn int_row(value: i64) -> Row {
    Row(vec![SortValue::Int(value); 3])
}

/// Mirrors Go's TestSortInDisk contract: unordered input must spill, remain globally
/// ordered across output chunks, and release both trackers when the executor closes.
#[test]
fn serial_sort_spills_orders_all_rows_and_releases_trackers() {
    let chunks = (0_usize..5)
        .map(|offset| {
            DataChunk::new(
                (offset..1024)
                    .step_by(5)
                    .rev()
                    .map(|value| int_row(value as i64))
                    .collect(),
            )
        })
        .collect();
    let source = VecRowSource::new(chunks);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::asc(0)], 1, 32, 1);

    let mut output = Vec::new();
    loop {
        let chunk = executor.Next(32).unwrap();
        if chunk.is_empty() {
            break;
        }
        assert!(chunk.num_rows() <= 32);
        output.extend(chunk.rows);
    }

    assert_eq!(output, (0..1024).map(int_row).collect::<Vec<_>>());
    assert!(executor.IsSpillTriggered());
    assert!(executor.GetMemTracker().bytes_consumed() > 0);
    assert!(executor.GetDiskTracker().bytes_consumed() > 0);

    executor.Close().unwrap();
    assert_eq!(executor.GetMemTracker().bytes_consumed(), 0);
    assert_eq!(executor.GetDiskTracker().bytes_consumed(), 0);
}

/// Descending comparison reverses non-NULL values while preserving NULLS LAST.
#[test]
fn serial_sort_honors_null_ordering_and_descending_values() {
    let source = VecRowSource::new(vec![DataChunk::new(vec![
        Row(vec![SortValue::Int(2)]),
        Row(vec![SortValue::Null]),
        Row(vec![SortValue::Int(1)]),
    ])]);
    let mut executor = SortExec::new(Box::new(source), vec![SortKey::desc(0)], 1, 8, -1);

    let output = executor.Next(8).unwrap();

    assert_eq!(
        output
            .rows
            .iter()
            .map(|row| row.0[0].clone())
            .collect::<Vec<_>>(),
        vec![SortValue::Int(2), SortValue::Int(1), SortValue::Null]
    );
}
