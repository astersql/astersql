// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// server 生产运行时适配层的集成边界测试。
//
// 这里通过真实的回环 TCP 连接和 canonical session Domain，验证 MySQL 包编解码、
// PROXY 协议探测、鉴权与 SQL 执行，以及连接和会话生命周期没有在适配过程中丢失语义。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::conn::{
    AuthIdentity, AuthRequest, CancellationToken, Command, ConnError, ConnectionDomain, PacketIo,
    ResponseLifecycle, SessionDriver, TiDBContext, Value,
};
use crate::runtime::{
    BootstrapAuthMode, CanonicalConnectionDomain, CanonicalServerDomain, ConcreteSessionDriver,
    TcpPacketIo,
};

#[test]
fn go_merge_43_extract_archive_view_and_partitions() {
    use crate::server::Domain as _;
    use astersql_server_handler_extractorhandler::extractor::{
        ExtractTask, ExtractType, RequestContext, Timestamp,
    };
    use astersql_util_stmtsummary::{StmtExecInfo, StmtExecLazyInfo, StmtSummaryByDigestMap};
    struct ViewSql;
    impl StmtExecLazyInfo for ViewSql {
        fn GetOriginalSQL(&self) -> String {
            "SELECT id FROM extract_view_outer".into()
        }
        fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
            (String::new(), String::new(), None)
        }
        fn GetBinaryPlan(&self) -> String {
            String::new()
        }
        fn GetPlanDigest(&self) -> String {
            "extract-view-plan".into()
        }
        fn GetBindingSQLAndDigest(&self) -> (String, String) {
            (String::new(), String::new())
        }
    }
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let sql = astersql_session::runtime::ConcreteSession::new(Arc::clone(&domain));
    sql.execute("CREATE TABLE extract_partition_base (id INT PRIMARY KEY) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)").unwrap();
    sql.execute("INSERT INTO extract_partition_base VALUES (1), (11)")
        .unwrap();
    sql.execute("ANALYZE TABLE extract_partition_base").unwrap();
    sql.execute("CREATE VIEW extract_view_inner AS SELECT id FROM extract_partition_base")
        .unwrap();
    sql.execute("CREATE VIEW extract_view_outer AS SELECT id FROM extract_view_inner")
        .unwrap();
    let mut statement = StmtExecInfo {
        SchemaName: "test".into(),
        Digest: "extract-view-digest".into(),
        PlanDigest: "extract-view-plan".into(),
        StartTime: std::time::SystemTime::now(),
        LazyInfo: Box::new(ViewSql),
        ..StmtExecInfo::default()
    };
    statement.StmtCtx.StmtType = "Select".into();
    statement
        .StmtCtx
        .SetLogicalPlanTables(vec![astersql_sessionctx_stmtctx::TableEntry {
            DB: "test".into(),
            Table: "extract_view_outer".into(),
        }]);
    StmtSummaryByDigestMap
        .lock()
        .unwrap()
        .AddStatement(&statement);
    let runtime = CanonicalServerDomain::new(Arc::clone(&domain))
        .extract_runtime()
        .unwrap();
    let name = runtime
        .extract_task(
            &RequestContext::default(),
            ExtractTask {
                extract_type: ExtractType::Plan,
                is_background_job: false,
                begin: Timestamp(0),
                end: Timestamp(i64::MAX / 2),
                skip_stats: false,
                use_history_view: false,
            },
        )
        .unwrap();
    let path = format!("{}/{name}", runtime.extract_task_directory());
    let mut reader = runtime
        .open_extract(&RequestContext::default(), &path)
        .unwrap();
    let mut archive = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        archive.extend_from_slice(&buffer[..count]);
    }
    reader.close().unwrap();
    if let Ok(path) = std::env::var("ASTERSQL_EXTRACT_VIEW_ARCHIVE_FOR_GO") {
        std::fs::write(path, &archive).unwrap();
    }
    let package = astersql_domain::plan_replayer_dump::decode_replay_archive(&archive).unwrap();
    for entry in [
        "view/test.extract_view_outer.view.txt",
        "view/test.extract_view_inner.view.txt",
        "schema/test.extract_partition_base.schema.txt",
        "stats/test.extract_partition_base.json",
    ] {
        assert!(
            package.files.contains_key(entry),
            "missing archive entry {entry}"
        );
    }
    let stats: serde_json::Value =
        serde_json::from_slice(&package.files["stats/test.extract_partition_base.json"]).unwrap();
    assert!(stats["columns"].is_null());
    assert!(stats["indices"].is_null());
    assert!(stats["predicate_columns"].is_null());
    assert_eq!(stats["count"], 0);
    assert_eq!(stats["modify_count"], 0);
    assert_eq!(stats["version"], 0);
    assert_eq!(stats["is_historical_stats"], false);
    assert_eq!(stats["partitions"]["p0"]["count"], 1);
    assert_eq!(stats["partitions"]["p1"]["count"], 1);
    assert!(stats["partitions"]["p0"]["partitions"].is_null());
    let context = astersql_planner_extstore::Context::background();
    astersql_planner_extstore::GetGlobalExtStorage(&context)
        .unwrap()
        .DeleteFile(&context, &path)
        .unwrap();
    StmtSummaryByDigestMap.lock().unwrap().Clear();
    domain.close();
}

