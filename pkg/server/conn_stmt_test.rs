// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 预处理语句连接处理（conn_stmt）单元测试。
//
// 覆盖 COM_STMT_PREPARE / SEND_LONG_DATA / RESET / SET_OPTION / CLOSE，
// 以及语句文本渲染与畸形包拒绝；通过假 Runtime 隔离会话执行细节。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::conn_stmt::*;

/// 测试用会话运行时：记录关闭的语句 ID，并对固定 SQL 返回预设 prepare 结果。
#[derive(Default)]
struct Runtime {
    /// 被 close_statement 记录的语句 ID 序列。
    closed: Mutex<Vec<u32>>,
    max_packet: Mutex<Option<u64>>,
}

impl StatementRuntime for Runtime {
    fn max_allowed_packet(&self) -> u64 {
        self.max_packet.lock().unwrap().unwrap_or(1024)
    }
    fn prepare(&self, sql: &str) -> Result<(u32, usize, Vec<ColumnInfo>), Error> {
        assert_eq!(sql, "select ?, ?");
        Ok((
            7,
            2,
            vec![ColumnInfo {
                name: "result".into(),
                column_type: 253,
            }],
        ))
    }
    fn execute(&self, _: u32, _: &[BinaryParam]) -> Result<Option<Box<dyn ResultSet>>, Error> {
        Ok(None)
    }
    fn close_statement(&self, id: u32) -> Result<(), Error> {
        self.closed.lock().unwrap().push(id);
        Ok(())
    }
    fn statement_cache_text(&self, id: u32) -> Option<String> {
        (id == 7).then(|| "select ?, ?".into())
    }
    fn statement_cache_valid(&self, id: u32) -> bool {
        id == 7
    }
    fn cursor_ru_delta(&self, _: u32, _: usize) {}
    fn retry_after_statement_error(&self, _: &Error) -> Result<bool, Error> {
        Ok(false)
    }
    fn should_fallback_tiflash(&self, _: &Error) -> bool {
        false
    }
    fn set_tiflash_enabled(&self, _: bool) {}
    fn append_statement_warning(&self, _: Error) {}
}

/// 用给定 Runtime 构造空的 clientConn（能力位与输出缓冲清零）。
fn connection(runtime: Arc<Runtime>) -> clientConn {
    clientConn {
        capability: 0,
        statements: Default::default(),
        output: Vec::new(),
        runtime,
    }
}

/// Prepare 应注册语句并写出含 statement_id / params 的 Prepared 协议事件。
#[test]
fn prepare_registers_statement_and_protocol_metadata() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    let statement = cc.statements.get(&7).unwrap();
    assert_eq!(statement.num_params, 2);
    assert_eq!(statement.sql, "select ?, ?");
    assert_eq!(cc.output.len(), 1);
    assert!(matches!(
        cc.output[0],
        ProtocolEvent::Prepared {
            statement_id: 7,
            params: 2,
            ..
        }
    ));
}

/// SEND_LONG_DATA 按参数槽累计字节；RESET 清空绑定并回 OK。
#[test]
fn long_data_accumulates_and_reset_consumes_it() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    // 包布局：stmt_id(4) + param_id(2) + payload；两次追加到参数 1。
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 1, 0, b'a']).unwrap();
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 1, 0, b'b']).unwrap();
    assert_eq!(
        cc.statements[&7].bound_params[1].as_deref(),
        Some(&b"ab"[..])
    );
    handleStmtReset(&mut cc, &[7, 0, 0, 0]).unwrap();
    assert_eq!(cc.statements[&7].bound_params, vec![None, None]);
    assert_eq!(cc.output.last(), Some(&ProtocolEvent::Ok));
}

/// 语句渲染应对文本转义引号，并对 NULL 参数输出 NULL；无参版本只保留 SQL。
#[test]
fn statement_rendering_quotes_text_and_marks_null() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    cc.statements.get_mut(&7).unwrap().last_params = vec![
        BinaryParam {
            value: b"a'b".to_vec(),
            ..Default::default()
        },
        BinaryParam {
            is_null: true,
            ..Default::default()
        },
    ];
    assert_eq!(preparedStmt2String(&cc, 7), "select ?, ? ['a\\'b', NULL]");
    assert_eq!(preparedStmt2StringNoArgs(&cc, 7), "select ?, ?");
    assert_eq!(
        preparedStmtID2CachePreparedStmt(&cc, 7),
        (Some("select ?, ?".into()), false)
    );
}

