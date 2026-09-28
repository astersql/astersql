// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Test exports corresponding to Go's `export_test.go`.
//!
//! These helpers deliberately compose the crate's production coprocessor
//! primitives so tests share the scan, extraction, handle, buffer, and error
//! paths used by DDL backfill code.

use crate::index_cop::{
    CopError, Datum, Handle, ScanRow, build_handle, build_table_scan, extract_datum_by_offsets,
    fetch_table_scan_result,
};

fn fetch_chunk_for_test(
    table_rows: &[ScanRow],
    start_key: &[u8],
    end_key: &[u8],
    batch_size: usize,
) -> Result<Vec<ScanRow>, CopError> {
    let (scan_rows, _) = build_table_scan(table_rows, start_key, end_key, None)?;
    let mut first_batch = None;
    fetch_table_scan_result(&scan_rows, batch_size, |batch| {
        if first_batch.is_none() {
            first_batch = Some(batch.to_vec());
        }
        Ok(())
    })?;
    Ok(first_batch.unwrap_or_default())
}

fn convert_row_to_handle_and_index_datum(
    row: &ScanRow,
    handle_offsets: &[usize],
    index_offsets: &[usize],
    common_handle: bool,
    handle_data_buffer: &mut Vec<Datum>,
    index_data_buffer: &mut Vec<Datum>,
) -> Result<(Handle, Vec<Datum>), CopError> {
    let index_data = extract_datum_by_offsets(row, index_offsets, index_data_buffer)?;
    let handle_data = extract_datum_by_offsets(row, handle_offsets, handle_data_buffer)?;
    let handle = build_handle(&handle_data, common_handle)?;
    Ok((handle, index_data))
}

#[test]
fn fetch_chunk_uses_production_key_range_and_batch_semantics() {
    let rows = (0..8)
        .map(|value| ScanRow {
            key: vec![value],
            columns: vec![
                Datum::Int(i64::from(value)),
                Datum::Text(format!("v{value}")),
            ],
        })
        .collect::<Vec<_>>();

    let chunk = fetch_chunk_for_test(&rows, &[2], &[8], 3).unwrap();
    assert_eq!(
        vec![vec![2], vec![3], vec![4]],
        chunk.iter().map(|row| row.key.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn convert_row_uses_production_buffers_and_integer_handle() {
    let row = ScanRow {
        key: vec![7],
        columns: vec![Datum::Int(7), Datum::UInt(9)],
    };
    let mut handle_buffer = vec![Datum::Null];
    let mut index_buffer = vec![Datum::Null];

    let (handle, index) = convert_row_to_handle_and_index_datum(
        &row,
        &[0],
        &[1],
        false,
        &mut handle_buffer,
        &mut index_buffer,
    )
    .unwrap();

    assert_eq!(Handle::Int(7), handle);
    assert_eq!(vec![Datum::UInt(9)], index);
    assert_eq!(vec![Datum::Int(7)], handle_buffer);
    assert_eq!(vec![Datum::UInt(9)], index_buffer);
}

#[test]
fn export_helpers_propagate_production_errors() {
    let rows = vec![ScanRow {
        key: vec![1],
        columns: vec![Datum::Int(1)],
    }];
    assert_eq!(
        Err(CopError::InvalidRange),
        fetch_chunk_for_test(&rows, &[2], &[1], 1)
    );
    assert_eq!(
        Err(CopError::InvalidRange),
        fetch_chunk_for_test(&rows, &[0], &[2], 0)
    );

    let mut handle_buffer = Vec::new();
    let mut index_buffer = Vec::new();
    assert_eq!(
        Err(CopError::ColumnOffset(1)),
        convert_row_to_handle_and_index_datum(
            &rows[0],
            &[0],
            &[1],
            false,
            &mut handle_buffer,
            &mut index_buffer,
        )
    );
}

#[test]
fn convert_row_builds_common_handle_from_all_handle_columns() {
    let row = ScanRow {
        key: vec![1],
        columns: vec![
            Datum::Text("tenant".to_owned()),
            Datum::Int(42),
            Datum::Bytes(vec![9]),
        ],
    };
    let mut handle_buffer = Vec::new();
    let mut index_buffer = Vec::new();
    let (handle, index) = convert_row_to_handle_and_index_datum(
        &row,
        &[0, 1],
        &[2],
        true,
        &mut handle_buffer,
        &mut index_buffer,
    )
    .unwrap();

    assert!(matches!(handle, Handle::Common(bytes) if !bytes.is_empty()));
    assert_eq!(vec![Datum::Bytes(vec![9])], index);
}
