// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use astersql_kv::{Getter, Retriever};

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::{TypeLonglong, TypeVarchar};

use crate::adapter::{ExecExecutor, ExecutionContext};
use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::{KeyRange, TypedKVScan};

#[test]
fn typed_kv_scan_checks_canonical_kill_signal_inside_next_with_context() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (key, value) = encode_row(90, 1, 10, "row");
    retriever.Put(key, value);
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(90).0);
    let mut scan = TypedKVScan::new(
        retriever.clone(),
        90,
        false,
        false,
        vec![ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..Default::default()
        }],
        vec![KeyRange {
            end: start.PrefixNext(),
            start,
        }],
        1,
        2,
    );
    let killer = Arc::new(astersql_util_sqlkiller::sqlkiller::SQLKiller::new());
    let context = ExecutionContext {
        sql_killer: Some(killer.clone()),
        ..Default::default()
    };
    scan.Open().expect("open real encoded KV scan");
    killer.SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
    let mut output = scan.NewChunk();
    assert!(scan.NextWithContext(&context, &mut output).is_err());
    assert_eq!(output.NumRows(), 0);
    assert_eq!(retriever.IterCalls(), 0, "kill precedes iterator creation");
}

struct KillOnAdvanceRetriever {
    source: Arc<MemoryRetriever>,
    killer: Arc<astersql_util_sqlkiller::sqlkiller::SQLKiller>,
    closed: Arc<AtomicUsize>,
}

struct KillOnAdvanceIterator {
    inner: Box<dyn astersql_kv::Iterator>,
    killer: Arc<astersql_util_sqlkiller::sqlkiller::SQLKiller>,
    closed: Arc<AtomicUsize>,
}

impl astersql_kv::Iterator for KillOnAdvanceIterator {
    fn Valid(&self) -> bool {
        self.inner.Valid()
    }
    fn Key(&self) -> astersql_kv::Key {
        self.inner.Key()
    }
    fn Value(&self) -> Vec<u8> {
        self.inner.Value()
    }
    fn Next(&mut self) -> Result<(), astersql_errors::SharedError> {
        self.inner.Next()?;
        self.killer
            .SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
        Ok(())
    }
    fn Close(&mut self) {
        self.inner.Close();
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

impl astersql_kv::Getter for KillOnAdvanceRetriever {
    fn Get(
        &self,
        context: &astersql_kv::context::Context,
        key: astersql_kv::Key,
        options: &[astersql_kv::GetOption],
    ) -> Result<astersql_kv::ValueEntry, astersql_errors::SharedError> {
        self.source.Get(context, key, options)
    }
}

impl astersql_kv::Retriever for KillOnAdvanceRetriever {
    fn Iter(
        &self,
        start: astersql_kv::Key,
        end: Option<astersql_kv::Key>,
    ) -> Result<Box<dyn astersql_kv::Iterator>, astersql_errors::SharedError> {
        Ok(Box::new(KillOnAdvanceIterator {
            inner: self.source.Iter(start, end)?,
            killer: self.killer.clone(),
            closed: self.closed.clone(),
        }))
    }
    fn IterReverse(
        &self,
        start: Option<astersql_kv::Key>,
        end: Option<astersql_kv::Key>,
    ) -> Result<Box<dyn astersql_kv::Iterator>, astersql_errors::SharedError> {
        Ok(Box::new(KillOnAdvanceIterator {
            inner: self.source.IterReverse(start, end)?,
            killer: self.killer.clone(),
            closed: self.closed.clone(),
        }))
    }
}

#[test]
fn typed_kv_scan_cancels_mid_iterator_and_closes_the_real_kv_cursor() {
    let source = Arc::new(MemoryRetriever::default());
    for handle in [1, 2] {
        let (key, value) = encode_row(91, handle, handle * 10, "row");
        source.Put(key, value);
    }
    let killer = Arc::new(astersql_util_sqlkiller::sqlkiller::SQLKiller::new());
    let closed = Arc::new(AtomicUsize::new(0));
    let retriever = Arc::new(KillOnAdvanceRetriever {
        source,
        killer: killer.clone(),
        closed: closed.clone(),
    });
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(91).0);
    let mut scan = TypedKVScan::new(
        retriever,
        91,
        false,
        false,
        vec![ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..Default::default()
        }],
        vec![KeyRange {
            end: start.PrefixNext(),
            start,
        }],
        2,
        2,
    );
    scan.Open().unwrap();
    let context = ExecutionContext {
        sql_killer: Some(killer),
        ..Default::default()
    };
    let mut output = scan.NewChunk();
    assert!(scan.NextWithContext(&context, &mut output).is_err());
    assert_eq!(
        output.NumRows(),
        0,
        "interrupted page must not leak partial rows"
    );
    assert!(
        scan.TakeLockKeys().is_empty(),
        "interrupted page must not leak lock keys"
    );
    assert_eq!(
        closed.load(Ordering::SeqCst),
        1,
        "KV iterator closes on cancellation"
    );
}