/// SET_OPTION 按 MySQL 包规则开关 MULTI_STATEMENTS；CLOSE 移除语句并通知 Runtime。
#[test]
fn set_option_and_close_follow_mysql_packet_rules() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime.clone());
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    handleSetOption(&mut cc, &[0, 0]).unwrap();
    assert_ne!(cc.capability, 0);
    handleSetOption(&mut cc, &[1, 0]).unwrap();
    assert_eq!(cc.capability, 0);
    handleStmtClose(&mut cc, &[7, 0, 0, 0]).unwrap();
    assert!(!cc.statements.contains_key(&7));
    assert_eq!(*runtime.closed.lock().unwrap(), [7]);
}

/// 长度不足的 EXECUTE / FETCH / SEND_LONG_DATA 包应判为 MalformedPacket。
#[test]
fn malformed_statement_packets_are_rejected() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    assert_eq!(
        handleStmtExecute(&mut cc, &[0; 8]),
        Err(Error::MalformedPacket)
    );
    assert_eq!(
        handleStmtFetch(&mut cc, &[0; 7]),
        Err(Error::MalformedPacket)
    );
    assert_eq!(
        handleStmtSendLongData(&mut cc, &[0; 5]),
        Err(Error::MalformedPacket)
    );
}

#[test]
fn execute_parse_error_still_resets_previous_cursor() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    install_statement(&mut cc, 9);
    let closed = Arc::new(AtomicBool::new(false));
    {
        let statement = cc.statements.get_mut(&9).unwrap();
        statement.num_params = 1;
        statement.bound_params = vec![None];
        statement.cursor = Some(Box::new(FailingResultSet {
            closed: Arc::clone(&closed),
            lazy: true,
        }));
        statement.cursor_active = true;
    }

    // stmt_id + cursor flag + iteration count + null bitmap + new-types flag
    // + MYSQL_TYPE_LONG; the required four-byte value is deliberately absent.
    let packet = [9, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 3, 0];
    assert_eq!(
        handleStmtExecute(&mut cc, &packet),
        Err(Error::MalformedPacket)
    );
    assert!(!cc.statements[&9].cursor_active);
    assert!(cc.statements[&9].cursor.is_none());
    assert!(closed.load(Ordering::SeqCst));
}

struct FailingResultSet {
    closed: Arc<AtomicBool>,
    lazy: bool,
}

impl ResultSet for FailingResultSet {
    fn columns(&self) -> &[ColumnInfo] {
        &[]
    }

    fn next(&mut self) -> Result<Option<Vec<Vec<u8>>>, Error> {
        Err(Error::Runtime("fetch failed".into()))
    }

    fn close(&mut self) -> Result<(), Error> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn exhausted(&self) -> bool {
        false
    }

    fn supports_lazy_cursor(&self) -> bool {
        self.lazy
    }
}

fn install_statement(cc: &mut clientConn, id: u32) {
    cc.statements.insert(
        id,
        PreparedStatement {
            id,
            sql: "select 1".into(),
            num_params: 0,
            columns: Vec::new(),
            bound_params: Vec::new(),
            bound_params_too_large: false,
            max_allowed_packet: 64 << 20,
            params_type: Vec::new(),
            last_params: Vec::new(),
            cursor: None,
            cursor_active: false,
            protocol_cursor: None,
        },
    );
}

#[test]
fn fetch_error_resets_and_closes_active_cursor() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    install_statement(&mut cc, 9);
    let closed = Arc::new(AtomicBool::new(false));
    executeWithCursor(
        &mut cc,
        9,
        Box::new(FailingResultSet {
            closed: Arc::clone(&closed),
            lazy: true,
        }),
    )
    .unwrap();

    assert_eq!(
        handleStmtFetch(&mut cc, &[9, 0, 0, 0, 1, 0, 0, 0]),
        Err(Error::Runtime("fetch failed".into()))
    );
    assert!(!cc.statements[&9].cursor_active);
    assert!(cc.statements[&9].cursor.is_none());
    assert!(closed.load(Ordering::SeqCst));
}

#[test]
fn eager_materialization_error_closes_source() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    install_statement(&mut cc, 9);
    let closed = Arc::new(AtomicBool::new(false));
    assert_eq!(
        executeWithCursor(
            &mut cc,
            9,
            Box::new(FailingResultSet {
                closed: Arc::clone(&closed),
                lazy: false,
            }),
        ),
        Err(Error::Runtime("fetch failed".into()))
    );
    assert!(closed.load(Ordering::SeqCst));
}

#[test]
fn non_cursor_read_error_closes_source() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    install_statement(&mut cc, 9);
    let closed = Arc::new(AtomicBool::new(false));
    assert_eq!(
        executePreparedStmtAndWriteResult(
            &mut cc,
            9,
            Some(Box::new(FailingResultSet {
                closed: Arc::clone(&closed),
                lazy: false,
            })),
            false,
        ),
        Err(Error::Runtime("fetch failed".into()))
    );
    assert!(closed.load(Ordering::SeqCst));
}

