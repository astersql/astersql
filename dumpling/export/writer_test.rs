// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `writer_test.go`：Writer 各 Write* 路径的冒烟测试（MemStorage + mock 表数据）。
//! 不断言磁盘内容，仅验证路径可跑通、与 Go 测试矩阵一一对应。
//! 使用 MemStorage 与 mockTableIR 隔离外部依赖。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::main_test::{app_logger, default_config_for_test};
use crate::util_for_test::{bytes_cell, mockTableIR};
use crate::*;

// 构造内存 Storage + 脚本化 Conn 的 Writer，metrics 传 None 与 Go 单测一致。
fn new_test_writer(mut conf: Config) -> (Writer, Arc<MemStorage>) {
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    // MemStorage 避免单测依赖真实文件系统权限。
    let store = Arc::new(MemStorage::new("/tmp/dumpling-writer-test"));
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let writer = NewWriter(
        tcontext::Background().WithLogger(app_logger()),
        0,
        Arc::new(conf),
        conn,
        store.clone(),
        None,
    );
    (writer, store)
}

// 以下各 test_* 与 Go writer_test.go 同名，覆盖元数据与数据写出路径。
// 验证 CREATE DATABASE 元数据能经 OutputTemplate 写出且不 panic。
#[test]
fn test_write_database_meta() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (w, store) = new_test_writer(conf);
    // schema 模板输出 db-schema-create.sql。
    w.WriteDatabaseMeta("db", "CREATE DATABASE `db`").unwrap();
    assert_eq!(
        String::from_utf8(store.ReadFile("db-schema-create.sql").unwrap()).unwrap(),
        "/*!40014 SET FOREIGN_KEY_CHECKS=0*/;\n/*!40101 SET NAMES binary*/;\nCREATE DATABASE `db`;\n"
    );
}

// placement policy DDL 独立路径，对应 TiDB 扩展对象。
#[test]
fn test_write_policy_meta() {
    // policy 模板不含 db/table 段。
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (w, _) = new_test_writer(conf);
    // policy 模板不含 db/table 段。
    w.WritePolicyMeta("p1", "CREATE PLACEMENT POLICY `p1` LEARNERS=1;")
        .unwrap();
}

// 表元数据 WriteTableMeta：schema 模板 + .sql 后缀。
#[test]
fn test_write_table_meta() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (w, _) = new_test_writer(conf);
    // 表 DDL 写入 db.t-schema.sql。
    w.WriteTableMeta("db", "t", "CREATE TABLE `t`(a int);")
        .unwrap();
}

// 视图元数据需合并底层表 DDL 与 CREATE VIEW。
#[test]
fn test_write_view_meta() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (w, store) = new_test_writer(conf);
    // 视图测试覆盖双 DDL 拼接逻辑。
    w.WriteViewMeta(
        "db",
        "v",
        "CREATE TABLE `v`(a int)",
        "CREATE VIEW `v` AS SELECT 1",
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(store.ReadFile("db.v-schema.sql").unwrap()).unwrap(),
        "/*!40014 SET FOREIGN_KEY_CHECKS=0*/;\n/*!40101 SET NAMES binary*/;\nCREATE TABLE `v`(a int);\n"
    );
    assert_eq!(
        String::from_utf8(store.ReadFile("db.v-schema-view.sql").unwrap()).unwrap(),
        "/*!40014 SET FOREIGN_KEY_CHECKS=0*/;\n/*!40101 SET NAMES binary*/;\nCREATE VIEW `v` AS SELECT 1;\n"
    );
}

// 单行 INT 表数据 → SQL INSERT 文件，chunk index 0。
#[test]
fn test_write_table_data() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (mut w, store) = new_test_writer(conf);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new("db", "t", vec![vec![bytes_cell("1")]], &[], &["INT"]);
    // chunk 0 对应 IndexStr 000000000。
    w.WriteTableData(&meta, &mut ir, 0).unwrap();
    assert!(store.ReadFile("db.t.000000000.sql").is_ok());
    assert!(store.ReadFile("db.t.000000001.sql").is_err());
}

