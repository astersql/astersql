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
}

impl StatementRuntime for Runtime {
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