#[test]
fn fetch_requires_exact_packet_length() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    assert_eq!(
        handleStmtFetch(&mut cc, &[0; 9]),
        Err(Error::MalformedPacket)
    );
}

#[test]
fn fetch_on_inactive_cursor_does_not_reset_bound_parameters() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    install_statement(&mut cc, 9);
    cc.statements.get_mut(&9).unwrap().bound_params = vec![Some(b"keep".to_vec())];

    assert_eq!(
        handleStmtFetch(&mut cc, &[9, 0, 0, 0, 1, 0, 0, 0]),
        Err(Error::WrongArguments("stmt_fetch"))
    );
    assert_eq!(cc.statements[&9].bound_params, vec![Some(b"keep".to_vec())]);
}

// These regressions use the canonical SQL session and the actual TCP PacketIO.
// Only the network write boundary is replaced to reproduce a delayed bad connection.
#[derive(Default)]
struct ResponseWriteFault {
    fail_on: usize,
    writes: usize,
    packets: Vec<Vec<u8>>,
    flush_delay: std::time::Duration,
    fail_flush: bool,
}
struct FaultPacket {
    inner: crate::runtime::TcpPacketIo,
    fault: Arc<Mutex<ResponseWriteFault>>,
}
impl crate::conn::PacketIo for FaultPacket {
    fn read_packet(&mut self) -> crate::conn::ConnResult<Vec<u8>> {
        self.inner.read_packet()
    }
    fn write_packet(&mut self, data: &[u8]) -> crate::conn::ConnResult<()> {
        let mut fault = self.fault.lock().unwrap();
        fault.writes += 1;
        fault.packets.push(data.to_vec());
        if fault.fail_on != 0 && fault.writes == fault.fail_on {
            std::thread::sleep(std::time::Duration::from_millis(50));
            return Err(crate::conn::ConnError::Io("bad connection".into()));
        }
        drop(fault);
        self.inner.write_packet(data)
    }
    fn flush(&mut self) -> crate::conn::ConnResult<()> {
        let f = self.fault.lock().unwrap();
        std::thread::sleep(f.flush_delay);
        if f.fail_flush {
            return Err(crate::conn::ConnError::Io("bad flush".into()));
        }
        drop(f);
        self.inner.flush()
    }
    fn reset_sequence(&mut self) {
        self.inner.reset_sequence()
    }
    fn set_read_timeout(&mut self, timeout: std::time::Duration) -> crate::conn::ConnResult<()> {
        self.inner.set_read_timeout(timeout)
    }
    fn set_compression(&mut self, algorithm: crate::conn::CompressionAlgorithm, level: i32) {
        self.inner.set_compression(algorithm, level)
    }
    fn upgrade_to_tls(&mut self) -> crate::conn::ConnResult<crate::conn::TlsState> {
        self.inner.upgrade_to_tls()
    }
    fn peer_addr(&self) -> crate::conn::ConnResult<(String, String)> {
        self.inner.peer_addr()
    }
    fn local_addr(&self) -> crate::conn::ConnResult<(String, String)> {
        self.inner.local_addr()
    }
    fn connection_alive(&self) -> bool {
        self.inner.connection_alive()
    }
    fn close(&mut self) -> crate::conn::ConnResult<()> {
        self.inner.close()
    }
}
fn sql_response_connection() -> (
    Arc<crate::conn::ClientConn>,
    std::net::TcpStream,
    Arc<Mutex<ResponseWriteFault>>,
) {
    sql_response_connection_with_capability(0)
}
fn sql_response_connection_with_capability(
    additional_capability: u32,
) -> (
    Arc<crate::conn::ClientConn>,
    std::net::TcpStream,
    Arc<Mutex<ResponseWriteFault>>,
) {
    use std::io::{Read, Write};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(crate::runtime::ConcreteSessionDriver::new_for_test(
        domain.clone(),
        crate::runtime::BootstrapAuthMode::InsecureRootOnly,
    ));
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig::default(),
        Arc::new(crate::runtime::CanonicalServerDriver),
    );
    server
        .set_connection_runtime(
            driver,
            Arc::new(crate::runtime::CanonicalConnectionDomain::new(domain)),
        )
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let fault = Arc::new(Mutex::new(ResponseWriteFault::default()));
    let connection = crate::conn::newClientConn(
        server,
        Box::new(FaultPacket {
            inner: crate::runtime::TcpPacketIo::new(socket, 32 * 1024 * 1024).unwrap(),
            fault: fault.clone(),
        }),
        vec![7; 20],
        false,
    );
    let handshake = std::thread::spawn(move || {
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let size =
            usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
        let mut data = vec![0; size];
        peer.read_exact(&mut data).unwrap();
        let capability = (1_u32 << 9) | (1 << 15) | (1 << 19) | additional_capability;
        let mut response = capability.to_le_bytes().to_vec();
        response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
        response.push(45);
        response.extend_from_slice(&[0; 23]);
        response.extend_from_slice(b"root\0\0mysql_native_password\0");
        peer.write_all(&[response.len() as u8, 0, 0, 1]).unwrap();
        peer.write_all(&response).unwrap();
        peer
    });
    connection.handshake().unwrap();
    let peer = handshake.join().unwrap();
    for sql in [
        "create table response_t(a int)",
        "insert into response_t values (1), (2)",
    ] {
        connection
            .dispatch(&[&[3], sql.as_bytes()].concat())
            .unwrap();
    }
    (connection, peer, fault)
}
#[test]
fn text_failed_row_write_is_accounted() {
    let (cc, _peer, fault) = sql_response_connection();
    // Match the source Go regression's single-row fixture exactly.
    cc.dispatch(b"\x03delete from response_t where a=2")
        .unwrap();
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
        f.fail_on = 4;
    }
    assert_eq!(
        cc.dispatch(b"\x03select * from response_t"),
        Err(crate::conn::ConnError::Io("bad connection".into()))
    );
    let f = fault.lock().unwrap();
    assert_eq!(f.packets[0], [1]);
    assert_eq!(f.packets[2][0], 0xfe);
    assert_eq!(f.packets[3], [1, b'1']);
    assert!(
        cc.getCtx()
            .unwrap()
            .unwrap()
            .protocol_write_duration_for_test()
            >= std::time::Duration::from_millis(50)
    );
    drop(f);
    cc.Close().unwrap();
}
#[test]
fn cursor_failed_row_write_is_accounted() {
    use crate::conn::{CancellationToken, Command};
    let (cc, _peer, fault) = sql_response_connection();
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
    }
    cc.handleStmt(
        Command::StmtPrepare,
        b"select * from response_t",
        &CancellationToken::new(),
    )
    .unwrap();
    let id = u32::from_le_bytes(fault.lock().unwrap().packets[0][1..5].try_into().unwrap());
    let mut execute = id.to_le_bytes().to_vec();
    execute.extend_from_slice(&[1, 1, 0, 0, 0]);
    cc.handleStmt(Command::StmtExecute, &execute, &CancellationToken::new())
        .unwrap();
    let before = cc.cursor_write_duration_for_test(id);
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
        f.fail_on = 1;
    }
    let fetch = [id.to_le_bytes(), 1_u32.to_le_bytes()].concat();
    assert_eq!(
        cc.handleStmt(Command::StmtFetch, &fetch, &CancellationToken::new()),
        Err(crate::conn::ConnError::Io("bad connection".into()))
    );
    assert!(cc.cursor_write_duration_for_test(id) >= before + std::time::Duration::from_millis(50));
    cc.Close().unwrap();
}