// FileSize=10 时两行宽 VARCHAR 应触发切文件或 flush，不断言文件数。
#[test]
fn test_write_table_data_with_file_size() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    // 10 字节限制迫使多行 VARCHAR 切分输出。
    // 极小 FileSize 迫使 early flush。
    conf.FileSize = 10;
    let (mut w, store) = new_test_writer(conf);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["VARCHAR"]);
    let mut ir = mockTableIR::new(
        "db",
        "t",
        vec![
            vec![bytes_cell("1234567890")],
            vec![bytes_cell("abcdefghij")],
        ],
        &[],
        &["VARCHAR"],
    );
    w.WriteTableData(&meta, &mut ir, 0).unwrap();
    assert!(store.ReadFile("db.t.000000000.sql").is_ok());
    assert!(store.ReadFile("db.t.000000001.sql").is_ok());
}

// Rows=1 限制每文件行数，与 FileSize 双重约束。
#[test]
fn test_write_table_data_with_file_size_and_rows() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    conf.FileSize = 100;
    // 每文件最多 1 行，与 FileSize 叠加。
    conf.Rows = 1;
    let (mut w, _) = new_test_writer(conf);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new(
        "db",
        "t",
        vec![vec![bytes_cell("1")], vec![bytes_cell("2")]],
        &[],
        &["INT"],
    );
    w.WriteTableData(&meta, &mut ir, 0).unwrap();
}

// StatementSize 过小会拆分多条 INSERT，此处仅验证不 panic。
#[test]
fn test_write_table_data_with_statement_size() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    // 限制单 INSERT 语句字节上限。
    conf.StatementSize = 20;
    let (mut w, store) = new_test_writer(conf);
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let mut ir = mockTableIR::new(
        "db",
        "t",
        vec![
            vec![bytes_cell("1")],
            vec![bytes_cell("2")],
            vec![bytes_cell("3")],
        ],
        &[],
        &["INT"],
    );
    w.WriteTableData(&meta, &mut ir, 0).unwrap();
    let sql = String::from_utf8(store.ReadFile("db.t.000000000.sql").unwrap()).unwrap();
    assert_eq!(sql.matches("INSERT INTO").count(), 3);
}

#[test]
fn test_output_file_namer_matches_go_split_indices() {
    let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    let template = OutputTemplate::default();

    let mut file_size_only = newOutputFileNamer(&meta, 7, false, true);
    assert_eq!(
        file_size_only.NextName(&template, "sql").unwrap().0,
        "db.t.000000000.sql"
    );
    assert_eq!(
        file_size_only.NextName(&template, "sql").unwrap().0,
        "db.t.000000001.sql"
    );

    let mut rows_and_file_size = newOutputFileNamer(&meta, 7, true, true);
    assert_eq!(
        rows_and_file_size.NextName(&template, "sql").unwrap().0,
        "db.t.0000000070000.sql"
    );
    assert_eq!(
        rows_and_file_size.NextName(&template, "sql").unwrap().0,
        "db.t.0000000070001.sql"
    );
}

#[test]
fn test_handle_task_runs_table_callback_only_for_last_chunk() {
    let mut conf = default_config_for_test();
    conf.FileType = FileFormatSQLTextString.into();
    let (mut writer, _) = new_test_writer(conf);
    let callbacks = Arc::new(AtomicUsize::new(0));
    let seen = callbacks.clone();
    writer.setFinishTableCallBack(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    }));

    for chunk in 0..2 {
        let meta = Box::new(mockTableIR::new("db", "t", vec![], &[], &["INT"]));
        let data = Box::new(mockTableIR::new("db", "t", vec![], &[], &["INT"]));
        let mut task = TaskEnum::TableData(NewTaskTableData(meta, data, chunk, 2));
        writer.handleTask(&mut task).unwrap();
    }

    assert_eq!(writer.received_task_count, 2);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
}

