// Copyright 2026 AsterSQL.

//! `testkit::external` 元数据辅助函数的 Go 语义对齐测试。
//!
//! 通过可观测的模拟 Domain 与 InfoSchema，验证刷新时机、列集合选择和索引查找
//! 等迁移时容易发生偏差的行为，而不依赖真实存储或会话。

use super::{
    Column, Domain, ExternalError, ExternalResult, GetIndexID, GetModifyColumn, GetTableByName,
    Index, InfoSchema, TableMetadata, TestKitDomain,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 同时保留公开列与完整列，用于覆盖隐藏列是否参与查找的分支。
struct MockTable {
    columns: Vec<Column>,
    all_columns: Vec<Column>,
    indices: Vec<Index>,
}

impl TableMetadata for MockTable {
    fn columns(&self) -> &[Column] {
        &self.columns
    }

    fn all_columns(&self) -> &[Column] {
        &self.all_columns
    }

    fn indices(&self) -> &[Index] {
        &self.indices
    }
}

#[derive(Clone, Debug)]
/// 记录每次表名查询，便于断言辅助函数访问了预期的库表。
struct MockInfoSchema {
    table: MockTable,
    lookups: Arc<Mutex<Vec<(String, String)>>>,
    lookup_error: Option<ExternalError>,
}

impl InfoSchema for MockInfoSchema {
    type Table = MockTable;

    fn table_by_name(&self, database: &str, table: &str) -> ExternalResult<Self::Table> {
        self.lookups
            .lock()
            .unwrap()
            .push((database.to_owned(), table.to_owned()));
        if let Some(error) = &self.lookup_error {
            return Err(error.clone());
        }
        Ok(self.table.clone())
    }
}

#[derive(Clone, Debug)]
/// 用计数器暴露 schema 刷新副作用，区分两个查表辅助函数的刷新策略。
struct MockDomain {
    info_schema: MockInfoSchema,
    reload_calls: Arc<Mutex<usize>>,
    reload_error: Option<ExternalError>,
}

impl Domain for MockDomain {
    type InfoSchema = MockInfoSchema;

    fn reload(&self) -> ExternalResult<()> {
        *self.reload_calls.lock().unwrap() += 1;
        if let Some(error) = &self.reload_error {
            return Err(error.clone());
        }
        Ok(())
    }

    fn info_schema(&self) -> Self::InfoSchema {
        self.info_schema.clone()
    }
}

#[derive(Clone, Debug)]
/// 仅提供待测辅助函数要求的 Domain 入口。
struct MockTestKit {
    domain: MockDomain,
}

impl TestKitDomain for MockTestKit {
    type Domain = MockDomain;

    fn domain(&self) -> &Self::Domain {
        &self.domain
    }
}

fn test_kit() -> (
    MockTestKit,
    Arc<Mutex<usize>>,
    Arc<Mutex<Vec<(String, String)>>>,
) {
    // `_hidden` 只出现在完整列集合中，使测试能明确区分 `columns` 与 `all_columns`。
    let reload_calls = Arc::new(Mutex::new(0));
    let lookups = Arc::new(Mutex::new(Vec::new()));
    let table = MockTable {
        columns: vec![Column::new("visible", 1, 0, false)],
        all_columns: vec![
            Column::new("visible", 1, 0, false),
            Column::new("_hidden", 2, 1, true),
        ],
        indices: vec![Index::new("idx_name", 7)],
    };
    (
        MockTestKit {
            domain: MockDomain {
                info_schema: MockInfoSchema {
                    table,
                    lookups: lookups.clone(),
                    lookup_error: None,
                },
                reload_calls: reload_calls.clone(),
                reload_error: None,
            },
        },
        reload_calls,
        lookups,
    )
}

#[test]
fn get_table_by_name_reloads_before_lookup_like_go() {
    let (test_kit, reload_calls, lookups) = test_kit();

    let table = GetTableByName(&test_kit, "test_db", "test_table").unwrap();

    assert_eq!(*reload_calls.lock().unwrap(), 1);
    assert_eq!(
        lookups.lock().unwrap().as_slice(),
        [("test_db".into(), "test_table".into())]
    );
    assert_eq!(table.columns()[0].name, "visible");
}

#[test]
fn get_table_by_name_propagates_reload_and_lookup_errors() {
    let (mut test_kit, reload_calls, lookups) = test_kit();
    test_kit.domain.reload_error = Some(ExternalError::new("reload failed"));

    let error = GetTableByName(&test_kit, "test_db", "test_table").unwrap_err();

    assert_eq!(error.message(), "reload failed");
    assert_eq!(*reload_calls.lock().unwrap(), 1);
    assert!(lookups.lock().unwrap().is_empty());

    test_kit.domain.reload_error = None;
    test_kit.domain.info_schema.lookup_error = Some(ExternalError::new("lookup failed"));
    let error = GetTableByName(&test_kit, "test_db", "missing_table").unwrap_err();

    assert_eq!(error.message(), "lookup failed");
    assert_eq!(*reload_calls.lock().unwrap(), 2);
    assert_eq!(
        lookups.lock().unwrap().as_slice(),
        [("test_db".into(), "missing_table".into())]
    );
}

#[test]
fn get_modify_column_uses_public_or_all_columns_like_go() {
    let (test_kit, _, _) = test_kit();

    assert_eq!(
        GetModifyColumn(&test_kit, "test_db", "test_table", "VISIBLE", false)
            .unwrap()
            .unwrap()
            .id,
        1
    );
    assert_eq!(
        GetModifyColumn(&test_kit, "test_db", "test_table", "_HIDDEN", true)
            .unwrap()
            .unwrap()
            .id,
        2
    );
    assert_eq!(
        GetModifyColumn(&test_kit, "test_db", "test_table", "_hidden", false).unwrap(),
        None
    );
    assert_eq!(
        GetModifyColumn(&test_kit, "test_db", "test_table", "missing", true).unwrap(),
        None
    );
}

#[test]
fn get_index_id_uses_current_schema_like_go() {
    let (test_kit, reload_calls, lookups) = test_kit();

    // Go 辅助函数直接读取当前 InfoSchema；这里特意断言不会触发 Domain 刷新。
    assert_eq!(
        GetIndexID(&test_kit, "test_db", "test_table", "idx_name").unwrap(),
        7
    );
    assert_eq!(*reload_calls.lock().unwrap(), 0);
    assert_eq!(
        lookups.lock().unwrap().as_slice(),
        [("test_db".into(), "test_table".into())]
    );
}

#[test]
fn get_index_id_preserves_go_failure_contract() {
    let (mut test_kit, reload_calls, lookups) = test_kit();

    let error = GetIndexID(&test_kit, "test_db", "test_table", "IDX_NAME").unwrap_err();
    assert_eq!(
        error.message(),
        "index IDX_NAME not found(db: test_db, tbl: test_table)"
    );
    assert_eq!(*reload_calls.lock().unwrap(), 0);

    test_kit.domain.info_schema.lookup_error = Some(ExternalError::new("lookup failed"));
    let error = GetIndexID(&test_kit, "test_db", "missing_table", "idx_name").unwrap_err();
    assert_eq!(error.message(), "lookup failed");
    assert_eq!(*reload_calls.lock().unwrap(), 0);
    assert_eq!(
        lookups.lock().unwrap().as_slice(),
        [
            ("test_db".into(), "test_table".into()),
            ("test_db".into(), "missing_table".into()),
        ]
    );
}