fn prepare_response_cursor(cc: &crate::conn::ClientConn, fault: &Mutex<ResponseWriteFault>) -> u32 {
    use crate::conn::{CancellationToken, Command};
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
        f.fail_on = 0;
    }
    cc.handleStmt(
        Command::StmtPrepare,
        b"select * from response_t",
        &CancellationToken::new(),
    )
    .unwrap();
    let id = u32::from_le_bytes(fault.lock().unwrap().packets[0][1..5].try_into().unwrap());
    let mut execute = id.to_le_bytes().to_vec();
    execute.extend_from_slice(&[1, 1, 0, 0, 0]);
    cc.handleStmt(Command::StmtExecute, &execute, &CancellationToken::new())
        .unwrap();
    id
}
fn fetch_response(
    cc: &crate::conn::ClientConn,
    id: u32,
    count: u32,
) -> crate::conn::ConnResult<()> {
    cc.handleStmt(
        crate::conn::Command::StmtFetch,
        &[id.to_le_bytes(), count.to_le_bytes()].concat(),
        &crate::conn::CancellationToken::new(),
    )
}
#[test]
fn text_metadata_and_eof_failures_are_accounted() {
    let (cc, _peer, fault) = sql_response_connection();
    for at in [1, 2, 3, 6] {
        {
            let mut f = fault.lock().unwrap();
            f.writes = 0;
            f.packets.clear();
            f.fail_on = at;
        }
        assert_eq!(
            cc.dispatch(b"\x03select * from response_t"),
            Err(crate::conn::ConnError::Io("bad connection".into()))
        );
        assert_eq!(fault.lock().unwrap().writes, at);
        assert!(
            cc.getCtx()
                .unwrap()
                .unwrap()
                .protocol_write_duration_for_test()
                >= std::time::Duration::from_millis(50)
        );
    }
    cc.Close().unwrap();
}
#[test]
fn query_flush_failure_is_accounted() {
    let (cc, _peer, fault) = sql_response_connection();
    {
        let mut f = fault.lock().unwrap();
        f.flush_delay = std::time::Duration::from_millis(50);
        f.fail_flush = true;
    }
    assert_eq!(
        cc.dispatch(b"\x03select * from response_t"),
        Err(crate::conn::ConnError::Io("bad flush".into()))
    );
    assert!(
        cc.getCtx()
            .unwrap()
            .unwrap()
            .protocol_write_duration_for_test()
            >= std::time::Duration::from_millis(50)
    );
    cc.Close().unwrap();
}
#[test]
fn next_and_finish_are_excluded_and_errors_preserved() {
    use std::time::{Duration, Instant};
    let (cc, _peer, fault) = sql_response_connection();
    let ctx = cc.getCtx().unwrap().unwrap();
    for (operation, at, fail) in [
        ("Next", 1, false),
        ("Next", 2, false),
        ("Finish", 1, false),
        ("Next", 1, true),
        ("Next", 2, true),
        ("Finish", 1, true),
    ] {
        ctx.result_fault_for_test(operation, at, Duration::from_millis(300), fail);
        {
            let mut f = fault.lock().unwrap();
            f.writes = 0;
            f.packets.clear();
        }
        let start = Instant::now();
        let result = cc.dispatch(b"\x03select * from response_t");
        let elapsed = start.elapsed();
        let measured = ctx.protocol_write_duration_for_test();
        assert!(
            elapsed.saturating_sub(measured) >= Duration::from_millis(250),
            "{operation} elapsed {elapsed:?}, measured {measured:?}"
        );
        if fail {
            assert_eq!(
                result,
                Err(crate::conn::ConnError::Session(format!(
                    "injected {operation} failure"
                )))
            );
            if operation == "Next" && at == 1 {
                assert!(fault.lock().unwrap().packets.is_empty());
            }
        } else {
            result.unwrap();
        }
        let events = ctx.result_events_for_test();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "Close")
                .count(),
            1,
            "{events:?}"
        );
        if !fail {
            assert_eq!(events, ["Next", "Next", "Finish", "Close"]);
        }
    }
    cc.Close().unwrap();
}
#[test]
fn cursor_iteration_is_timed_and_notification_excluded() {
    use std::time::{Duration, Instant};
    let (cc, _peer, fault) = sql_response_connection();
    let ctx = cc.getCtx().unwrap().unwrap();
    let id = prepare_response_cursor(&cc, &fault);
    ctx.result_fault_for_test("Next", 1, Duration::from_millis(50), false);
    let before = cc.cursor_write_duration_for_test(id);
    fetch_response(&cc, id, 1).unwrap();
    assert!(cc.cursor_write_duration_for_test(id) >= before + Duration::from_millis(50));
    assert_eq!(ctx.result_events_for_test(), ["Next", "FetchReturned"]);
    {
        let f = fault.lock().unwrap();
        let eof = f.packets.last().unwrap();
        let status = u16::from_le_bytes(eof[3..5].try_into().unwrap());
        assert_ne!(status & 0x40, 0);
        assert_eq!(status & 0x80, 0);
    }
    ctx.result_fault_for_test("FetchReturned", 1, Duration::from_millis(300), false);
    let before = cc.cursor_write_duration_for_test(id);
    let started = Instant::now();
    fetch_response(&cc, id, 0).unwrap();
    let added = cc.cursor_write_duration_for_test(id).saturating_sub(before);
    assert!(started.elapsed().saturating_sub(added) >= Duration::from_millis(250));
    assert_eq!(ctx.result_events_for_test(), ["FetchReturned"]);
    ctx.result_fault_for_test("none", 1, Duration::ZERO, false);
    fetch_response(&cc, id, 5).unwrap();
    let packets = &fault.lock().unwrap().packets;
    let eof = packets.last().unwrap();
    let status = u16::from_le_bytes(eof[3..5].try_into().unwrap());
    assert_eq!(status & 0x40, 0);
    assert_ne!(status & 0x80, 0);
    assert_eq!(
        ctx.result_events_for_test(),
        ["Next", "FetchReturned", "Close"]
    );
    assert!(ctx.protocol_write_duration_for_test() >= before);
    cc.Close().unwrap();
}
#[test]
fn cursor_iterator_error_is_accounted() {
    use std::time::Duration;
    let (cc, _peer, fault) = sql_response_connection();
    let ctx = cc.getCtx().unwrap().unwrap();
    for (at, count) in [(1, 1), (2, 2)] {
        let id = prepare_response_cursor(&cc, &fault);
        ctx.result_fault_for_test("Next", at, Duration::from_millis(50), true);
        let before = cc.cursor_write_duration_for_test(id);
        fault.lock().unwrap().packets.clear();
        assert_eq!(
            fetch_response(&cc, id, count),
            Err(crate::conn::ConnError::Session(
                "injected Next failure".into()
            ))
        );
        assert!(cc.cursor_write_duration_for_test(id) >= before + Duration::from_millis(50));
        let mut expected = vec!["Next"; at];
        assert_eq!(ctx.result_events_for_test(), expected);
        let f = fault.lock().unwrap();
        assert_eq!(f.packets.len(), if at == 1 { 0 } else { 2 });
        if at == 2 {
            assert_eq!(f.packets[0][0], 0);
            assert_eq!(f.packets[1][0], 0);
        }
        drop(f);
        cc.handleStmt(
            crate::conn::Command::StmtReset,
            &id.to_le_bytes(),
            &crate::conn::CancellationToken::new(),
        )
        .unwrap();
        expected.push("Close");
        assert_eq!(ctx.result_events_for_test(), expected);
    }
    cc.Close().unwrap();
}

