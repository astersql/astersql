// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Notifier 表存储 `List`/`Read` 行为测试。
//
// 重点验证：向调用方传入的 `reused` 缓冲区反序列化时，若槽位已有残留
// （leftover）字段，应通过 `overwrite_from` 合并新事件，避免旧分区等字段泄漏。

use crate::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct MockRecordSet {
    rows: VecDeque<SqlRow>,
}

impl ddl_session::RecordSet for MockRecordSet {
    fn drain(&mut self, batch_size: usize) -> Result<Vec<SqlRow>, ddl_session::SessionError> {
        let count = batch_size.min(self.rows.len());
        Ok(self.rows.drain(..count).collect())
    }

    fn close(&mut self) -> Result<(), ddl_session::SessionError> {
        Ok(())
    }
}

type MockSqlTable = BTreeMap<(i64, i64), (Vec<u8>, u64)>;

struct MockSqlContext {
    variables: Arc<ddl_session::SessionVariables>,
    transaction: AtomicBool,
    rows: Mutex<MockSqlTable>,
    statements: Mutex<Vec<String>>,
}

impl Default for MockSqlContext {
    fn default() -> Self {
        Self {
            variables: Arc::new(ddl_session::SessionVariables::default()),
            transaction: AtomicBool::new(false),
            rows: Mutex::new(BTreeMap::new()),
            statements: Mutex::new(Vec::new()),
        }
    }
}

fn argument_i64(arguments: &[SqlValue], index: usize) -> i64 {
    match &arguments[index] {
        SqlValue::Integer(value) => *value,
        other => panic!("expected i64 argument at {index}, got {other:?}"),
    }
}

fn argument_u64(arguments: &[SqlValue], index: usize) -> u64 {
    match &arguments[index] {
        SqlValue::Unsigned(value) => *value,
        other => panic!("expected u64 argument at {index}, got {other:?}"),
    }
}

impl ddl_session::SessionContext for MockSqlContext {
    fn session_id(&self) -> u64 {
        326
    }

    fn session_variables(&self) -> Arc<ddl_session::SessionVariables> {
        self.variables.clone()
    }

    fn enter_new_transaction(
        &self,
        _mode: ddl_session::TransactionMode,
    ) -> Result<(), ddl_session::SessionError> {
        self.transaction.store(true, Ordering::Release);
        Ok(())
    }

    fn statement_commit(&self, _context: &ddl_session::ExecutionContext) {}

    fn commit_transaction(
        &self,
        _context: &ddl_session::ExecutionContext,
    ) -> Result<(), ddl_session::SessionError> {
        self.transaction.store(false, Ordering::Release);
        Ok(())
    }

    fn transaction(
        &self,
        _activate: bool,
    ) -> Result<Option<ddl_session::Transaction>, ddl_session::SessionError> {
        Ok(Some(ddl_session::Transaction {
            start_ts: 1,
            valid: self.transaction.load(Ordering::Acquire),
        }))
    }

    fn statement_rollback(
        &self,
        _context: &ddl_session::ExecutionContext,
        _pessimistic_retry: bool,
    ) {
    }

    fn rollback_transaction(&self, _context: &ddl_session::ExecutionContext) {
        self.transaction.store(false, Ordering::Release);
    }

