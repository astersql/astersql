// Copyright 2026 AsterSQL.

// TiDB Driver 相关列信息转换的对照测试。
//
// `ConvertColumnInfo` 将规划器 ResultField 转为 MySQL 协议列元数据；
// 本测试校验别名、原始名、类型提升与显示宽度（字符集倍率）规则。

use crate::driver_tidb::{
    ColumnInfo, Error, Expression, NewTiDBDriver, PrepareResult, PreparedStmtInfo, ResultSet,
    SessionStates, SqlWarning, StatementStats, TiDBContext, TiDBSessionRuntime, TiDBStore,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Runtime {
    next_id: Mutex<u32>,
    current_db: Mutex<String>,
    set_next_ids: Mutex<Vec<u32>>,
    dropped: Mutex<Vec<u32>>,
    close_error: Mutex<Option<Error>>,
    memory: Option<Arc<crate::conn_stmt_test::TestLongDataMemory>>,
}

impl TiDBSessionRuntime for Runtime {
    fn long_data_memory(&self) -> Option<Arc<dyn crate::conn_stmt::LongDataMemory>> {
        self.memory
            .clone()
            .map(|memory| memory as Arc<dyn crate::conn_stmt::LongDataMemory>)
    }
    fn max_allowed_packet(&self) -> u64 {
        1024
    }
    fn configure(&self, _: u64, _: u32, _: u8) -> Result<(), Error> {
        Ok(())
    }
    fn prepare(&self, sql: &str) -> Result<PrepareResult, Error> {
        Ok(PrepareResult {
            id: 7,
            param_count: 1,
            columns: vec![],
            database: sql.into(),
        })
    }
    fn execute_prepared(
        &self,
        _: u32,
        _: &[Expression],
    ) -> Result<Option<Box<dyn ResultSet>>, Error> {
        Ok(None)
    }
    fn drop_prepared(&self, id: u32) -> Result<(), Error> {
        self.dropped.lock().unwrap().push(id);
        Ok(())
    }
    fn execute_statement(&self, _: &str, _: bool) -> Result<Option<Box<dyn ResultSet>>, Error> {
        Ok(None)
    }
    fn field_list(&self, _: &str) -> Result<Vec<ColumnInfo>, Error> {
        Ok(vec![])
    }
    fn warnings(&self) -> Vec<SqlWarning> {
        vec![]
    }
    fn statement_stats(&self) -> StatementStats {
        StatementStats::default()
    }
    fn sandbox_mode(&self) -> bool {
        false
    }
    fn restricted_sql(&self) -> bool {
        false
    }
    fn close(&self) -> Result<(), Error> {
        self.close_error.lock().unwrap().clone().map_or(Ok(()), Err)
    }
    fn prepared_metadata(&self) -> Result<HashMap<u32, PreparedStmtInfo>, Error> {
        Ok(HashMap::new())
    }
    fn next_prepared_id(&self) -> u32 {
        *self.next_id.lock().unwrap()
    }
    fn set_next_prepared_id(&self, id: u32) {
        *self.next_id.lock().unwrap() = id;
        self.set_next_ids.lock().unwrap().push(id);
    }
    fn current_db(&self) -> String {
        self.current_db.lock().unwrap().clone()
    }
    fn set_current_db(&self, database: &str) {
        *self.current_db.lock().unwrap() = database.into();
    }
    fn prepare_named(&self, _: &str, _: &str) -> Result<(), Error> {
        Ok(())
    }
}

fn context(runtime: Arc<Runtime>) -> TiDBContext {
    struct Store(Arc<Runtime>);
    impl TiDBStore for Store {
        fn create_session(&self) -> Result<Arc<dyn TiDBSessionRuntime>, Error> {
            Ok(self.0.clone())
        }
    }
    NewTiDBDriver(Arc::new(Store(runtime)))
        .OpenCtx(1, 0, 0)
        .unwrap()
}

#[test]
fn decode_empty_session_states_is_a_noop_like_go() {
    let runtime = Arc::new(Runtime::default());
    *runtime.next_id.lock().unwrap() = 11;
    let context = context(runtime.clone());

    context
        .DecodeSessionStates(&SessionStates::default())
        .unwrap();

    assert_eq!(*runtime.next_id.lock().unwrap(), 11);
    assert!(runtime.set_next_ids.lock().unwrap().is_empty());
}

#[test]
fn context_close_ignores_cleanup_errors_like_go_terror_call() {
    let runtime = Arc::new(Runtime::default());
    *runtime.close_error.lock().unwrap() = Some(Error("session close failed".into()));
    let context = context(runtime);

    assert_eq!(context.Close(), Ok(()));
}

#[test]
fn statement_close_releases_long_data_without_resetting_cursor() {
    let runtime = Arc::new(Runtime::default());
    let context = context(runtime.clone());
    let (statement, _, _) = context.Prepare("select ?").unwrap();
    let mut statement = statement.lock().unwrap();
    statement.AppendParam(0, b"value").unwrap();
    statement.SetCursorActive(true);

    statement.Close().unwrap();

    assert_eq!(statement.BoundParams(), &[None]);
    assert!(statement.GetCursorActive());
    assert_eq!(*runtime.dropped.lock().unwrap(), vec![7]);
}

#[test]
fn sandbox_allows_only_the_two_go_ast_statement_kinds() {
    let runtime = Arc::new(Runtime::default());
    // The mock exposes flags through a purpose-built wrapper below.
    struct SandboxRuntime(Arc<Runtime>);
    impl TiDBSessionRuntime for SandboxRuntime {
        fn configure(&self, a: u64, b: u32, c: u8) -> Result<(), Error> {
            self.0.configure(a, b, c)
        }
        fn prepare(&self, sql: &str) -> Result<PrepareResult, Error> {
            self.0.prepare(sql)
        }
        fn execute_prepared(
            &self,
            id: u32,
            args: &[Expression],
        ) -> Result<Option<Box<dyn ResultSet>>, Error> {
            self.0.execute_prepared(id, args)
        }
        fn drop_prepared(&self, id: u32) -> Result<(), Error> {
            self.0.drop_prepared(id)
        }
        fn execute_statement(
            &self,
            sql: &str,
            nt: bool,
        ) -> Result<Option<Box<dyn ResultSet>>, Error> {
            self.0.execute_statement(sql, nt)
        }
        fn field_list(&self, table: &str) -> Result<Vec<ColumnInfo>, Error> {
            self.0.field_list(table)
        }
        fn warnings(&self) -> Vec<SqlWarning> {
            vec![]
        }
        fn statement_stats(&self) -> StatementStats {
            StatementStats::default()
        }
        fn sandbox_mode(&self) -> bool {
            true
        }
        fn restricted_sql(&self) -> bool {
            false
        }
        fn close(&self) -> Result<(), Error> {
            Ok(())
        }
        fn prepared_metadata(&self) -> Result<HashMap<u32, PreparedStmtInfo>, Error> {
            Ok(HashMap::new())
        }
        fn next_prepared_id(&self) -> u32 {
            self.0.next_prepared_id()
        }
        fn set_next_prepared_id(&self, id: u32) {
            self.0.set_next_prepared_id(id)
        }
        fn current_db(&self) -> String {
            self.0.current_db()
        }
        fn set_current_db(&self, db: &str) {
            self.0.set_current_db(db)
        }
        fn prepare_named(&self, name: &str, sql: &str) -> Result<(), Error> {
            self.0.prepare_named(name, sql)
        }
    }
    struct Store(Arc<SandboxRuntime>);
    impl TiDBStore for Store {
        fn create_session(&self) -> Result<Arc<dyn TiDBSessionRuntime>, Error> {
            Ok(self.0.clone())
        }
    }
    let context = NewTiDBDriver(Arc::new(Store(Arc::new(SandboxRuntime(runtime)))))
        .OpenCtx(1, 0, 0)
        .unwrap();

    assert!(context.checkSandBoxMode("SET PASSWORD = 'x'").is_ok());
    assert!(
        context
            .checkSandBoxMode("ALTER USER u IDENTIFIED BY 'x'")
            .is_ok()
    );
    assert!(context.checkSandBoxMode("SET PASSWORDLESS = 1").is_err());
    assert!(context.checkSandBoxMode("ALTER USERNAME u").is_err());
}

/// 校验 VARCHAR + utf8mb4 时 ColumnLength 按 4 字节/字符放大，类型提升为 TypeVarString。
#[test]
fn canonical_convert_column_info_preserves_mysql_display_width_rules() {
    use astersql_meta_model::group_1 as model;
    use astersql_planner_core_resolve as resolve;
    use astersql_server_internal_column::{ConvertColumnInfo, mysql, types};
    use std::rc::Rc;

    // 构造元数据列：名称 org、VARCHAR flen=12、未指定小数位、utf8mb4。
    let mut column = model::ColumnInfo::default();
    column.Name = resolve::ast::NewCIStr("org");
    column.SetType(mysql::TypeVarchar);
    column.SetFlen(12);
    column.SetDecimal(types::UnspecifiedLength as isize);
    column.SetCharset("utf8mb4".into());
    let field = resolve::ResultField {
        column: Some(Rc::new(column)),
        column_as_name: resolve::ast::NewCIStr("alias"),
        empty_org_name: false,
        table: None,
        table_as_name: resolve::ast::NewCIStr("t"),
        db_name: resolve::ast::NewCIStr("db"),
    };
    let converted = ConvertColumnInfo(&field);
    assert_eq!(converted.Name, "alias");
    assert_eq!(converted.OrgName, "org");
    assert_eq!(converted.Type, mysql::TypeVarString);
    // utf8mb4 最大 4 字节/字符：12 * 4 = 48。
    assert_eq!(converted.ColumnLength, 48);
    assert_eq!(converted.Decimal, mysql::NotFixedDec as u8);
}

#[test]
fn statement_long_data_enforces_packet_limit_and_clears_deferred_error() {
    let context = context(Arc::new(Runtime::default()));
    let (statement, _, _) = context.Prepare("select ?").unwrap();
    let mut statement = statement.lock().unwrap();
    statement.AppendParam(0, &vec![b'a'; 1024]).unwrap();
    statement.CheckLongDataSize().unwrap();
    statement.AppendParam(0, b"b").unwrap();
    assert_eq!(statement.BoundParams()[0].as_ref().unwrap().len(), 1024);
    assert!(
        statement
            .CheckLongDataSize()
            .unwrap_err()
            .to_string()
            .contains("max_allowed_packet")
    );
    statement.AppendParam(0, &[]).unwrap();
    assert_eq!(statement.BoundParams()[0], Some(Vec::new()));
    assert!(statement.CheckLongDataSize().is_err());
    statement.Reset().unwrap();
    statement.CheckLongDataSize().unwrap();
    statement.AppendParam(0, b"c").unwrap();
    assert_eq!(statement.BoundParams()[0], Some(b"c".to_vec()));
    assert!(statement.AppendParam(1, b"bad").is_err());
}

#[test]
fn statement_long_data_mem_quota_releases_on_reset_empty_and_close() {
    let memory = crate::conn_stmt_test::TestLongDataMemory::new(1024);
    let context = context(Arc::new(Runtime {
        memory: Some(memory.clone()),
        ..Default::default()
    }));
    let (statement, _, _) = context.Prepare("select ?").unwrap();
    let mut statement = statement.lock().unwrap();
    statement.AppendParam(0, &vec![b'a'; 600]).unwrap();
    assert_eq!(memory.tracker.BytesConsumed(), 600);
    statement.CheckLongDataSize().unwrap();
    statement.AppendParam(0, &vec![b'b'; 424]).unwrap();
    assert_eq!(statement.BoundParams()[0].as_ref().unwrap().len(), 600);
    assert!(
        statement
            .CheckLongDataSize()
            .unwrap_err()
            .to_string()
            .contains("8175")
    );
    statement.AppendParam(0, &[]).unwrap();
    assert_eq!(memory.tracker.BytesConsumed(), 0);
    statement.AppendParam(0, b"x").unwrap();
    assert_eq!(statement.BoundParams()[0], Some(Vec::new()));
    assert!(statement.CheckLongDataSize().is_err());
    statement.Reset().unwrap();
    statement.AppendParam(0, &vec![b'c'; 1023]).unwrap();
    assert_eq!(memory.tracker.BytesConsumed(), 1023);
    statement.Close().unwrap();
    assert_eq!(memory.tracker.BytesConsumed(), 0);
    statement.CheckLongDataSize().unwrap();
}