#[test]
fn cursor_eof_and_flush_errors_are_accounted() {
    use std::time::Duration;
    let (cc, _peer, fault) = sql_response_connection();
    let id = prepare_response_cursor(&cc, &fault);
    let before = cc.cursor_write_duration_for_test(id);
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
        f.fail_on = 2;
    }
    assert_eq!(
        fetch_response(&cc, id, 1),
        Err(crate::conn::ConnError::Io("bad connection".into()))
    );
    assert!(cc.cursor_write_duration_for_test(id) >= before + Duration::from_millis(50));
    let before = cc.cursor_write_duration_for_test(id);
    {
        let mut f = fault.lock().unwrap();
        f.fail_on = 0;
        f.flush_delay = Duration::from_millis(50);
        f.fail_flush = true;
    }
    assert_eq!(
        fetch_response(&cc, id, 0),
        Err(crate::conn::ConnError::Io("bad flush".into()))
    );
    assert!(cc.cursor_write_duration_for_test(id) >= before + Duration::from_millis(50));
    cc.Close().unwrap();
}
#[test]
fn binary_query_and_cursor_metadata_errors_are_accounted() {
    use crate::conn::{CancellationToken, Command};
    let (cc, _peer, fault) = sql_response_connection();
    {
        let mut f = fault.lock().unwrap();
        f.packets.clear();
    }
    cc.handleStmt(
        Command::StmtPrepare,
        b"select * from response_t",
        &CancellationToken::new(),
    )
    .unwrap();
    let id = u32::from_le_bytes(fault.lock().unwrap().packets[0][1..5].try_into().unwrap());
    for cursor in [0, 1] {
        for at in if cursor == 0 {
            vec![1, 3, 4, 6]
        } else {
            vec![1, 2, 3]
        } {
            {
                let mut f = fault.lock().unwrap();
                f.writes = 0;
                f.packets.clear();
                f.fail_on = at;
            }
            let mut execute = id.to_le_bytes().to_vec();
            execute.extend_from_slice(&[cursor, 1, 0, 0, 0]);
            assert_eq!(
                cc.handleStmt(Command::StmtExecute, &execute, &CancellationToken::new()),
                Err(crate::conn::ConnError::Io("bad connection".into()))
            );
            assert!(
                cc.getCtx()
                    .unwrap()
                    .unwrap()
                    .protocol_write_duration_for_test()
                    >= std::time::Duration::from_millis(50)
            );
        }
    }
    cc.Close().unwrap();
}

