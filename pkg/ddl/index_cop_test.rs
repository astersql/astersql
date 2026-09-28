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

use super::index_cop::{
    CopError, Datum, Handle, ScanRow, build_handle, build_table_scan, complete_error,
    extract_datum_by_offsets, fetch_table_scan_result, get_restore_data, wrap_in_begin_rollback,
};
use std::cell::RefCell;

fn rows() -> Vec<ScanRow> {
    (0_u8..8)
        .map(|value| ScanRow {
            key: vec![value + 1],
            columns: vec![
                Datum::Int(i64::from(value)),
                Datum::Int(i64::from(value)),
                Datum::Text(value.to_string()),
            ],
        })
        .collect()
}

#[test]
fn add_index_fetches_rows_through_the_production_coprocessor_helpers() {
    let scanned = build_table_scan(&rows(), &[1], &[9], None).unwrap().0;
    let handles = scanned
        .iter()
        .map(|row| build_handle(&row.columns[..1], false).unwrap())
        .collect::<Vec<_>>();
    assert_eq!((0..8).map(Handle::Int).collect::<Vec<_>>(), handles);

    let batches = RefCell::new(Vec::new());
    fetch_table_scan_result(&scanned, 3, |batch| {
        batches.borrow_mut().push(batch.to_vec());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        vec![3, 3, 2],
        batches.borrow().iter().map(Vec::len).collect::<Vec<_>>()
    );

    let common = build_handle(&[Datum::Text("0".into()), Datum::Text("0".into())], true).unwrap();
    assert!(matches!(common, Handle::Common(bytes) if !bytes.is_empty()));
    assert_eq!(Err(CopError::InvalidHandle), build_handle(&[], false));
}

#[test]
fn extraction_copies_each_rows_index_data_and_checks_offsets() {
    let source = rows();
    let mut buffer = Vec::new();
    let mut first = extract_datum_by_offsets(&source[0], &[1], &mut buffer).unwrap();
    let second = extract_datum_by_offsets(&source[1], &[1], &mut buffer).unwrap();
    first[0] = Datum::Int(99);
    assert_eq!(vec![Datum::Int(1)], second);
    assert_eq!(
        Err(CopError::ColumnOffset(3)),
        extract_datum_by_offsets(&source[0], &[3], &mut buffer)
    );
}

#[test]
fn begin_rollback_matches_go_error_and_cleanup_contract() {
    let calls = RefCell::new(Vec::new());
    let result = wrap_in_begin_rollback(
        42,
        || {
            calls.borrow_mut().push("begin");
            Ok(())
        },
        |start_ts| {
            calls.borrow_mut().push("operation");
            assert_eq!(42, start_ts);
            Ok(7)
        },
        || {
            calls.borrow_mut().push("rollback");
            Err("ignored by Go defer".into())
        },
    );
    assert_eq!(Ok(7), result);
    assert_eq!(vec!["begin", "operation", "rollback"], calls.into_inner());

    let operation_error = wrap_in_begin_rollback(
        1,
        || Ok(()),
        |_| Err::<(), _>(CopError::InvalidHandle),
        || Err("also ignored".into()),
    );
    assert_eq!(Err(CopError::InvalidHandle), operation_error);

    let rollback_called = RefCell::new(false);
    let begin_error = wrap_in_begin_rollback(
        1,
        || Err("begin".into()),
        |_| Ok(()),
        || {
            *rollback_called.borrow_mut() = true;
            Ok(())
        },
    );
    assert_eq!(Err(CopError::Transaction("begin".into())), begin_error);
    assert!(!rollback_called.into_inner());
}

#[test]
fn selection_range_errors_and_error_completion_are_preserved() {
    assert_eq!(
        Err(CopError::InvalidRange),
        build_table_scan(&rows(), &[2], &[2], None)
    );
    let (selected, pushed) = build_table_scan(
        &rows(),
        &[1],
        &[9],
        Some(&|row| row.columns[1] == Datum::Int(3)),
    )
    .unwrap();
    assert!(pushed);
    assert_eq!(
        vec![vec![4]],
        selected
            .iter()
            .map(|row| row.key.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        CopError::DuplicateKey("idx_b".into()),
        complete_error(CopError::DuplicateKey("placeholder".into()), "idx_b")
    );
}

#[test]
fn restore_data_follows_the_go_common_handle_gate() {
    let target = [Datum::Text("index value".into())];
    let primary = [
        Datum::Text("pk-a".into()),
        Datum::Null,
        Datum::Text("pk-c".into()),
    ];
    assert!(get_restore_data(&target, &primary, false).is_empty());
    assert_eq!(
        vec![Datum::Text("pk-a".into()), Datum::Text("pk-c".into())],
        get_restore_data(&target, &primary, true)
    );
}