    fn execute_internal(
        &self,
        _context: &ddl_session::ExecutionContext,
        query: &str,
        arguments: &[SqlValue],
    ) -> Result<Option<Box<dyn ddl_session::RecordSet>>, ddl_session::SessionError> {
        self.statements.lock().unwrap().push(query.to_owned());
        let normalized = query.trim_start();
        if normalized.starts_with("INSERT INTO") {
            let event = match &arguments[2] {
                SqlValue::Bytes(value) => value.clone(),
                other => panic!("expected JSON bytes, got {other:?}"),
            };
            self.rows.lock().unwrap().insert(
                (argument_i64(arguments, 0), argument_i64(arguments, 1)),
                (event, 0),
            );
            return Ok(None);
        }
        if normalized.starts_with("SELECT processed_by_flag") {
            let key = (argument_i64(arguments, 0), argument_i64(arguments, 1));
            let rows = self
                .rows
                .lock()
                .unwrap()
                .get(&key)
                .map(|(_, flag)| SqlRow {
                    values: vec![SqlValue::Unsigned(*flag)],
                })
                .into_iter()
                .collect();
            return Ok(Some(Box::new(MockRecordSet { rows })));
        }
        if normalized.starts_with("UPDATE") {
            let key = (argument_i64(arguments, 1), argument_i64(arguments, 2));
            self.rows.lock().unwrap().get_mut(&key).unwrap().1 = argument_u64(arguments, 0);
            return Ok(None);
        }
        if normalized.starts_with("DELETE FROM") {
            self.rows
                .lock()
                .unwrap()
                .remove(&(argument_i64(arguments, 0), argument_i64(arguments, 1)));
            return Ok(None);
        }
        if normalized.starts_with("SELECT ddl_job_id") {
            let cursor = (argument_i64(arguments, 0), argument_i64(arguments, 1));
            let limit = argument_i64(arguments, 2) as usize;
            let rows = self
                .rows
                .lock()
                .unwrap()
                .range((
                    std::ops::Bound::Excluded(cursor),
                    std::ops::Bound::Unbounded,
                ))
                .take(limit)
                .map(|(key, (event, flag))| SqlRow {
                    values: vec![
                        SqlValue::Integer(key.0),
                        SqlValue::Integer(key.1),
                        SqlValue::Bytes(event.clone()),
                        SqlValue::Unsigned(*flag),
                    ],
                })
                .collect();
            return Ok(Some(Box::new(MockRecordSet { rows })));
        }
        Err(ddl_session::SessionError::Sql(format!(
            "unexpected notifier SQL: {query}"
        )))
    }

    fn close(&self) {}
}

#[test]
/// 先写入多条 CreateTable 事件，再用带残留 `AddedPartInfo` 的缓冲区 `Read`，
/// 断言表名被覆盖为新值且残留分区信息仍保留（部分字段覆盖语义）。
fn test_leftover_when_unmarshal() {
    let store = OpenTableStore("test", "notifier");
    let session = Session::default();
    let new_table = model::TableInfo {
        Name: ast::NewCIStr("new"),
        Columns: vec![
            model::ColumnInfo {
                Name: ast::NewCIStr("c2"),
                ..Default::default()
            },
            model::ColumnInfo {
                Name: ast::NewCIStr("c3"),
                ..Default::default()
            },
        ],
        Indices: vec![model::IndexInfo {
            Name: ast::NewCIStr("i4"),
            ..Default::default()
        }],
        Constraints: vec![model::ConstraintInfo {
            Name: ast::NewCIStr("c1"),
            ..Default::default()
        }],
        ..Default::default()
    };
    // 连续插入 3 条同结构 CreateTable 事件，供后续 List 批量读取。
    for id in 1..=3 {
        PubSchemeChangeToStore(
            &session,
            id,
            1,
            NewCreateTableEvent(Some(Box::new(new_table.clone()))),
            store.as_ref(),
        )
        .unwrap();
    }
    let old_table = model::TableInfo {
        Name: ast::NewCIStr("old"),
        ..Default::default()
    };
    // 预填 reused 槽位：含旧表名事件、带 AddedPartInfo 的残留、以及空槽。
    let mut reused = vec![
        Some(SchemaChange {
            ddlJobID: 0,
            subJobID: 0,
            event: NewCreateTableEvent(Some(Box::new(old_table))),
            processedByFlag: 0,
        }),
        Some(SchemaChange {
            ddlJobID: 0,
            subJobID: 0,
            event: SchemaChangeEvent {
                inner: Some(JsonSchemaChangeEvent {
                    AddedPartInfo: Some(Box::new(model::PartitionInfo {
                        Expr: "test".to_owned(),
                        ..Default::default()
                    })),
                    ..Default::default()
                }),
            },
            processedByFlag: 0,
        }),
        None,
    ];
    let (mut result, close) = store.List(Session::default());
    assert_eq!(result.Read(&mut reused).unwrap(), 3);
    close();
    let expected_table = serde_json::to_value(&new_table).unwrap();
    for row in &reused {
        assert_eq!(
            serde_json::to_value(
                row.as_ref()
                    .unwrap()
                    .event
                    .inner
                    .as_ref()
                    .unwrap()
                    .TableInfo
                    .as_deref()
                    .unwrap()
            )
            .unwrap(),
            expected_table
        );
    }
    assert!(
        reused[1]
            .as_ref()
            .unwrap()
            .event
            .inner
            .as_ref()
            .unwrap()
            .AddedPartInfo
            .is_some()
    );
}