#[test]
fn multichunk_next_failure_preserves_written_rows() {
    use std::time::Duration;
    let (cc, _peer, fault) = sql_response_connection();
    let values = (3..=1030)
        .map(|n| format!("({n})"))
        .collect::<Vec<_>>()
        .join(",");
    cc.dispatch(format!("\x03insert into response_t values {values}").as_bytes())
        .unwrap();
    let ctx = cc.getCtx().unwrap().unwrap();
    for fail in [true, false] {
        ctx.result_fault_for_test("Next", 2, Duration::ZERO, fail);
        {
            let mut f = fault.lock().unwrap();
            f.writes = 0;
            f.packets.clear();
        }
        let result = cc.dispatch(b"\x03select * from response_t order by a");
        let f = fault.lock().unwrap();
        if fail {
            assert_eq!(
                result,
                Err(crate::conn::ConnError::Session(
                    "injected Next failure".into()
                ))
            );
            assert_eq!(f.packets.len(), 1027); // metadata plus the first real 1024-row chunk
            assert_eq!(f.packets[3], [1, b'1']);
            assert_eq!(f.packets[1026], [4, b'1', b'0', b'2', b'4']);
            assert_eq!(ctx.result_events_for_test(), ["Next", "Next", "Close"]);
        } else {
            result.unwrap();
            assert_eq!(f.packets.len(), 1034);
            assert_eq!(f.packets[1032], [4, b'1', b'0', b'3', b'0']);
            assert_eq!(f.packets[1033][0], 0xfe);
            assert_eq!(
                ctx.result_events_for_test(),
                ["Next", "Next", "Next", "Finish", "Close"]
            );
        }
    }
    cc.Close().unwrap();
}
#[test]
fn deprecated_eof_and_absent_details_preserve_wire_rows() {
    let (cc, _peer, fault) = sql_response_connection_with_capability(1 << 24);
    {
        let mut f = fault.lock().unwrap();
        f.packets.clear();
        f.writes = 0;
    }
    let ctx = cc.getCtx().unwrap().unwrap();
    ctx.result_fault_for_test("none", 1, std::time::Duration::ZERO, false);
    let mut results = ctx
        .execute_query_streaming(
            "select * from response_t",
            false,
            &crate::conn::CancellationToken::new(),
        )
        .unwrap();
    let mut result = results.remove(0);
    // context.Background-equivalent: there is no statement detail to update.
    result.response_lifecycle.take();
    cc.writeResultSet(&result).unwrap();
    cc.flush().unwrap();
    let f = fault.lock().unwrap();
    assert_eq!(f.packets.len(), 5);
    assert_eq!(f.packets[2], [1, b'1']);
    assert_eq!(f.packets[3], [1, b'2']);
    assert_eq!(f.packets[4][0], 0xfe);
    assert!(f.packets[4].len() >= 7);
    assert_eq!(
        ctx.protocol_write_duration_for_test(),
        std::time::Duration::ZERO
    );
    assert_eq!(
        ctx.result_events_for_test(),
        ["Next", "Next", "Finish", "Close"]
    );
    drop(f);
    cc.Close().unwrap();
}
#[test]
fn binary_encoding_failure_is_accounted() {
    use std::time::Duration;
    let (cc, _peer, fault) = sql_response_connection();
    let id = prepare_response_cursor(&cc, &fault);
    let before = cc.cursor_write_duration_for_test(id);
    let error = crate::conn::ConnError::Session("injected row encoding failure".into());
    cc.fail_binary_encoding_for_test(Duration::from_millis(50), error.clone());
    assert_eq!(fetch_response(&cc, id, 1), Err(error.clone()));
    assert!(cc.cursor_write_duration_for_test(id) >= before + Duration::from_millis(50));
    cc.handleStmt(
        crate::conn::Command::StmtReset,
        &id.to_le_bytes(),
        &crate::conn::CancellationToken::new(),
    )
    .unwrap();
    cc.fail_binary_encoding_for_test(Duration::from_millis(50), error.clone());
    let mut execute = id.to_le_bytes().to_vec();
    execute.extend_from_slice(&[0, 1, 0, 0, 0]);
    assert_eq!(
        cc.handleStmt(
            crate::conn::Command::StmtExecute,
            &execute,
            &crate::conn::CancellationToken::new()
        ),
        Err(error)
    );
    assert!(
        cc.getCtx()
            .unwrap()
            .unwrap()
            .protocol_write_duration_for_test()
            >= Duration::from_millis(50)
    );
    cc.Close().unwrap();
}

