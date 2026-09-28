// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 本地临时表 DDL 单元测试（对应 Go temptable DDL tests）。
//
// 覆盖创建/删除/截断：全局 ID 分配、会话 MemBuffer 初始化、
// 重复创建错误，以及截断后旧键清空与新 table ID。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::{
    CiString, DbInfo, MemBuffer, Retriever, SchemaState, SessionContext, SessionTables,
    SessionVariables, SessionVarsProvider, Store, Table, TableInfo, TempTableError,
    TemporaryTableDdl, ValueEntry, encode_table_prefix, get_temporary_table_ddl,
};

/// 测试用 Store：内存 MemBuffer + 原子递增全局 ID。
struct TestStore {
    next_id: AtomicI64,
}

impl TestStore {
    /// 从 ID=1 起分配。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicI64::new(1),
        })
    }
}

impl Store for TestStore {
    fn begin(&self, _start_ts: u64) -> Result<Arc<MemBuffer>, TempTableError> {
        Ok(Arc::new(MemBuffer::new()))
    }

    fn generate_global_id(&self) -> Result<i64, TempTableError> {
        Ok(self.next_id.fetch_add(1, Ordering::SeqCst))
    }
}

/// 测试会话：绑定 Store 与 SessionVariables。
struct TestSession {
    store: Arc<dyn Store>,
    vars: Arc<SessionVariables>,
}

impl SessionVarsProvider for TestSession {
    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.vars)
    }
}

impl SessionContext for TestSession {
    fn store(&self) -> Arc<dyn Store> {
        Arc::clone(&self.store)
    }
}

/// 构造会话与 TemporaryTableDdl。
fn create_test_suite() -> (Arc<TestSession>, Arc<dyn TemporaryTableDdl>) {
    let store = TestStore::new();
    let session = Arc::new(TestSession {
        store,
        vars: Arc::new(SessionVariables::default()),
    });
    let ddl = get_temporary_table_ddl(session.clone() as Arc<dyn SessionContext>);
    (session, ddl)
}

/// 构造简化行键：表前缀 + `_` + handle 大端字节。
fn encode_row_key_with_handle(table_id: i64, handle: i64) -> Vec<u8> {
    let mut key = encode_table_prefix(table_id);
    // Record-row marker used by tablecodec; suffix uniqueness is enough for these tests.
    key.push(b'_');
    key.extend_from_slice(&handle.to_be_bytes());
    key
}

/// 构造 Public 状态的模拟 TableInfo。
fn new_mock_table(tbl_name: &str) -> TableInfo {
    TableInfo {
        name: CiString::new(tbl_name),
        state: SchemaState::Public,
        ..TableInfo::default()
    }
}

/// 构造固定 id=10 的模拟库。
fn new_mock_schema(schema_name: &str) -> Arc<DbInfo> {
    Arc::new(DbInfo {
        id: 10,
        name: CiString::new(schema_name),
    })
}

/// 读取会话 temporary_table_data。
fn session_data(session: &TestSession) -> Option<Arc<MemBuffer>> {
    session.vars.temporary_table_data.lock().unwrap().clone()
}

#[test]
/// 创建本地临时表：初始化目录/缓冲、ID 递增、重复名失败、跨库同名成功。
fn test_add_local_temporary_table() {
    let (session, ddl) = create_test_suite();
    let db1 = new_mock_schema("db1");
    let db2 = new_mock_schema("db2");
    let mut tbl1 = new_mock_table("t1");
    let mut tbl2 = new_mock_table("t2");

    assert!(
        session
            .vars
            .local_temporary_tables
            .lock()
            .unwrap()
            .is_none()
    );
    assert!(session.vars.temporary_table_data.lock().unwrap().is_none());

    ddl.create_local_temporary_table(Arc::clone(&db1), &mut tbl1)
        .unwrap();
    assert!(
        session
            .vars
            .local_temporary_tables
            .lock()
            .unwrap()
            .is_some()
    );
    assert!(session.vars.temporary_table_data.lock().unwrap().is_some());
    assert_eq!(tbl1.id, 1);
    let local = session
        .vars
        .local_temporary_tables
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert_eq!(got.metadata().id, tbl1.id);
    assert_eq!(got.metadata().name, tbl1.name);

    ddl.create_local_temporary_table(Arc::clone(&db1), &mut tbl2)
        .unwrap();
    assert_eq!(tbl2.id, 2);
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t2"))
        .unwrap();
    assert_eq!(got.metadata().id, tbl2.id);

    let k = encode_row_key_with_handle(tbl1.id, 1);
    let data = session_data(&session).unwrap();
    data.set_table_key(tbl1.id, k.clone(), b"v1".to_vec())
        .unwrap();
    let val = data.get(&k).unwrap();
    assert_eq!(val, ValueEntry::new(b"v1".to_vec(), 0));

    let mut tbl1x = new_mock_table("t1");
    let err = ddl
        .create_local_temporary_table(Arc::clone(&db1), &mut tbl1x)
        .unwrap_err();
    assert!(matches!(err, TempTableError::TableAlreadyExists(_)));
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert_eq!(got.metadata().id, tbl1.id);

    // Dup create still allocates a global ID (3), so the next success is 4.
    ddl.create_local_temporary_table(Arc::clone(&db2), &mut tbl1x)
        .unwrap();
    let got = local
        .table_by_name(&CiString::new("db2"), &CiString::new("t1"))
        .unwrap();
    assert_eq!(got.metadata().id, 4);
    assert_eq!(got.metadata().name.original(), "t1");
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert_eq!(got.metadata().id, tbl1.id);
}