#[test]
fn open_table_store_instances_share_the_same_persistent_table() {
    let first = OpenTableStore("test", "notifier_shared");
    let second = OpenTableStore("test", "notifier_shared");
    PubSchemeChangeToStore(
        &Session::default(),
        326,
        -1,
        NewCreateTableEvent(Some(Box::new(model::TableInfo {
            ID: 42,
            Name: ast::NewCIStr("shared"),
            ..Default::default()
        }))),
        first.as_ref(),
    )
    .unwrap();

    let mut changes = vec![None; 1];
    let (mut result, close) = second.List(Session::default());
    assert_eq!(result.Read(&mut changes).unwrap(), 1);
    close();
    assert_eq!(
        changes[0]
            .as_ref()
            .unwrap()
            .event
            .GetCreateTableInfo()
            .unwrap()
            .Name
            .O,
        "shared"
    );
}

#[test]
fn insert_initializes_processed_by_flag_to_zero() {
    let store = OpenTableStore("test", "notifier_insert_flag");
    let session = Session::default();
    let change = SchemaChange {
        ddlJobID: 2484,
        subJobID: -1,
        event: NewCreateTableEvent(Some(Box::new(model::TableInfo::default()))),
        processedByFlag: u64::MAX,
    };

    store.Insert(&session, &change).unwrap();

    let mut changes = vec![None; 1];
    let (mut result, close) = store.List(Session::default());
    assert_eq!(result.Read(&mut changes).unwrap(), 1);
    close();
    assert_eq!(changes[0].as_ref().unwrap().processedByFlag, 0);
}

#[test]
fn table_store_uses_real_ddl_session_sql_transactions() {
    let context = Arc::new(MockSqlContext::default());
    let ddl_session = Arc::new(ddl_session::Session::new(context.clone()));
    let session = Session::FromDDLSession(ddl_session);
    let store = OpenTableStore("test", "notifier_sql");
    PubSchemeChangeToStore(
        &session,
        326,
        -1,
        NewCreateTableEvent(Some(Box::new(model::TableInfo {
            ID: 42,
            Name: ast::NewCIStr("sql"),
            Charset: "utf8mb4".to_owned(),
            ..Default::default()
        }))),
        store.as_ref(),
    )
    .unwrap();

    let mut changes = vec![None; 1];
    let (mut result, close) = store.List(session.clone());
    assert_eq!(result.Read(&mut changes).unwrap(), 1);
    close();
    assert_eq!(
        changes[0]
            .as_ref()
            .unwrap()
            .event
            .GetCreateTableInfo()
            .unwrap()
            .Charset,
        "utf8mb4"
    );

    session.BeginPessimistic().unwrap();
    store.UpdateProcessed(&session, 326, -1, 0, 1).unwrap();
    session.Commit().unwrap();
    assert_eq!(context.rows.lock().unwrap().get(&(326, -1)).unwrap().1, 1);

    store.DeleteAndCommit(&session, 326, -1).unwrap();
    assert!(context.rows.lock().unwrap().is_empty());
    let statements = context.statements.lock().unwrap();
    assert!(statements.iter().any(|sql| sql.starts_with("INSERT INTO")));
    assert!(
        statements
            .iter()
            .any(|sql| sql.starts_with("SELECT processed_by_flag"))
    );
    assert!(statements.iter().any(|sql| sql.starts_with("UPDATE")));
    assert!(statements.iter().any(|sql| sql.starts_with("DELETE FROM")));
}