#[test]
fn empty_results_and_successful_flush_are_accounted() {
    use std::time::Duration;
    let (cc, _peer, fault) = sql_response_connection();
    let ctx = cc.getCtx().unwrap().unwrap();
    ctx.result_fault_for_test("none", 1, Duration::ZERO, false);
    {
        let mut f = fault.lock().unwrap();
        f.writes = 0;
        f.packets.clear();
        f.flush_delay = Duration::from_millis(50);
    }
    cc.dispatch(b"\x03select * from response_t where a=100")
        .unwrap();
    assert!(ctx.protocol_write_duration_for_test() >= Duration::from_millis(50));
    assert_eq!(ctx.result_events_for_test(), ["Next", "Finish", "Close"]);
    {
        let f = fault.lock().unwrap();
        assert_eq!(f.packets.len(), 4);
        assert_eq!(f.packets[2][0], 0xfe);
        assert_eq!(f.packets[3][0], 0xfe);
    }
    {
        let mut f = fault.lock().unwrap();
        f.packets.clear();
    }
    cc.handleStmt(
        crate::conn::Command::StmtPrepare,
        b"select * from response_t where a=100",
        &crate::conn::CancellationToken::new(),
    )
    .unwrap();
    let id = u32::from_le_bytes(fault.lock().unwrap().packets[0][1..5].try_into().unwrap());
    let mut execute = id.to_le_bytes().to_vec();
    execute.extend_from_slice(&[1, 1, 0, 0, 0]);
    cc.handleStmt(
        crate::conn::Command::StmtExecute,
        &execute,
        &crate::conn::CancellationToken::new(),
    )
    .unwrap();
    ctx.result_fault_for_test("none", 1, Duration::ZERO, false);
    {
        let mut f = fault.lock().unwrap();
        f.packets.clear();
    }
    fetch_response(&cc, id, 5).unwrap();
    let f = fault.lock().unwrap();
    assert_eq!(f.packets.len(), 1);
    let status = u16::from_le_bytes(f.packets[0][3..5].try_into().unwrap());
    assert_eq!(status & 0x40, 0);
    assert_ne!(status & 0x80, 0);
    assert_eq!(
        ctx.result_events_for_test(),
        ["Next", "FetchReturned", "Close"]
    );
    assert!(fetch_response(&cc, id, 1).is_err());
    drop(f);
    cc.Close().unwrap();
}
#[test]
fn worker_releases_domain_when_context_is_dropped() {
    use crate::conn::SessionDriver;
    use std::time::{Duration, Instant};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = crate::runtime::ConcreteSessionDriver::new_for_test(
        domain.clone(),
        crate::runtime::BootstrapAuthMode::InsecureRootOnly,
    );
    let before = Arc::strong_count(&domain);
    let context = driver.open_ctx(7761792, 0, 45, "test", None).unwrap();
    let results = context
        .execute_query_streaming("select 1", false, &crate::conn::CancellationToken::new())
        .unwrap();
    assert_eq!(
        results[0]
            .result_set
            .as_ref()
            .unwrap()
            .next_chunk()
            .unwrap(),
        [vec![crate::conn::Value::Text("1".into())]]
    );
    drop(results);
    drop(context);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Arc::strong_count(&domain) > before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        Arc::strong_count(&domain),
        before,
        "session worker retains Domain after all response/context owners are gone"
    );
}