#[test]
fn csv_file_size_rotation_preserves_all_rows() {
    let dump = |limit| {
        let mut conf = default_config_for_test();
        conf.FileType = FileFormatCSVString.into();
        conf.NoHeader = true;
        conf.FileSize = limit;
        conf.CsvDelimiter = "\"".into();
        conf.CsvSeparator = ",".into();
        conf.CsvLineTerminator = "\n".into();
        let (mut writer, store) = new_test_writer(conf);
        let meta = mockTableIR::new("test", "employee", vec![], &[], &["INT", "VARCHAR"]);
        let mut ir = mockTableIR::new(
            "test",
            "employee",
            vec![
                vec![bytes_cell("1"), bytes_cell("bob@mail.com")],
                vec![bytes_cell("2"), bytes_cell("sarah@mail.com")],
                vec![bytes_cell("3"), bytes_cell("john@mail.com")],
                vec![bytes_cell("4"), bytes_cell("sarah@mail.com")],
            ],
            &[],
            &["INT", "VARCHAR"],
        );
        writer.WriteTableData(&meta, &mut ir, 0).unwrap();
        let mut files = Vec::new();
        for i in 0..10 {
            match store.ReadFile(&format!("test.employee.{i:09}.csv")) {
                Ok(bytes) => files.push(bytes),
                Err(_) => break,
            }
        }
        files
    };
    let split = dump(40);
    let whole = dump(UnspecifiedSize);
    assert_eq!(whole.len(), 1);
    assert!(split.len() > 1, "small FileSize must rotate CSV output");
    assert_eq!(split.concat(), whole[0]);
}

#[test]
fn data_files_forward_multipart_options_to_lazy_create() {
    struct RecordingStore {
        inner: MemStorage,
        options: std::sync::Mutex<Vec<Option<astersql_objstore_storeapi::WriterOption>>>,
    }
    impl Storage for RecordingStore {
        fn WriteFile(&self, n: &str, d: &[u8]) -> Result<()> {
            self.inner.WriteFile(n, d)
        }
        fn ReadFile(&self, n: &str) -> Result<Vec<u8>> {
            self.inner.ReadFile(n)
        }
        fn Create(&self, n: &str) -> Result<Box<dyn ObjectWriter>> {
            self.inner.Create(n)
        }
        fn CreateWithOptions(
            &self,
            n: &str,
            o: Option<&astersql_objstore_storeapi::WriterOption>,
        ) -> Result<Box<dyn ObjectWriter>> {
            self.options.lock().unwrap().push(o.cloned());
            self.inner.Create(n)
        }
        fn FilePath(&self) -> String {
            self.inner.FilePath()
        }
    }
    for format in [FileFormatCSVString, FileFormatSQLTextString] {
        let mut conf = default_config_for_test();
        conf.FileType = format.into();
        conf.NoHeader = true;
        let store = Arc::new(RecordingStore {
            inner: MemStorage::new("memory"),
            options: std::sync::Mutex::new(vec![]),
        });
        let mut writer = NewWriter(
            tcontext::Background(),
            0,
            Arc::new(conf),
            DB::new().Conn().unwrap(),
            store.clone(),
            None,
        );
        let meta = mockTableIR::new("db", "t", vec![], &[], &["INT"]);
        let mut ir = mockTableIR::new("db", "t", vec![vec![bytes_cell("1")]], &[], &["INT"]);
        writer.WriteTableData(&meta, &mut ir, 0).unwrap();
        let options = store.options.lock().unwrap();
        assert_eq!(options.len(), 1);
        let opt = options[0].as_ref().unwrap();
        assert_eq!(opt.Concurrency, 4);
        assert_eq!(opt.PartSize, 5 * 1024 * 1024);
    }
}