#[test]
fn go_merge_43_canonical_server_domain_serves_extract_archive() {
    use crate::server::Domain as _;
    use astersql_server_handler_extractorhandler::extractor::{
        ExtractTask, ExtractType, RequestContext, Timestamp,
    };
    use astersql_util_stmtsummary::{StmtExecInfo, StmtExecLazyInfo, StmtSummaryByDigestMap};
    struct SampleSql;
    impl StmtExecLazyInfo for SampleSql {
        fn GetOriginalSQL(&self) -> String {
            "SELECT id FROM extract_runtime_sample".into()
        }
        fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
            (String::new(), String::new(), None)
        }
        fn GetBinaryPlan(&self) -> String {
            String::new()
        }
        fn GetPlanDigest(&self) -> String {
            "extract-runtime-plan".into()
        }
        fn GetBindingSQLAndDigest(&self) -> (String, String) {
            (String::new(), String::new())
        }
    }
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let sql = astersql_session::runtime::ConcreteSession::new(Arc::clone(&domain));
    sql.execute(
        "CREATE TABLE extract_runtime_sample (id INT PRIMARY KEY, value INT, KEY ix_value(value))",
    )
    .unwrap();
    sql.execute("INSERT INTO extract_runtime_sample VALUES (1, 10), (2, 20), (3, 30)")
        .unwrap();
    sql.execute("ANALYZE TABLE extract_runtime_sample").unwrap();
    let mut statement = StmtExecInfo {
        SchemaName: "test".into(),
        Digest: "extract-runtime-digest".into(),
        PlanDigest: "extract-runtime-plan".into(),
        StartTime: std::time::SystemTime::now(),
        LazyInfo: Box::new(SampleSql),
        ..StmtExecInfo::default()
    };
    statement.StmtCtx.StmtType = "Select".into();
    statement
        .StmtCtx
        .SetLogicalPlanTables(vec![astersql_sessionctx_stmtctx::TableEntry {
            DB: "test".into(),
            Table: "extract_runtime_sample".into(),
        }]);
    StmtSummaryByDigestMap
        .lock()
        .unwrap()
        .AddStatement(&statement);
    let runtime = CanonicalServerDomain::new(Arc::clone(&domain))
        .extract_runtime()
        .expect("production Extract runtime");
    let name = runtime
        .extract_task(
            &RequestContext::default(),
            ExtractTask {
                extract_type: ExtractType::Plan,
                is_background_job: false,
                begin: Timestamp(0),
                end: Timestamp(i64::MAX / 2),
                skip_stats: false,
                use_history_view: false,
            },
        )
        .unwrap();
    let path = format!("{}/{name}", runtime.extract_task_directory());
    let mut reader = runtime
        .open_extract(&RequestContext::default(), &path)
        .unwrap();
    let mut archive = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        archive.extend_from_slice(&buffer[..count]);
    }
    reader.close().unwrap();
    if let Ok(path) = std::env::var("ASTERSQL_EXTRACT_ARCHIVE_FOR_GO") {
        std::fs::write(path, &archive).unwrap();
    }
    let package = astersql_domain::plan_replayer_dump::decode_replay_archive(&archive).unwrap();
    assert!(package.files.contains_key("extract_meta.txt"));
    assert_eq!(
        package.files["extract_meta.txt"],
        b"SkipStats = \"false\"\ntaskType = \"Plan\"\n"
    );
    assert!(package.files.contains_key("schema/schema_meta.txt"));
    assert!(package.files.contains_key("variables.toml"));
    let config = std::str::from_utf8(&package.files["config.toml"]).unwrap();
    let config: toml::Value = toml::from_str(config).unwrap();
    assert!(config.get("store").is_some());
    assert!(package.files["meta.txt"].starts_with(b"Release Version: "));
    assert!(package.files.contains_key("global_bindings.sql"));
    assert_eq!(
        std::str::from_utf8(&package.files["schema/schema_meta.txt"]).unwrap(),
        "test.extract_runtime_sample;"
    );
    assert!(
        package
            .files
            .contains_key("schema/test.extract_runtime_sample.schema.txt")
    );
    assert!(
        package.files["schema/test.extract_runtime_sample.schema.txt"]
            .starts_with(b"create database if not exists `test`; use `test`;CREATE TABLE")
    );
    assert!(
        package
            .files
            .contains_key("SQLs/extract-runtime-digest.json")
    );
    assert!(
        package
            .files
            .contains_key("stats/test.extract_runtime_sample.json")
    );
    let sql_record: serde_json::Value =
        serde_json::from_slice(&package.files["SQLs/extract-runtime-digest.json"]).unwrap();
    assert_eq!(sql_record["schema"], "test");
    assert_eq!(sql_record["sql"], "SELECT id FROM extract_runtime_sample");
    assert_eq!(sql_record["digest"], "extract-runtime-digest");
    let stats: serde_json::Value =
        serde_json::from_slice(&package.files["stats/test.extract_runtime_sample.json"]).unwrap();
    assert_eq!(stats["database_name"], "test");
    assert_eq!(stats["table_name"], "extract_runtime_sample");
    assert!(stats["columns"].is_object());
    assert_eq!(stats["count"], 3);
    assert!(stats["columns"]["id"]["histogram"]["ndv"].is_number());
    assert!(stats["indices"]["ix_value"]["histogram"]["ndv"].is_number());
    assert!(stats["indices"].is_object());
    assert!(stats["count"].is_number());
    assert!(stats["modify_count"].is_number());
    assert!(stats["version"].is_number());
    let skip_name = runtime
        .extract_task(
            &RequestContext::default(),
            ExtractTask {
                extract_type: ExtractType::Plan,
                is_background_job: false,
                begin: Timestamp(0),
                end: Timestamp(i64::MAX / 2),
                skip_stats: true,
                use_history_view: false,
            },
        )
        .unwrap();
    let skip_path = format!("{}/{}", runtime.extract_task_directory(), skip_name);
    let mut skip_reader = runtime
        .open_extract(&RequestContext::default(), &skip_path)
        .unwrap();
    let mut skip_archive = Vec::new();
    loop {
        let count = skip_reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        skip_archive.extend_from_slice(&buffer[..count]);
    }
    skip_reader.close().unwrap();
    if let Ok(path) = std::env::var("ASTERSQL_EXTRACT_SKIP_ARCHIVE_FOR_GO") {
        std::fs::write(path, &skip_archive).unwrap();
    }
    let skipped =
        astersql_domain::plan_replayer_dump::decode_replay_archive(&skip_archive).unwrap();
    assert_eq!(
        skipped.files["extract_meta.txt"],
        b"SkipStats = \"true\"\ntaskType = \"Plan\"\n"
    );
    assert!(!skipped.files.keys().any(|name| name.starts_with("stats/")));
    assert_eq!(
        runtime
            .extract_task(
                &RequestContext::default(),
                ExtractTask {
                    extract_type: ExtractType::Plan,
                    is_background_job: true,
                    begin: Timestamp(0),
                    end: Timestamp(i64::MAX / 2),
                    skip_stats: true,
                    use_history_view: false,
                },
            )
            .unwrap(),
        ""
    );
    let context = astersql_planner_extstore::Context::background();
    astersql_planner_extstore::GetGlobalExtStorage(&context)
        .unwrap()
        .DeleteFile(&context, &path)
        .unwrap();
    astersql_planner_extstore::GetGlobalExtStorage(&context)
        .unwrap()
        .DeleteFile(&context, &skip_path)
        .unwrap();
    StmtSummaryByDigestMap.lock().unwrap().Clear();
    domain.close();
}