#[test]
fn long_data_packet_limit_defers_error_and_resets_on_execute() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime);
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    let limit = 1024;
    for parameter in [0u16, 1] {
        let mut packet = 7u32.to_le_bytes().to_vec();
        packet.extend_from_slice(&parameter.to_le_bytes());
        packet.resize(6 + limit, b'a');
        handleStmtSendLongData(&mut cc, &packet).unwrap();
    }
    assert!(!cc.statements[&7].bound_params_too_large);
    assert_eq!(
        cc.statements[&7].bound_params[1].as_ref().unwrap().len(),
        limit
    );
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 0, 0, b'c']).unwrap();
    assert_eq!(
        cc.statements[&7].bound_params[0].as_ref().unwrap().len(),
        limit
    );
    let error = handleStmtExecute(&mut cc, &[7, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0]).unwrap_err();
    assert!(error.to_string().contains("max_allowed_packet"));
    assert_eq!(cc.statements[&7].bound_params, vec![None, None]);
}

#[test]
fn empty_long_data_replaces_previous_bytes() {
    let mut cc = connection(Arc::new(Runtime::default()));
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 0, 0, b'a']).unwrap();
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(cc.statements[&7].bound_params[0], Some(Vec::new()));
}

#[test]
fn long_data_rechecks_current_limit_and_reset_clears_rejection() {
    let runtime = Arc::new(Runtime::default());
    let mut cc = connection(runtime.clone());
    HandleStmtPrepare(&mut cc, "select ?, ?").unwrap();
    let mut packet = vec![7, 0, 0, 0, 0, 0];
    packet.resize(1030, b'a');
    handleStmtSendLongData(&mut cc, &packet).unwrap();
    *runtime.max_packet.lock().unwrap() = Some(512);
    assert_eq!(
        handleStmtExecute(&mut cc, &[7, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
        Err(Error::NetPacketTooLarge)
    );
    assert_eq!(cc.statements[&7].bound_params, vec![None, None]);
    handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 0, 0, b'a']).unwrap();
    assert!(!cc.statements[&7].bound_params_too_large);
    assert_eq!(
        handleStmtSendLongData(&mut cc, &[7, 0, 0, 0, 2, 0]),
        Err(Error::WrongArguments("stmt_send_longdata"))
    );
    packet.resize(519, b'a');
    handleStmtSendLongData(&mut cc, &packet).unwrap();
    assert!(cc.statements[&7].bound_params_too_large);
    handleStmtReset(&mut cc, &[7, 0, 0, 0]).unwrap();
    assert!(!cc.statements[&7].bound_params_too_large);
}