#[test]
fn typed_kv_scan_reads_real_codec_rows_only_on_next_and_pages_without_duplicates() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, a, b) in [(1, 10, "one"), (2, 20, "two"), (3, 30, "three")] {
        let (key, value) = encode_row(88, handle, a, b);
        retriever.Put(key, value);
    }
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let columns = vec![
        ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..ColumnInfo::default()
        },
        ColumnInfo {
            ID: 2,
            Name: NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
            ..ColumnInfo::default()
        },
    ];
    let mut scan = TypedKVScan::new(
        retriever.clone(),
        88,
        false,
        false,
        columns,
        vec![KeyRange { start, end }],
        1,
        1,
    );
    assert_eq!(retriever.IterCalls(), 0);
    scan.Open().unwrap();
    assert_eq!(retriever.IterCalls(), 0);
    let mut output = scan.NewChunk();
    for (handle, a, b) in [(1, 10, "one"), (2, 20, "two"), (3, 30, "three")] {
        scan.Next(&mut output).unwrap();
        assert_eq!(output.NumRows(), 1);
        assert_eq!(output.GetRow(0).GetInt64(0), a);
        assert_eq!(output.GetRow(0).GetBytes(1), b.as_bytes());
        let (encoded, _) = encode_row(88, handle, a, b);
        assert_eq!(scan.TakeLockKeys(), vec![encoded.0]);
        assert!(
            scan.TakeLockKeys().is_empty(),
            "lock keys are consumed once"
        );
    }
    scan.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert!(scan.TakeLockKeys().is_empty());
    // A final empty iterator establishes EOF without reading ahead past the
    // third full output chunk.
    assert_eq!(retriever.IterCalls(), 4);
    scan.Next(&mut output).unwrap();
    assert_eq!(retriever.IterCalls(), 4);
    scan.Close().unwrap();
}

#[test]
fn typed_kv_scan_descending_pages_exact_half_open_range_without_duplicates() {
    let retriever = Arc::new(MemoryRetriever::default());
    for handle in [1, 2, 3] {
        let (key, value) = encode_row(88, handle, handle * 10, "row");
        retriever.Put(key, value);
    }
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..ColumnInfo::default()
    }];
    let mut scan = TypedKVScan::new(
        retriever,
        88,
        false,
        true,
        columns,
        vec![KeyRange { start, end }],
        1,
        1,
    );
    scan.Open().unwrap();
    let mut output = scan.NewChunk();
    for (handle, expected) in [(3, 30), (2, 20), (1, 10)] {
        scan.Next(&mut output).unwrap();
        assert_eq!(output.NumRows(), 1);
        assert_eq!(output.GetRow(0).GetInt64(0), expected);
        let (encoded, _) = encode_row(88, handle, expected, "row");
        assert_eq!(scan.TakeLockKeys(), vec![encoded.0]);
    }
    scan.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
}

#[test]
fn typed_kv_scan_descending_orders_multiple_encoded_ranges_globally() {
    let retriever = Arc::new(MemoryRetriever::default());
    let mut split = None;
    for handle in [1, 2, 3, 4] {
        let (key, value) = encode_row(88, handle, handle * 10, "row");
        if handle == 3 {
            split = Some(key.clone());
        }
        retriever.Put(key, value);
    }
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let split = split.expect("third encoded record key");
    let columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..ColumnInfo::default()
    }];
    let mut scan = TypedKVScan::new(
        retriever,
        88,
        false,
        true,
        columns,
        vec![
            KeyRange {
                start,
                end: split.clone(),
            },
            KeyRange { start: split, end },
        ],
        1,
        1,
    );
    scan.Open().unwrap();
    let mut output = scan.NewChunk();
    for expected in [40, 30, 20, 10] {
        scan.Next(&mut output).unwrap();
        assert_eq!(output.GetRow(0).GetInt64(0), expected);
    }
    scan.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
}

#[test]
fn detached_typed_scan_keeps_real_kv_snapshot_after_original_closes() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, a, b) in [(1, 10, "one"), (2, 20, "two")] {
        let (key, value) = encode_row(89, handle, a, b);
        retriever.Put(key, value);
    }
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(89).0);
    let end = start.PrefixNext();
    let columns = vec![
        ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..ColumnInfo::default()
        },
        ColumnInfo {
            ID: 2,
            Name: NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
            ..ColumnInfo::default()
        },
    ];
    let mut original = TypedKVScan::new(
        retriever.clone(),
        89,
        false,
        false,
        columns,
        vec![KeyRange { start, end }],
        1,
        1,
    );
    original.Open().unwrap();
    let mut output = original.NewChunk();
    original.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    let mut detached = original.Detach().expect("real KV scan can detach");
    let (first_key, _) = encode_row(89, 1, 10, "one");
    assert_eq!(original.TakeLockKeys(), vec![first_key.0]);
    assert!(
        detached.TakeLockKeys().is_empty(),
        "detach does not carry original statement lock bookkeeping"
    );
    original.Close().unwrap();
    drop(original);
    drop(retriever);
    let mut independent = detached.NewChunk();
    detached.Next(&mut independent).unwrap();
    assert_eq!(independent.GetRow(0).GetInt64(0), 20);
    detached.Close().unwrap();
}

#[test]
fn physical_builder_returns_a_typed_lazy_executor_for_real_kv_scan() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (key, value) = encode_row(90, 1, 77, "seventy-seven");
    retriever.Put(key, value);
    let mut plan = astersql_planner_core_operator_physicalop::PhysicalTableScan::New(context());
    plan.Table = Some(astersql_meta_model::TableInfo {
        ID: 90,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    plan.Columns = vec![
        ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..ColumnInfo::default()
        },
        ColumnInfo {
            ID: 2,
            Name: NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
            ..ColumnInfo::default()
        },
    ];
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(90).0);
    let end = start.PrefixNext();
    let mut executor = crate::builder::BuildTypedTableScan(
        &plan,
        retriever.clone(),
        vec![KeyRange { start, end }],
        1,
        1,
    )
    .unwrap();
    executor.Open().unwrap();
    assert_eq!(retriever.IterCalls(), 0);
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 77);
    executor.Close().unwrap();
}
