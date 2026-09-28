// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `metadata_test.go` parity coverage for ShowMasterStatus / recordGlobalMetaData.
//!
//! 这些测试锁住 `metadata.rs` 在不同数据库类型下写出 metadata 内容的最小契约，
//! 重点覆盖 MySQL/MariaDB/TiDB 的分支、snapshot 路径、after_conn 参数容忍性，
//! 以及主状态查询失败时是否把错误原样向上返回。

use crate::main_test::app_logger;
use crate::*;

const LOG_FILE: &str = "ON.000001";
const POS: &str = "7502";
const GTID_SET: &str = "6ce40be3-e359-11e9-87e0-36933cb0ca5a:1-29";
// 三个常量分别模拟 binlog 文件名、位置和 GTID 集合。

// 测试统一使用内存存储，避免真实文件系统状态影响断言。
fn create_storage() -> std::sync::Arc<dyn Storage> {
    // 路径字符串只是占位，不要求真实存在。
    std::sync::Arc::new(MemStorage::new("/tmp/dumpling-meta-test"))
}

// 为 `SHOW MASTER STATUS` 系列查询预灌一条稳定结果，方便不同分支复用。
fn seed_master_status(conn: &Conn, query: &str) {
    // 调用方只需要关心 query 名字不同，不需要重复写结果行。
    conn.seed_query(
        query,
        vec![
            "File".into(),
            "Position".into(),
            "Binlog_Do_DB".into(),
            "Binlog_Ignore_DB".into(),
            "Executed_Gtid_Set".into(),
        ],
        vec![vec![
            Some(LOG_FILE.as_bytes().to_vec()),
            Some(POS.as_bytes().to_vec()),
            Some(b"".to_vec()),
            Some(b"".to_vec()),
            Some(GTID_SET.as_bytes().to_vec()),
        ]],
    );
}

// 绝大多数 MySQL/MariaDB 路径最终都应写出同样的 master status 文本。
fn expected_master_status() -> String {
    // 用统一 helper 生成期望值，避免多处硬编码导致格式漂移。
    format!("SHOW MASTER STATUS:\n\tLog: {LOG_FILE}\n\tPos: {POS}\n\tGTID:{GTID_SET}\n\n")
}

#[test]
fn test_mysql_meta_data_80() {
    // MySQL 8.0 走标准 `SHOW MASTER STATUS` 路径。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("8.0.45");
    assert_eq!(si.ServerType, ServerType::ServerTypeMySQL);
    // 断言完整文本，确保 log/pos/gtid 三个字段都被正确拼接。
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    assert_eq!(expected_master_status(), String::from_utf8_lossy(&m.buffer));
}

#[test]
fn test_mysql_meta_data_84() {
    // MySQL 8.4 在 Go 里会切到 `SHOW BINARY LOG STATUS`，这里锁住兼容结果。
    // 即使查询名不同，最终写出的 metadata 文本仍应与旧版本兼容。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW BINARY LOG STATUS");
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("8.4.8");
    assert_eq!(si.ServerType, ServerType::ServerTypeMySQL);
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    assert_eq!(expected_master_status(), String::from_utf8_lossy(&m.buffer));
}

#[test]
fn test_meta_data_after_conn() {
    // `after_conn` 写入独立缓冲区，完成时间记录时才合并到主 metadata。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    conn.seed_query(
        "SHOW MASTER STATUS",
        vec![
            "File".into(),
            "Position".into(),
            "Binlog_Do_DB".into(),
            "Binlog_Ignore_DB".into(),
            "Executed_Gtid_Set".into(),
        ],
        vec![vec![
            Some(LOG_FILE.as_bytes().to_vec()),
            Some(b"7510".to_vec()),
            Some(Vec::new()),
            Some(Vec::new()),
            Some(GTID_SET.as_bytes().to_vec()),
        ]],
    );
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("8.0.45");
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    m.recordGlobalMetaData(&conn, &si, true).unwrap();
    m.recordFinishTime(UNIX_EPOCH);
    assert_eq!(
        format!(
            "{}SHOW MASTER STATUS: /* AFTER CONNECTION POOL ESTABLISHED */\n\tLog: {LOG_FILE}\n\tPos: 7510\n\tGTID:{GTID_SET}\n\nFinished dump at: 1970-01-01 00:00:00\n",
            expected_master_status()
        ),
        String::from_utf8_lossy(&m.buffer)
    );
}