/// 创建一对已经建立连接的回环 TCP 流，分别模拟客户端和服务端。
fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let address = listener.local_addr().expect("loopback listener address");
    let client = TcpStream::connect(address).expect("connect loopback client");
    let (server, _) = listener.accept().expect("accept loopback client");
    (client, server)
}

#[test]
fn write_sql_resp_accumulates_protocol_sections_before_finish() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callback_observed = Arc::clone(&observed);
    let lifecycle = ResponseLifecycle::new(move |duration| {
        callback_observed.lock().unwrap().push(duration);
    });

    lifecycle.add_write_duration(Duration::from_millis(3));
    lifecycle.add_write_duration(Duration::from_millis(5));
    lifecycle.finish();
    lifecycle.finish();

    assert_eq!(*observed.lock().unwrap(), [Duration::from_millis(8)]);
}

#[test]
fn write_sql_resp_excludes_fetch_returned_callback_time() {
    let observed = Arc::new(Mutex::new(Duration::ZERO));
    let callback_observed = Arc::clone(&observed);
    let lifecycle = ResponseLifecycle::new(move |duration| {
        *callback_observed.lock().unwrap() = duration;
    });

    lifecycle.add_write_duration(Duration::from_millis(2));
    thread::sleep(Duration::from_millis(20));
    lifecycle.add_write_duration(Duration::from_millis(4));
    lifecycle.finish();

    assert_eq!(*observed.lock().unwrap(), Duration::from_millis(6));
}