#[test]
/// 同一 table ID 不能被不同名称的临时表复用（对应 Go AddTable 的 ID 索引检查）。
fn test_add_table_rejects_duplicate_table_id() {
    let tables = SessionTables::new();
    let database = new_mock_schema("db1");

    tables
        .add_table(
            Arc::clone(&database),
            Arc::new(Table::from_metadata(TableInfo {
                id: 42,
                name: CiString::new("t1"),
                ..TableInfo::default()
            })),
        )
        .unwrap();

    let err = tables
        .add_table(
            database,
            Arc::new(Table::from_metadata(TableInfo {
                id: 42,
                name: CiString::new("t2"),
                ..TableInfo::default()
            })),
        )
        .unwrap_err();
    assert!(matches!(err, TempTableError::TableAlreadyExists(_)));
    assert!(
        tables
            .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
            .is_some()
    );
    assert!(
        tables
            .table_by_name(&CiString::new("db1"), &CiString::new("t2"))
            .is_none()
    );
    assert_eq!(
        tables.table_by_id(42).unwrap().metadata().name,
        CiString::new("t1")
    );
}

#[test]
/// 删除本地临时表：不存在报错；成功后元数据消失且键变为空值删除语义。
fn test_remove_local_temporary_table() {
    let (session, ddl) = create_test_suite();
    let db1 = new_mock_schema("db1");

    let err = ddl
        .drop_local_temporary_table(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap_err();
    assert!(matches!(err, TempTableError::TableNotExists(_)));

    let mut tbl1 = new_mock_table("t1");
    ddl.create_local_temporary_table(Arc::clone(&db1), &mut tbl1)
        .unwrap();
    assert_eq!(tbl1.id, 1);
    let k = encode_row_key_with_handle(1, 1);
    let data = session_data(&session).unwrap();
    data.set_table_key(tbl1.id, k.clone(), b"v1".to_vec())
        .unwrap();

    assert!(matches!(
        ddl.drop_local_temporary_table(&CiString::new("db1"), &CiString::new("t2")),
        Err(TempTableError::TableNotExists(_))
    ));
    assert!(matches!(
        ddl.drop_local_temporary_table(&CiString::new("db2"), &CiString::new("t1")),
        Err(TempTableError::TableNotExists(_))
    ));

    let local = session
        .vars
        .local_temporary_tables
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let got = local.table_by_id(tbl1.id).unwrap();
    assert_eq!(got.metadata().id, tbl1.id);
    assert_eq!(data.get(&k).unwrap(), ValueEntry::new(b"v1".to_vec(), 0));

    ddl.drop_local_temporary_table(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert!(
        local
            .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
            .is_none()
    );
    assert_eq!(data.get(&k).unwrap(), ValueEntry::new(Vec::new(), 0));
}

#[test]
/// 截断：换新 ID，清空旧表键，不影响同库其他表数据。
fn test_truncate_local_temporary_table() {
    let (session, ddl) = create_test_suite();
    let db1 = new_mock_schema("db1");

    let err = ddl
        .truncate_local_temporary_table(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap_err();
    assert!(matches!(err, TempTableError::TableNotExists(_)));
    assert!(
        session
            .vars
            .local_temporary_tables
            .lock()
            .unwrap()
            .is_none()
    );
    assert!(session.vars.temporary_table_data.lock().unwrap().is_none());

    let mut tbl1 = new_mock_table("t1");
    ddl.create_local_temporary_table(Arc::clone(&db1), &mut tbl1)
        .unwrap();
    assert_eq!(tbl1.id, 1);
    let k = encode_row_key_with_handle(1, 1);
    let data = session_data(&session).unwrap();
    data.set_table_key(1, k.clone(), b"v1".to_vec()).unwrap();

    assert!(matches!(
        ddl.truncate_local_temporary_table(&CiString::new("db1"), &CiString::new("t2")),
        Err(TempTableError::TableNotExists(_))
    ));
    assert!(matches!(
        ddl.truncate_local_temporary_table(&CiString::new("db2"), &CiString::new("t1")),
        Err(TempTableError::TableNotExists(_))
    ));

    let local = session
        .vars
        .local_temporary_tables
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert_eq!(got.metadata().id, tbl1.id);
    assert_eq!(data.get(&k).unwrap(), ValueEntry::new(b"v1".to_vec(), 0));

    let mut tbl2 = new_mock_table("t2");
    ddl.create_local_temporary_table(Arc::clone(&db1), &mut tbl2)
        .unwrap();
    assert_eq!(tbl2.id, 2);
    let k2 = encode_row_key_with_handle(2, 1);
    data.set_table_key(2, k2.clone(), b"v2".to_vec()).unwrap();

    ddl.truncate_local_temporary_table(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    let got = local
        .table_by_name(&CiString::new("db1"), &CiString::new("t1"))
        .unwrap();
    assert_ne!(got.metadata().id, tbl1.id);
    assert_eq!(got.metadata().id, 3);
    assert_eq!(data.get(&k).unwrap(), ValueEntry::new(Vec::new(), 0));
    assert_eq!(data.get(&k2).unwrap(), ValueEntry::new(b"v2".to_vec(), 0));
}