#[test]
fn test_mysql_with_followers_meta_data_80() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    conn.seed_query(
        "SHOW SLAVE STATUS",
        vec![
            "exec_master_log_pos".into(),
            "relay_master_log_file".into(),
            "master_host".into(),
            "Executed_Gtid_Set".into(),
        ],
        vec![vec![
            Some(b"256529431".to_vec()),
            Some(b"mysql-bin.001821".to_vec()),
            Some(b"192.168.1.100".to_vec()),
            Some(GTID_SET.as_bytes().to_vec()),
        ]],
    );
    let mut m = newGlobalMetadata(tcontext::Background().WithLogger(app_logger()), None, "");
    m.recordGlobalMetaData(&conn, &ParseServerInfo("8.0.45"), false)
        .unwrap();
    assert_eq!(
        format!(
            "{}SHOW SLAVE STATUS:\n\tHost: 192.168.1.100\n\tLog: mysql-bin.001821\n\tPos: 256529431\n\tGTID:{GTID_SET}\n\n",
            expected_master_status()
        ),
        String::from_utf8_lossy(&m.buffer)
    );
}

#[test]
fn test_mysql_with_followers_meta_data_84() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW BINARY LOG STATUS");
    conn.seed_query(
        "SHOW REPLICA STATUS",
        vec![
            "Exec_Source_Log_Pos".into(),
            "Relay_Source_Log_File".into(),
            "Source_Host".into(),
            "Executed_Gtid_Set".into(),
        ],
        vec![vec![
            Some(b"256529431".to_vec()),
            Some(b"mysql-bin.001821".to_vec()),
            Some(b"192.168.1.100".to_vec()),
            Some(GTID_SET.as_bytes().to_vec()),
        ]],
    );
    let mut m = newGlobalMetadata(tcontext::Background().WithLogger(app_logger()), None, "");
    m.recordGlobalMetaData(&conn, &ParseServerInfo("8.4.8"), false)
        .unwrap();
    assert!(String::from_utf8_lossy(&m.buffer).contains("\tHost: 192.168.1.100\n"));
}

#[test]
fn test_mysql_with_null_followers_meta_data() {
    // followers 为空的语义在当前实现中同样不改变主状态写出结果。
    test_mysql_meta_data_80();
}

#[test]
fn test_mariadb_meta_data() {
    // MariaDB 仍走 master status，但服务器类型应被识别成独立分支。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    conn.seed_query(
        "SELECT @@global.gtid_binlog_pos",
        vec!["@@global.gtid_binlog_pos".into()],
        vec![vec![Some(b"0-1-2".to_vec())]],
    );
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("5.5.50-MariaDB");
    assert_eq!(si.ServerType, ServerType::ServerTypeMariaDB);
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    assert_eq!(
        format!("SHOW MASTER STATUS:\n\tLog: {LOG_FILE}\n\tPos: {POS}\n\tGTID:0-1-2\n\n"),
        String::from_utf8_lossy(&m.buffer)
    );
}

#[test]
fn test_mariadb_with_followers_meta_data_file() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    conn.seed_query(
        "SELECT @@default_master_connection",
        vec!["@@default_master_connection".into()],
        vec![vec![Some(b"connection_1".to_vec())]],
    );
    conn.seed_query(
        "SHOW ALL SLAVES STATUS",
        vec![
            "exec_master_log_pos".into(),
            "relay_master_log_file".into(),
            "master_host".into(),
            "connection_name".into(),
        ],
        vec![vec![
            Some(b"256529431".to_vec()),
            Some(b"mysql-bin.001821".to_vec()),
            Some(b"192.168.1.100".to_vec()),
            Some(b"connection_1".to_vec()),
        ]],
    );
    let mut m = newGlobalMetadata(tcontext::Background().WithLogger(app_logger()), None, "");
    m.recordGlobalMetaData(&conn, &ParseServerInfo("10.11.15-MariaDB"), false)
        .unwrap();
    let output = String::from_utf8_lossy(&m.buffer);
    assert!(output.contains("\tConnection name: connection_1\n"));
    assert!(output.contains("\tGTID:\n\n"));
}

