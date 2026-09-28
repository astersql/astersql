// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// Mock 连接单元测试：验证查询/分发输出缓冲与 Auth Socket 用户桩。

use super::mock_conn::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
/// 记录关闭状态的简易 MockSession。
struct Session {
    closed: Mutex<bool>,
}
impl MockSession for Session {
    fn handle_query(&self, sql: &str, output: &mut Vec<u8>) -> Result<(), Error> {
        output.extend_from_slice(sql.as_bytes());
        Ok(())
    }
    fn dispatch(&self, packet: &[u8], output: &mut Vec<u8>) -> Result<(), Error> {
        output.extend_from_slice(packet);
        Ok(())
    }
    fn close(&self) -> Result<(), Error> {
        *self.closed.lock().unwrap() = true;
        Ok(())
    }
    fn authenticate_root(&self) -> Result<(), Error> {
        Ok(())
    }
}
/// 始终返回同一 Session，并断言 collation==45。
struct Driver {
    session: Arc<Session>,
}
impl MockDriver for Driver {
    fn open(&self, _: u64, collation: u8) -> Result<Arc<dyn MockSession>, Error> {
        assert_eq!(collation, 45);
        Ok(self.session.clone())
    }
}

#[test]
/// 验证 HandleQuery/Dispatch 写入输出，GetOutput 像 Go 版一样轮换缓冲，Close 关闭会话。
fn mock_connection_dispatches_and_rotates_output_between_reads() {
    let session = Arc::new(Session::default());
    let server = CreateMockServer(Arc::new(Driver {
        session: session.clone(),
    }));
    let connection = CreateMockConn(&server).unwrap();
    connection.HandleQuery("select 1").unwrap();
    let query_output = connection.GetOutput();
    assert_eq!(&*query_output.lock().unwrap(), b"select 1");
    connection.Dispatch(b"ping").unwrap();
    let dispatch_output = connection.GetOutput();
    assert_eq!(&*query_output.lock().unwrap(), b"select 1");
    assert_eq!(&*dispatch_output.lock().unwrap(), b"ping");
    assert!(!Arc::ptr_eq(&query_output, &dispatch_output));
    connection.Close().unwrap();
    assert!(*session.closed.lock().unwrap());
}

#[test]
/// 验证注入 OS 用户名后可被 Clear 清除。
fn auth_socket_override_is_recoverable() {
    MockOSUserForAuthSocket("alice".into()).unwrap();
    ClearOSUserForAuthSocket().unwrap();
}
