// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use autoid_dependency::Allocators;
use kv_dependency::{Handle, Key, Transaction};
use model_dependency::group_4 as model;
use tblctx_dependency::AllocatorContext;
use tblctx_dependency::stmtctx::ReservedRowIDAlloc;
use types_dependency::datum::{self, Datum};

use crate::{
    AddRecordOption, Column, Constraint, DupKeyCheckMode, Index, IndexMutateContext, MutateContext,
    NewAddRecordOpt, PartitionedTable, RemoveRecordOption, RowIDShardGenerator, Table, TableResult,
    Type, UpdateRecordOption, WrapTableWithMaterializedViewLog, columnAPI,
    mview_log::MLogSourceStmt,
};

#[derive(Clone)]
struct Event {
    table: &'static str,
    kind: &'static str,
    row: Vec<Datum>,
    skip_dup_check: bool,
    transaction_id: usize,
}

struct TestTable {
    name: &'static str,
    meta: model::TableInfo,
    events: Arc<Mutex<Vec<Event>>>,
    fail_add: bool,
    fail_update: bool,
    fail_remove: bool,
}

impl columnAPI for TestTable {
    fn Cols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
    fn VisibleCols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
    fn HiddenCols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
    fn WritableCols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
    fn DeletableCols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<Column>> {
        vec![]
    }
}

impl Table for TestTable {
    fn Indices(&self) -> Vec<Arc<dyn Index>> {
        vec![]
    }
    fn DeletableIndices(&self) -> Vec<Arc<dyn Index>> {
        vec![]
    }
    fn WritableConstraint(&self) -> Vec<Arc<Constraint>> {
        vec![]
    }
    fn RecordPrefix(&self) -> Key {
        Key(vec![])
    }
    fn IndexPrefix(&self) -> Key {
        Key(vec![])
    }
    fn AddRecord(
        &self,
        context: &mut dyn MutateContext,
        txn: &mut dyn Transaction,
        row: &[Datum],
        options: &[&dyn AddRecordOption],
    ) -> TableResult<Box<dyn Handle>> {
        if self.fail_add {
            return Err(errors_dependency::New("add failed"));
        }
        if self.name == "log" {
            let (allocator, available) = context.GetReservedRowIDAlloc();
            if available {
                allocator.unwrap().Consume();
            }
        }
        let event_number = self.events.lock().unwrap().len();
        txn.Set(
            Key(format!("{}_{}", self.name, event_number).into_bytes()),
            vec![1],
        )?;
        self.events.lock().unwrap().push(Event {
            table: self.name,
            kind: "add",
            row: row.to_vec(),
            skip_dup_check: NewAddRecordOpt(options).DupKeyCheck()
                == DupKeyCheckMode::DupKeyCheckSkip,
            transaction_id: std::ptr::from_ref(txn) as *const () as usize,
        });
        Ok(Box::new(kv_dependency::IntHandle(1)))
    }
    fn UpdateRecord(
        &self,
        _context: &mut dyn MutateContext,
        txn: &mut dyn Transaction,
        _handle: &dyn Handle,
        _old: &[Datum],
        new: &[Datum],
        _touched: &[bool],
        _options: &[&dyn UpdateRecordOption],
    ) -> TableResult<()> {
        if self.fail_update {
            return Err(errors_dependency::New("update failed"));
        }
        self.events.lock().unwrap().push(Event {
            table: self.name,
            kind: "update",
            row: new.to_vec(),
            skip_dup_check: false,
            transaction_id: std::ptr::from_ref(txn) as *const () as usize,
        });
        Ok(())
    }
    fn RemoveRecord(
        &self,
        _context: &mut dyn MutateContext,
        txn: &mut dyn Transaction,
        _handle: &dyn Handle,
        row: &[Datum],
        _options: &[&dyn RemoveRecordOption],
    ) -> TableResult<()> {
        if self.fail_remove {
            return Err(errors_dependency::New("remove failed"));
        }
        self.events.lock().unwrap().push(Event {
            table: self.name,
            kind: "remove",
            row: row.to_vec(),
            skip_dup_check: false,
            transaction_id: std::ptr::from_ref(txn) as *const () as usize,
        });
        Ok(())
    }
    fn Allocators(&self, _context: &mut dyn AllocatorContext) -> Allocators {
        Allocators::new(true, vec![])
    }
    fn Meta(&self) -> &model::TableInfo {
        &self.meta
    }
    fn UseNewCollate(&self) -> bool {
        false
    }
    fn Type(&self) -> Type {
        Type::NormalTable
    }
    fn GetPartitionedTable(&self) -> Option<&dyn PartitionedTable> {
        None
    }
}

