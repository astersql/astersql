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

// BRIE DDL 拆批工具的单元测试。
//
// 用会在多表时返回 entry too large 的 mock 上下文，验证
// `splitBatchCreateTable` 递归二分后按原始顺序完成建表，且
// `mergeQuerys` 拼接结果正确。

use std::collections::BTreeMap;

use crate::brie_utils::{
    BRIECreateDatabase, BRIECreateTable, BRIECreateTables, BrieDDLContext, mergeQuerys,
    showRestoredCreateDatabase, showRestoredCreateTable, splitBatchCreateTable,
};

/// 模拟 DDL 上下文：多表时报 entry too large，单表则记入 completed。
struct SplitContext {
    query: Option<String>,
    foreign_keys: bool,
    completed: Vec<String>,
    created_databases: Vec<String>,
    batch_queries: Vec<(String, Vec<String>, String)>,
    max_batch_size: usize,
    render_table_error: Option<String>,
    create_table_error: Option<String>,
    batch_error: Option<String>,
}

impl Default for SplitContext {
    fn default() -> Self {
        Self {
            query: None,
            foreign_keys: false,
            completed: Vec::new(),
            created_databases: Vec::new(),
            batch_queries: Vec::new(),
            max_batch_size: 1,
            render_table_error: None,
            create_table_error: None,
            batch_error: None,
        }
    }
}

impl BrieDDLContext for SplitContext {
    type Database = String;
    type Table = String;
    type CreateTableOption = String;
    type Error = String;

    fn render_create_database(&mut self, database: &String) -> Result<String, String> {
        Ok(format!("CREATE DATABASE `{database}`"))
    }
    fn render_create_table(&mut self, table: &String) -> Result<String, String> {
        if let Some(error) = self.render_table_error.clone() {
            return Err(error);
        }
        Ok(format!("CREATE TABLE `{table}`"))
    }
    fn ensure_default_charset(&mut self, database: &mut String) {
        database.push_str(":default-charset");
    }
    fn query_string(&self) -> Option<String> {
        self.query.clone()
    }
    fn set_query_string(&mut self, query: Option<String>) {
        self.query = query;
    }
    fn foreign_key_checks(&self) -> bool {
        self.foreign_keys
    }
    fn set_foreign_key_checks(&mut self, enabled: bool) {
        self.foreign_keys = enabled;
    }
    fn create_database_on_exist_error(&mut self, database: String) -> Result<(), String> {
        self.created_databases.push(database);
        Ok(())
    }
    fn create_table_on_exist_ignore(
        &mut self,
        _: &str,
        table: String,
        _: &[String],
    ) -> Result<(), String> {
        if let Some(error) = self.create_table_error.clone() {
            return Err(error);
        }
        self.completed.push(table);
        Ok(())
    }
    fn batch_create_tables_on_exist_ignore(
        &mut self,
        schema: &str,
        tables: &[String],
        _: &[String],
    ) -> Result<(), String> {
        self.batch_queries.push((
            schema.to_owned(),
            tables.to_vec(),
            self.query.clone().unwrap_or_default(),
        ));
        if let Some(error) = self.batch_error.clone() {
            return Err(error);
        }
        if tables.len() > self.max_batch_size {
            Err("entry too large".into())
        } else {
            self.completed.extend_from_slice(tables);
            Ok(())
        }
    }
    fn is_entry_or_transaction_too_large(&self, error: &String) -> bool {
        error == "entry too large"
    }
}

#[test]
/// 三表批量创建应递归拆到单表，completed 顺序与输入一致。
fn brie_batch_create_recursively_splits_and_preserves_query_order() {
    let tables = vec!["t1".to_owned(), "t2".to_owned(), "t3".to_owned()];
    let queries = vec![
        "CREATE TABLE `t1`".to_owned(),
        "CREATE TABLE `t2`".to_owned(),
        "CREATE TABLE `t3`".to_owned(),
    ];
    let mut context = SplitContext::default();
    splitBatchCreateTable(&mut context, "test", &tables, &queries, &[]).unwrap();
    assert_eq!(context.completed, tables);
    assert_eq!(
        mergeQuerys(&queries),
        "CREATE TABLE `t1`;CREATE TABLE `t2`;CREATE TABLE `t3`;"
    );
    assert_eq!(context.query, Some("CREATE TABLE `t3`;".into()));
}

