// 导入器解析、数据生成、批量写入与连接清理的回归测试。
//
// 测试通过共享状态的数据库替身记录 SQL、事务提交和连接关闭次数，重点校验
// Rust 实现与 Go 版本的解析语义一致，并保证并发作业的批次边界和失败清理行为。

use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
/// 数据库替身的共享观测状态，用于跨连接和事务汇总副作用。
struct DatabaseState {
    executed: Vec<String>,
    transactions: usize,
    committed: usize,
    closed: usize,
}

/// 将执行与提交动作写入共享状态的事务替身。
struct MockTransaction {
    state: Arc<Mutex<DatabaseState>>,
}

impl DatabaseTransaction for MockTransaction {
    fn execute(&mut self, sql: &str) -> Result<(), ImporterError> {
        self.state.lock().unwrap().executed.push(sql.to_owned());
        Ok(())
    }

    fn commit(self: Box<Self>) -> Result<(), ImporterError> {
        self.state.lock().unwrap().committed += 1;
        Ok(())
    }
}

/// 记录直接执行、开启事务和关闭连接次数的数据库替身。
struct MockDatabase {
    state: Arc<Mutex<DatabaseState>>,
}

impl Database for MockDatabase {
    fn execute(&self, sql: &str) -> Result<(), ImporterError> {
        self.state.lock().unwrap().executed.push(sql.to_owned());
        Ok(())
    }

    fn begin(&self) -> Result<Box<dyn DatabaseTransaction>, ImporterError> {
        self.state.lock().unwrap().transactions += 1;
        Ok(Box::new(MockTransaction {
            state: Arc::clone(&self.state),
        }))
    }

    fn close(&self) -> Result<(), ImporterError> {
        self.state.lock().unwrap().closed += 1;
        Ok(())
    }
}

/// 可在第 `fail_on_open` 次打开时注入错误的连接器替身。
struct MockConnector {
    state: Arc<Mutex<DatabaseState>>,
    fail_on_open: Option<usize>,
}

impl DatabaseConnector for MockConnector {
    fn open(&self, _: &str) -> Result<Arc<dyn Database>, ImporterError> {
        let mut state = self.state.lock().unwrap();
        state.executed.push("open".to_owned());
        let opened = state
            .executed
            .iter()
            .filter(|entry| entry.as_str() == "open")
            .count();
        if self.fail_on_open == Some(opened) {
            return Err(ImporterError::Database("open failed".to_owned()));
        }
        drop(state);
        Ok(Arc::new(MockDatabase {
            state: Arc::clone(&self.state),
        }))
    }
}

#[test]
// 外键约束不应被当作索引，普通注释也不能误触发无符号或范围语义。
fn parse_table_matches_go_constraint_and_rule_semantics() {
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (id INT COMMENT '[[range=1,2,3]]', value INT COMMENT 'unsigned', CONSTRAINT fk FOREIGN KEY (id) REFERENCES other(id), PRIMARY KEY (id))",
    )
    .unwrap();

    assert_eq!(table.name, "t");
    assert_eq!(table.column_list, "`id`,`value`");
    assert!(table.unique_indices.contains("id"));
    assert!(table.indices.is_empty());
    assert!(!table.find_column("value").unwrap().field_type.unsigned);
    let id = table.find_column("id").unwrap();
    assert!(id.minimum.is_empty());
    assert!(id.maximum.is_empty());
}

#[test]
// 索引是否唯一取决于 SQL 中的索引类型，而不是索引名称中的文本。
fn parse_index_uses_index_kind_not_index_name() {
    let mut table = Table::new();
    parse_table_sql(&mut table, "CREATE TABLE t (id INT, value INT)").unwrap();
    parse_index_sql(&mut table, "CREATE INDEX not_unique_name ON t (value)").unwrap();

    assert!(table.indices.contains_key("value"));
    assert!(!table.unique_indices.contains("value"));
}

#[test]
// 五个作业按每批两个写入时应生成五条插入语句，并提交三个事务批次。
fn generation_and_job_processing_preserve_row_and_batch_counts() {
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (id INT PRIMARY KEY COMMENT '[[range=10,20;step=2]]', name VARCHAR(8))",
    )
    .unwrap();
    let table = Arc::new(table);
    let state = Arc::new(Mutex::new(DatabaseState::default()));
    let db: Arc<dyn Database> = Arc::new(MockDatabase {
        state: Arc::clone(&state),
    });

    let first = generate_row_data(&table).unwrap();
    assert!(first.starts_with("insert into t (`id`,`name`) values (10,'"));
    let report = process_jobs(Arc::clone(&table), &[Arc::clone(&db), db], 5, 2, 2).unwrap();
    assert_eq!(report.jobs, 5);
    assert_eq!(state.lock().unwrap().committed, 3);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .executed
            .iter()
            .filter(|sql| sql.starts_with("insert into"))
            .count(),
        5
    );
}

#[test]
// 后续连接打开失败时，必须关闭此前已经成功建立的连接。
fn create_databases_closes_connections_when_opening_fails() {
    let state = Arc::new(Mutex::new(DatabaseState::default()));
    let connector = MockConnector {
        state: Arc::clone(&state),
        fail_on_open: Some(2),
    };
    let config = DbConfig {
        user: "u".to_owned(),
        password: "p".to_owned(),
        host: "h".to_owned(),
        port: 3306,
        schema: "s".to_owned(),
    };

    assert!(create_databases(&connector, &config, 3).is_err());
    assert_eq!(state.lock().unwrap().closed, 1);
}