/// 基于真实 canonical Domain 构造测试驱动，并仅允许 root 空密码登录。
fn session_driver() -> ConcreteSessionDriver {
    let (domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize canonical domain");
    ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly)
}

/// 构造固定来源和插件的握手鉴权请求，便于只改变用户名与响应数据。
fn auth(user: &str, auth_data: &[u8]) -> AuthRequest {
    AuthRequest {
        identity: AuthIdentity {
            username: user.to_owned(),
            hostname: "127.0.0.1".to_owned(),
            plugin: "mysql_native_password".to_owned(),
        },
        auth_data: auth_data.to_vec(),
        salt: vec![7; 20],
        tls_state: None,
        attributes: Default::default(),
    }
}

/// 验证生产 PacketIO 的读写、序列号、超时、地址和关闭状态均透传到 TCP 套接字。
#[test]
fn tcp_packet_io_flushes_existing_packet_codec_and_exposes_socket_lifecycle() {
    let (mut client, server) = tcp_pair();
    client
        .write_all(&[3, 0, 0, 0, b'o', b'n', b'e'])
        .expect("write client packet");

    let mut packet = TcpPacketIo::new(server, 32 * 1024 * 1024).expect("production packet IO");
    assert_eq!(packet.read_packet().expect("read packet"), b"one");
    let peer = packet.peer_addr().expect("peer address");
    let local = packet.local_addr().expect("local address");
    assert_eq!(peer.0, "127.0.0.1");
    assert_eq!(local.0, "127.0.0.1");
    assert!(!peer.1.is_empty());
    assert!(!local.1.is_empty());

    packet.write_packet(b"two").expect("encode server packet");
    packet.flush().expect("flush encoded bytes to socket");
    let mut response = [0_u8; 7];
    client
        .read_exact(&mut response)
        .expect("read server packet");
    assert_eq!(response, [3, 0, 0, 1, b't', b'w', b'o']);

    packet.reset_sequence();
    packet
        .set_read_timeout(Duration::from_millis(30))
        .expect("install socket read timeout");
    let started = Instant::now();
    assert!(
        packet.read_packet().is_err(),
        "idle socket read must time out"
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    packet.close().expect("close socket");
    assert!(!packet.connection_alive());
}

/// PROXY 协议必须延迟到首次读取时探测，以允许服务端先写出握手包再等待客户端数据。
#[test]
fn proxy_fallback_is_lazy_until_after_server_handshake_write() {
    let (mut client, server) = tcp_pair();
    let client = thread::spawn(move || {
        let mut handshake = [0_u8; 9];
        client
            .read_exact(&mut handshake)
            .expect("receive server handshake before sending client bytes");
        assert_eq!(&handshake[4..], b"hello");
        client
            .write_all(&[3, 0, 0, 1, b'o', b'n', b'e'])
            .expect("write plain MySQL packet without PROXY header");
    });
    let mut packet = TcpPacketIo::new_with_options(
        server,
        32 * 1024 * 1024,
        None,
        None,
        Some(("127.0.0.0/8".into(), true, Duration::from_secs(1))),
    )
    .expect("proxy-aware packet IO");
    packet.write_packet(b"hello").unwrap();
    packet.flush().unwrap();
    assert_eq!(packet.read_packet().unwrap(), b"one");
    assert_eq!(packet.peer_addr().unwrap().0, "127.0.0.1");
    client.join().unwrap();
}

/// 跨越单包上限一个字节时应复用标准分包逻辑，并为后续分片递增序列号。
#[test]
fn tcp_packet_io_flushes_mysql_multi_packet_payload_without_reimplementing_framing() {
    let (mut client, server) = tcp_pair();
    let mut packet = TcpPacketIo::new(server, 64 * 1024 * 1024).expect("production packet IO");
    let payload = vec![b'x'; astersql_server_internal::MAX_PAYLOAD_LEN + 1];
    let expected_len = payload.len() + 8;
    // 大包写入可能填满套接字缓冲区，因此由并发读端持续排空全部编码结果。
    let reader = thread::spawn(move || {
        let mut encoded = vec![0; expected_len];
        client
            .read_exact(&mut encoded)
            .expect("read all split packet bytes");
        encoded
    });

    packet.write_packet(&payload).expect("encode split payload");
    packet.flush().expect("flush split payload");
    let encoded = reader.join().expect("join loopback reader");
    assert_eq!(&encoded[..4], &[0xff, 0xff, 0xff, 0]);
    let second = 4 + astersql_server_internal::MAX_PAYLOAD_LEN;
    assert_eq!(&encoded[second..second + 4], &[1, 0, 0, 1]);
    assert_eq!(encoded[second + 4], b'x');
}

/// 验证具体会话驱动贯通真实鉴权、SQL 结果元数据、字段查询、取消和关闭语义。
#[test]
fn concrete_session_driver_authenticates_and_returns_real_sql_results() {
    let driver = session_driver();
    let context = driver
        .open_ctx(81, 1 << 16, 46, "", None)
        .expect("open concrete context");

    context
        .authenticate(&auth("root", &[]))
        .expect("insecure root empty password");
    assert!(matches!(
        context.authenticate(&auth("root", b"password-response")),
        Err(ConnError::AccessDenied { .. })
    ));
    assert!(matches!(
        context.authenticate(&auth("alice", &[])),
        Err(ConnError::AccessDenied { .. })
    ));

    let results = context
        .execute_query(
            "create database runtime_db; use runtime_db; \
             create table items (id bigint primary key, name varchar(32)); \
             insert into items values (1, 'from-tikv'); \
             select * from items",
            true,
            &CancellationToken::new(),
        )
        .expect("execute real multi-statement SQL");
    assert_eq!(results.len(), 5);
    assert_eq!(results[3].state.affected_rows, 1);
    assert_eq!(
        results[4]
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );
    assert_eq!(results[4].columns[0].schema, "runtime_db");
    assert_eq!(results[4].columns[0].table, "items");
    assert_eq!(results[4].columns[0].org_table, "items");
    assert_eq!(results[4].columns[0].org_name, "id");
    assert_eq!(
        results[4].columns[0].column_type,
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert!(astersql_parser_mysql::r#type::HasPriKeyFlag(
        results[4].columns[0].flags as usize
    ));
    assert_eq!(
        results[4].columns[1].column_type,
        astersql_parser_mysql::r#type::TypeVarString
    );
    assert_eq!(
        results[4].rows,
        vec![vec![
            Value::Text("1".into()),
            Value::Text("from-tikv".into())
        ]]
    );
    assert_eq!(context.state().status & 0x0002, 0x0002);
    assert_eq!(context.last_statement(), "select * from items");

    let fields = context
        .field_list("items", "%")
        .expect("COM_FIELD_LIST uses canonical table metadata");
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );
    assert_eq!(fields[0].schema, "runtime_db");
    assert_eq!(fields[0].table, "items");
    assert_eq!(
        fields[0].column_type,
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert_eq!(
        fields[1].column_type,
        astersql_parser_mysql::r#type::TypeVarString
    );

    // 取消只作用于下一条命令；命中后应被消费，后续查询仍能正常执行。
    context.cancel();
    assert!(
        context
            .execute_query("select 1", false, &CancellationToken::new())
            .expect_err("cancel must reach the concrete session boundary")
            .to_string()
            .contains("interrupted")
    );
    assert_eq!(
        context
            .execute_query("select 2", false, &CancellationToken::new())
            .expect("cancellation applies to one command")[0]
            .rows,
        vec![vec![Value::Text("2".into())]]
    );

    assert!(matches!(
        context.execute_command(Command::StmtPrepare, b"select 1", &CancellationToken::new()),
        Err(ConnError::UnsupportedCommand(0x16))
    ));
    context.close().expect("close concrete context");
    assert!(
        context
            .execute_query("select 1", false, &CancellationToken::new())
            .is_err()
    );
}

/// canonical 连接域应直接委托 Domain 分配唯一连接 ID，并允许成对释放。
#[test]
fn canonical_connection_domain_delegates_id_allocation_and_release() {
    let driver = session_driver();
    let domain = CanonicalConnectionDomain::new(driver.domain().clone());
    let first = domain.next_connection_id();
    let second = domain.next_connection_id();
    assert_ne!(first, second);
    domain.release_connection_id(first);
    domain.release_connection_id(second);
}

#[test]
fn query_cancellation_completion_preserves_session_transaction() {
    let driver = session_driver();
    let context = driver.open_ctx(71001, 0, 45, "", None).unwrap();
    context.authenticate(&auth("root", &[])).unwrap();
    context
        .execute_query("begin", false, &CancellationToken::new())
        .unwrap();
    assert!(context.in_transaction());
    context.cancel();
    context.finish_query_cancellation();
    assert!(context.in_transaction());
    assert!(
        context
            .execute_query("select 1", false, &CancellationToken::new())
            .is_ok()
    );
    context
        .execute_query("rollback", false, &CancellationToken::new())
        .unwrap();
    context.close().unwrap();
}

#[test]
fn normal_ddl_plan_user_mdl_real_driver_and_domain_lifecycle() {
    use crate::server::{Server, ServerConfig};
    use astersql_infoschema_issyncer::{InfoSchemaCoordinator, JobMDL};
    struct RestoreMdl(bool);
    impl Drop for RestoreMdl {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::SetEnableMDL(self.0);
        }
    }
    let _restore_mdl = RestoreMdl(astersql_sessionctx_vardef::IsMDLEnabled());
    astersql_sessionctx_vardef::SetEnableMDL(true);
    let driver = Arc::new(session_driver());
    let domain = driver.domain().clone();
    let server = Server::new_test(
        ServerConfig::default(),
        Arc::new(crate::runtime::CanonicalServerDriver),
    );
    server
        .set_connection_runtime(
            driver.clone(),
            Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        )
        .unwrap();
    assert!(
        domain.schema_coordinator().is_some(),
        "driver installation wires Domain before any session is opened"
    );
    let (mut peer, socket) = tcp_pair();
    let connection = crate::conn::newClientConn(
        server.clone(),
        Box::new(TcpPacketIo::new(socket, 32 * 1024 * 1024).unwrap()),
        vec![7; 20],
        false,
    );
    let handshake = thread::spawn(move || {
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let size =
            usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
        let mut initial = vec![0; size];
        peer.read_exact(&mut initial).unwrap();
        assert_eq!(initial[0], 10);
        let capability = (1_u32 << 9) | (1 << 15) | (1 << 19);
        let mut response = capability.to_le_bytes().to_vec();
        response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
        response.push(45);
        response.extend_from_slice(&[0; 23]);
        response.extend_from_slice(b"root\0\0mysql_native_password\0");
        let len = response.len();
        peer.write_all(&[len as u8, (len >> 8) as u8, (len >> 16) as u8, 1])
            .unwrap();
        peer.write_all(&response).unwrap();
        peer
    });
    connection.handshake().unwrap();
    let _peer = handshake.join().unwrap();
    let context = connection.getCtx().unwrap().unwrap();
    let connection_id = connection.connection_id();
    let query = |sql: &str| {
        context
            .execute_query(sql, true, &CancellationToken::new())
            .unwrap()
    };
    query(
        "USE test; CREATE TABLE user_mdl_target (id INT PRIMARY KEY, payload VARCHAR(30)); INSERT INTO user_mdl_target VALUES (1, 'held')",
    );
    let table = domain.table_by_name("test", "user_mdl_target").unwrap();
    assert_eq!(table.Columns.len(), 2);
    // The registration API accepts non-restricted sessions too. Acquire MDL
    // through real SQL, while the production restricted pool is covered below
    // in normal_ddl_test and must preserve Go's restricted-SQL bypass.
    let internal_session = astersql_session::runtime::ConcreteSession::new(domain.clone());
    internal_session.execute("CREATE TABLE internal_mdl_target (id INT PRIMARY KEY, payload VARCHAR(30)); INSERT INTO internal_mdl_target VALUES (2, 'internal')").unwrap();
    let internal_table = domain.table_by_name("test", "internal_mdl_target").unwrap();
    let internal = Arc::new(astersql_domain_crossks::new_schema_coordinator());
    internal.store_internal_session(Arc::new(astersql_domain_crossks::RegisteredMDLSession {
        id: 9,
        mdl: internal_session.transaction_mdl(),
    }));
    let coordinator = astersql_session::runtime::normal_ddl_service::NormalSchemaCoordinator {
        domain: Arc::downgrade(&domain),
        internal: internal.clone(),
    };
    let version = domain.info_schema().SchemaMetaVersion();
    let jobs = || {
        std::collections::HashMap::from([
            (
                1,
                JobMDL {
                    Ver: version + 1,
                    TableIDs: [table.ID].into_iter().collect(),
                },
            ),
            (
                2,
                JobMDL {
                    Ver: version,
                    TableIDs: [table.ID].into_iter().collect(),
                },
            ),
            (
                3,
                JobMDL {
                    Ver: version + 1,
                    TableIDs: [table.ID + 1000].into_iter().collect(),
                },
            ),
            (
                4,
                JobMDL {
                    Ver: version + 1,
                    TableIDs: [internal_table.ID].into_iter().collect(),
                },
            ),
        ])
    };
    internal_session.execute("BEGIN").unwrap();
    let mut internal_rows = internal_session
        .execute("SELECT * FROM internal_mdl_target")
        .unwrap();
    assert_eq!(
        internal_rows[0].next_row().unwrap(),
        Some(vec!["2".into(), "internal".into()])
    );
    assert!(internal_rows[0].next_row().unwrap().is_none());
    internal_rows[0].close().unwrap();
    query("BEGIN");
    assert_eq!(
        query("SELECT * FROM user_mdl_target")[0].rows,
        vec![vec![Value::Text("1".into()), Value::Text("held".into())]]
    );
    for _ in 0..2 {
        let mut pending = jobs();
        coordinator.CheckOldRunningTxn(&mut pending);
        assert!(!pending.contains_key(&1) && !pending.contains_key(&4));
        assert!(pending.contains_key(&2) && pending.contains_key(&3));
    }
    query("COMMIT");
    let mut pending = jobs();
    coordinator.CheckOldRunningTxn(&mut pending);
    assert_eq!(pending.len(), 3);
    assert!(
        !pending.contains_key(&4),
        "user COMMIT must not release an internal transaction's MDL"
    );
    internal_session.execute("COMMIT").unwrap();
    let mut pending = jobs();
    coordinator.CheckOldRunningTxn(&mut pending);
    assert_eq!(pending.len(), 4);
    internal.delete_internal_session(9);
    drop(internal_session);
    query("BEGIN; SELECT * FROM user_mdl_target");
    let mdl = context.transaction_mdl().unwrap();
    assert!(server.unregister_connection(connection.connection_id()));
    let mut pending = jobs()
        .into_iter()
        .map(|(id, job)| {
            (
                id,
                Arc::new(astersql_session_sessmgr::mdldef::JobMDL {
                    ver: job.Ver,
                    table_ids: job.TableIDs,
                }),
            )
        })
        .collect();
    mdl.check_jobs(&mut pending);
    assert!(
        !pending.contains_key(&1),
        "unregister must not release a user's transaction lock"
    );
    server.register_connection(connection.clone()).unwrap();
    connection.Close().unwrap();
    connection.Close().unwrap();
    let mut pending = jobs()
        .into_iter()
        .map(|(id, job)| {
            (
                id,
                Arc::new(astersql_session_sessmgr::mdldef::JobMDL {
                    ver: job.Ver,
                    table_ids: job.TableIDs,
                }),
            )
        })
        .collect();
    mdl.check_jobs(&mut pending);
    assert_eq!(
        pending.len(),
        4,
        "closing the session rolls back its transaction"
    );
    assert!(
        !server.unregister_connection(connection_id),
        "Close must invoke connection cleanup"
    );
    server.close();
    server.close();
    let weak = Arc::downgrade(&server);
    drop(connection);
    drop(server);
    assert!(
        weak.upgrade().is_none(),
        "Domain and driver must weakly retain Server"
    );
    assert!(domain.schema_coordinator().is_none());
    domain.close();
}