#[derive(Default)]
struct TestContext {
    reserved: ReservedRowIDAlloc,
}

impl IndexMutateContext for TestContext {
    fn GetExprCtx(&self) -> &dyn tblctx_dependency::exprctx::ExprContext {
        unreachable!()
    }
    fn ConnectionID(&self) -> u64 {
        0
    }
    fn GetMutateBuffers(&mut self) -> &mut tblctx_dependency::MutateBuffers {
        unreachable!()
    }
}
impl AllocatorContext for TestContext {
    fn AlternativeAllocators(&mut self, _table: &model::TableInfo) -> (Allocators, bool) {
        unreachable!()
    }
}
impl MutateContext for TestContext {
    fn InRestrictedSQL(&self) -> bool {
        false
    }
    fn TxnAssertionLevel(&self) -> tblctx_dependency::variable::AssertionLevel {
        unreachable!()
    }
    fn EnableMutationChecker(&self) -> bool {
        false
    }
    fn GetRowEncodingConfig(&self) -> tblctx_dependency::RowEncodingConfig {
        unreachable!()
    }
    fn GetRowIDShardGenerator(&mut self) -> &mut dyn RowIDShardGenerator {
        unreachable!()
    }
    fn GetReservedRowIDAlloc(&mut self) -> (Option<&mut ReservedRowIDAlloc>, bool) {
        (Some(&mut self.reserved), true)
    }
    fn GetStatisticsSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::StatisticsSupport>, bool) {
        (None, false)
    }
    fn GetCachedTableSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::CachedTableSupport>, bool) {
        (None, false)
    }
    fn GetTemporaryTableSupport(
        &mut self,
    ) -> (
        Option<&mut dyn tblctx_dependency::TemporaryTableSupport>,
        bool,
    ) {
        (None, false)
    }
    fn GetExchangePartitionDMLSupport(
        &mut self,
    ) -> (Option<&mut dyn crate::ExchangePartitionDMLSupport>, bool) {
        (None, false)
    }
}

fn column(name: &str, offset: isize) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: model::ast::NewCIStr(name),
        Offset: offset,
        State: model::StatePublic,
        ..Default::default()
    }
}

fn wrapped(source: MLogSourceStmt) -> (Box<dyn Table>, Arc<Mutex<Vec<Event>>>) {
    wrapped_with_failures(source, false, false, false, false)
}

fn wrapped_with_failures(
    source: MLogSourceStmt,
    fail_base_add: bool,
    fail_log_add: bool,
    fail_update: bool,
    fail_remove: bool,
) -> (Box<dyn Table>, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(vec![]));
    let base = model::TableInfo {
        ID: 10,
        MaterializedViewBase: Some(model::MaterializedViewBaseInfo {
            MLogID: 20,
            ..Default::default()
        }),
        Columns: vec![column("untracked", 0), column("tracked", 1)],
        ..Default::default()
    };
    let log = model::TableInfo {
        ID: 20,
        MaterializedViewLog: Some(model::MaterializedViewLogInfo {
            BaseTableID: 10,
            Columns: vec![model::ast::NewCIStr("tracked")],
            ..Default::default()
        }),
        Columns: vec![
            column("tracked", 0),
            column(model::MaterializedViewLogDMLTypeColumnName, 1),
            column(model::MaterializedViewLogOldNewColumnName, 2),
        ],
        ..Default::default()
    };
    let table = WrapTableWithMaterializedViewLog(
        Box::new(TestTable {
            name: "base",
            meta: base,
            events: events.clone(),
            fail_add: fail_base_add,
            fail_update,
            fail_remove,
        }),
        Box::new(TestTable {
            name: "log",
            meta: log,
            events: events.clone(),
            fail_add: fail_log_add,
            fail_update: false,
            fail_remove: false,
        }),
        source,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    (table, events)
}