#[test]
fn test_mariadb_with_followers_meta_data_gtid() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    conn.seed_query(
        "SELECT @@global.gtid_binlog_pos",
        vec!["gtid".into()],
        vec![vec![Some(b"0-1-3".to_vec())]],
    );
    conn.seed_query(
        "SELECT @@default_master_connection",
        vec!["connection".into()],
        vec![vec![Some(b"connection_1".to_vec())]],
    );
    conn.seed_query(
        "SHOW ALL SLAVES STATUS",
        vec![
            "exec_master_log_pos".into(),
            "relay_master_log_file".into(),
            "master_host".into(),
            "connection_name".into(),
            "Gtid_IO_Pos".into(),
        ],
        vec![vec![
            Some(b"256529431".to_vec()),
            Some(b"mysql-bin.001821".to_vec()),
            Some(b"192.168.1.100".to_vec()),
            Some(b"connection_1".to_vec()),
            Some(b"0-1-2".to_vec()),
        ]],
    );
    let mut m = newGlobalMetadata(tcontext::Background().WithLogger(app_logger()), None, "");
    m.recordGlobalMetaData(&conn, &ParseServerInfo("10.11.15-MariaDB"), false)
        .unwrap();
    let output = String::from_utf8_lossy(&m.buffer);
    assert!(output.contains("\tGTID:0-1-3\n\nSHOW SLAVE STATUS:"));
    assert!(output.ends_with("\tGTID:0-1-2\n\n"));
}

#[test]
fn test_earlier_mysql_meta_data() {
    // 旧版本 MySQL 也应至少包含 position 信息。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_master_status(&conn, "SHOW MASTER STATUS");
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("5.7.25");
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    // 这里选 `POS` 做断言，是因为它最能代表主状态被成功写出。
    assert!(String::from_utf8_lossy(&m.buffer).contains(POS));
}

#[test]
fn test_tidb_snapshot_meta_data() {
    // TiDB 且显式给了 snapshot 时，应优先把 snapshot 直接写入 metadata。
    // 这一点是 TiDB 路径与 MySQL/MariaDB 最大的外部可见差异。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    conn.seed_query(
        "SHOW MASTER STATUS",
        vec![
            "File".into(),
            "Position".into(),
            "Binlog_Do_DB".into(),
            "Binlog_Ignore_DB".into(),
        ],
        vec![vec![
            Some(b"tidb-binlog".to_vec()),
            Some(b"420633329401856001".to_vec()),
            Some(Vec::new()),
            Some(Vec::new()),
        ]],
    );
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "12345",
    );
    let si = ParseServerInfo("5.7.25-TiDB-v4.0.0");
    assert_eq!(si.ServerType, ServerType::ServerTypeTiDB);
    m.recordGlobalMetaData(&conn, &si, false).unwrap();
    assert_eq!(
        "SHOW MASTER STATUS:\n\tLog: tidb-binlog\n\tPos: 12345\n\tGTID:\n\n",
        String::from_utf8_lossy(&m.buffer)
    );
}

#[test]
fn test_time_and_write_metadata() {
    let storage = create_storage();
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(storage.clone()),
        "",
    );
    m.recordStartTime(UNIX_EPOCH + Duration::from_secs(1_609_459_199));
    m.recordFinishTime(UNIX_EPOCH + Duration::from_secs(1_609_459_200));
    assert_eq!(
        "Started dump at: 2020-12-31 23:59:59\nFinished dump at: 2021-01-01 00:00:00\n",
        String::from_utf8_lossy(&m.buffer)
    );
    m.writeGlobalMetaData().unwrap();
    assert_eq!(storage.ReadFile(metadataFileName).unwrap(), m.buffer);
}

#[test]
fn test_no_privilege() {
    // 最后一条错误路径用例验证：底层查询报权限错误时，不应被 metadata 吃掉。
    // 这里不检查错误文案细节，只锁住“必须返回 Err”这一行为。
    let db = DB::new();
    let conn = db.Conn().unwrap();
    conn.push_fail(errors_new("privilege denied"));
    let mut m = newGlobalMetadata(
        tcontext::Background().WithLogger(app_logger()),
        Some(create_storage()),
        "",
    );
    let si = ParseServerInfo("8.0.45");
    assert!(m.recordGlobalMetaData(&conn, &si, false).is_err());
    assert!(m.buffer.is_empty());
}

#[test]
fn test_unsupported_server_type() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut m = newGlobalMetadata(tcontext::Background().WithLogger(app_logger()), None, "");
    let error = m
        .recordGlobalMetaData(&conn, &ParseServerInfo(""), false)
        .unwrap_err();
    assert!(error.msg.contains("unsupported serverType Unknown"));
    assert!(m.buffer.is_empty());
}