#[test]
fn restored_sql_rendering_and_database_creation_match_go_contract() {
    let mut context = SplitContext {
        query: Some("original query".into()),
        ..Default::default()
    };

    assert_eq!(
        showRestoredCreateDatabase(&mut context, &"db".into(), "/*from(br)*/").unwrap(),
        "/*from(br)*/CREATE DATABASE `db`"
    );
    assert_eq!(
        showRestoredCreateTable(&mut context, &"t".into(), "").unwrap(),
        "CREATE TABLE `t`"
    );

    BRIECreateDatabase(&mut context, &"db".into(), "/*from(br)*/").unwrap();
    assert_eq!(context.query.as_deref(), Some("original query"));
    assert_eq!(context.created_databases, ["db:default-charset"]);
}

#[test]
fn single_table_creation_restores_session_state_on_ddl_error() {
    let mut context = SplitContext {
        query: Some("original query".into()),
        foreign_keys: true,
        create_table_error: Some("ddl failed".into()),
        ..Default::default()
    };

    let error = BRIECreateTable(&mut context, "db", &"t".into(), "/*br*/", &[]).unwrap_err();
    assert_eq!(error, "ddl failed");
    assert_eq!(context.query.as_deref(), Some("original query"));
    assert!(context.foreign_keys);
}

#[test]
fn table_render_error_does_not_mutate_session_state() {
    let mut context = SplitContext {
        query: Some("original query".into()),
        foreign_keys: true,
        render_table_error: Some("render failed".into()),
        ..Default::default()
    };

    let error = BRIECreateTable(&mut context, "db", &"t".into(), "", &[]).unwrap_err();
    assert_eq!(error, "render failed");
    assert_eq!(context.query.as_deref(), Some("original query"));
    assert!(context.foreign_keys);
}

#[test]
fn batch_creation_matches_each_split_with_its_query_and_restores_state() {
    let mut tables = BTreeMap::new();
    tables.insert("db1".into(), vec!["t1".into(), "t2".into()]);
    tables.insert("db2".into(), vec!["t3".into()]);
    let mut context = SplitContext {
        query: Some("original query".into()),
        foreign_keys: true,
        ..Default::default()
    };

    BRIECreateTables(&mut context, &tables, "/*from(br)*/", &[]).unwrap();

    assert_eq!(context.query.as_deref(), Some("original query"));
    assert!(context.foreign_keys);
    assert_eq!(context.completed, ["t1", "t2", "t3"]);
    assert_eq!(
        context.batch_queries,
        [
            (
                "db1".into(),
                vec!["t1".into(), "t2".into()],
                "/*from(br)*/CREATE TABLE `t1`;/*from(br)*/CREATE TABLE `t2`;".into(),
            ),
            (
                "db1".into(),
                vec!["t1".into()],
                "/*from(br)*/CREATE TABLE `t1`;".into(),
            ),
            (
                "db1".into(),
                vec!["t2".into()],
                "/*from(br)*/CREATE TABLE `t2`;".into(),
            ),
            (
                "db2".into(),
                vec!["t3".into()],
                "/*from(br)*/CREATE TABLE `t3`;".into(),
            ),
        ]
    );
}

#[test]
fn split_propagates_singleton_size_and_non_size_errors_without_retry() {
    let tables = vec!["t1".to_owned()];
    let queries = vec!["CREATE TABLE `t1`".to_owned()];
    let mut singleton = SplitContext {
        max_batch_size: 0,
        ..Default::default()
    };
    assert_eq!(
        splitBatchCreateTable(&mut singleton, "db", &tables, &queries, &[]),
        Err("entry too large".into())
    );
    assert_eq!(singleton.batch_queries.len(), 1);
    assert_eq!(singleton.query.as_deref(), Some("CREATE TABLE `t1`;"));

    let mut ordinary_error = SplitContext {
        batch_error: Some("permission denied".into()),
        ..Default::default()
    };
    let two_tables = vec!["t1".to_owned(), "t2".to_owned()];
    let two_queries = vec!["q1".to_owned(), "q2".to_owned()];
    assert_eq!(
        splitBatchCreateTable(&mut ordinary_error, "db", &two_tables, &two_queries, &[],),
        Err("permission denied".into())
    );
    assert_eq!(ordinary_error.batch_queries.len(), 1);
}