#[test]
fn go_merge_49_mlog_propagates_mutation_errors_without_extra_log_rows() {
    let row = vec![datum::NewIntDatum(1), datum::NewStringDatum("v".to_owned())];
    let mut txn = kv_dependency::test_fixtures::MockTxn::default();
    let mut context = TestContext::default();
    for (base_add, log_add, update, remove, expected_events) in [
        (true, false, false, false, 0),
        (false, true, false, false, 1),
    ] {
        let (table, events) =
            wrapped_with_failures(MLogSourceStmt::Insert, base_add, log_add, update, remove);
        assert!(table.AddRecord(&mut context, &mut txn, &row, &[]).is_err());
        assert_eq!(events.lock().unwrap().len(), expected_events);
    }
    let (table, events) = wrapped_with_failures(MLogSourceStmt::Update, false, false, true, false);
    assert!(
        table
            .UpdateRecord(
                &mut context,
                &mut txn,
                &kv_dependency::IntHandle(1),
                &row,
                &row,
                &[false, true],
                &[]
            )
            .is_err()
    );
    assert!(events.lock().unwrap().is_empty());
    let (table, events) = wrapped_with_failures(MLogSourceStmt::Delete, false, false, false, true);
    assert!(
        table
            .RemoveRecord(
                &mut context,
                &mut txn,
                &kv_dependency::IntHandle(1),
                &row,
                &[]
            )
            .is_err()
    );
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn go_merge_49_mlog_writes_real_table_mutations_and_restores_row_ids() {
    let (table, events) = wrapped(MLogSourceStmt::Insert);
    let mut context = TestContext::default();
    context.reserved.Reset(10, 20);
    let mut txn = kv_dependency::test_fixtures::MockTxn::default();
    let transaction_id = std::ptr::from_ref(&txn) as *const () as usize;
    let old = vec![
        datum::NewIntDatum(1),
        datum::NewStringDatum("old".to_owned()),
    ];
    let new = vec![
        datum::NewIntDatum(2),
        datum::NewStringDatum("new".to_owned()),
    ];
    table.AddRecord(&mut context, &mut txn, &old, &[]).unwrap();
    assert_eq!(context.reserved.Current(), (10, 20));
    table
        .UpdateRecord(
            &mut context,
            &mut txn,
            &kv_dependency::IntHandle(1),
            &old,
            &new,
            &[true, false],
            &[],
        )
        .unwrap();
    table
        .UpdateRecord(
            &mut context,
            &mut txn,
            &kv_dependency::IntHandle(1),
            &old,
            &new,
            &[false, true],
            &[],
        )
        .unwrap();
    table
        .RemoveRecord(
            &mut context,
            &mut txn,
            &kv_dependency::IntHandle(1),
            &old,
            &[],
        )
        .unwrap();
    table.AddRecord(&mut context, &mut txn, &new, &[]).unwrap();
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.transaction_id == transaction_id)
    );
    let log: Vec<_> = events.iter().filter(|event| event.table == "log").collect();
    assert_eq!(log.len(), 5);
    assert!(log.iter().all(|event| event.skip_dup_check));
    let expected = [
        ("old", "I", 1),
        ("old", "U", -1),
        ("new", "U", 1),
        ("old", "U", -1),
        ("new", "U", 1),
    ];
    for (event, (value, dml, marker)) in log.iter().zip(expected) {
        assert_eq!(event.kind, "add");
        assert_eq!(event.row[0].GetString(), value);
        assert_eq!(event.row[1].GetString(), dml);
        assert_eq!(event.row[2].GetInt64(), marker);
    }
}

#[test]
fn go_merge_49_mlog_base_and_log_writes_rollback_together() {
    let (table, _) = wrapped(MLogSourceStmt::Insert);
    let mut context = TestContext::default();
    let mut txn = kv_dependency::test_fixtures::MockTxn::default();
    table
        .AddRecord(
            &mut context,
            &mut txn,
            &[
                datum::NewIntDatum(1),
                datum::NewStringDatum("value".to_owned()),
            ],
            &[],
        )
        .unwrap();
    assert!(
        txn.pending_writes()
            .keys()
            .any(|key| key.0.starts_with(b"base_"))
    );
    assert!(
        txn.pending_writes()
            .keys()
            .any(|key| key.0.starts_with(b"log_"))
    );
    txn.Rollback().unwrap();
    assert!(txn.pending_writes().is_empty());
}
