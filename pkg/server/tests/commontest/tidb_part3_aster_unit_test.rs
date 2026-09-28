// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TiDB server 公共协议回归测试（part3）：扩展连接事件与 mock 会话生命周期。
//
// 覆盖 ExtensionListeners 在 Connected/Handshake/Disconnected 等事件上携带的
// 连接身份与会话别名；以及批量关闭 mock 连接时会话必须被释放。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use astersql_server::extension::{
    ClientConn, ConnEventInfo, ConnEventTp, ConnectionInfo, Error as ExtensionError,
    ExtensionListeners, SessionVars, StmtEventTp, onExtensionConnEvent, stmtEventInfo,
};
use astersql_server::mock_conn::{
    CreateMockConn, CreateMockServer, Error, MockDriver, MockSession,
};

/// 记录连接事件序列的 ExtensionListeners 实现（本用例禁止语句事件）。
#[derive(Default)]
struct EventLog(Mutex<Vec<(ConnEventTp, ConnEventInfo)>>);

impl ExtensionListeners for EventLog {
    /// 本测试只关心连接事件，声明无语句事件监听器。
    fn has_stmt_event_listeners(&self) -> bool {
        false
    }

    /// 追加一条连接事件及其快照信息。
    fn on_connection_event(&self, event: ConnEventTp, info: ConnEventInfo) {
        self.0
            .lock()
            .expect("event log lock poisoned")
            .push((event, info));
    }

    /// 连接事件用例不应触发语句事件。
    fn on_stmt_event(&self, _event: StmtEventTp, _info: stmtEventInfo) {
        panic!("connection-event test must not emit statement events");
    }
}

/// 验证握手接受/断开/拒绝路径保留 connection_id、别名、角色与错误信息。
#[test]
fn extension_connection_events_preserve_handshake_identity_and_disconnect_state() {
    let log = Arc::new(EventLog::default());
    let listener: Arc<dyn ExtensionListeners> = log.clone();
    let transport_info = ConnectionInfo {
        connection_id: 42,
        client_host: "127.0.0.1".to_owned(),
    };
    let mut connection = ClientConn {
        connection_info: transport_info.clone(),
        session_vars: None,
        extensions: Some(listener),
    };

    // Connected：尚无 session_vars，身份仅来自传输层 ConnectionInfo。
    onExtensionConnEvent(&connection, ConnEventTp::Connected, None);

    // HandshakeAccepted：挂上 session_vars，别名与角色仍为空。
    connection.session_vars = Some(SessionVars {
        connection_info: Some(transport_info.clone()),
        session_alias: String::new(),
        active_roles: Vec::new(),
        ..SessionVars::default()
    });
    onExtensionConnEvent(&connection, ConnEventTp::HandshakeAccepted, None);

    // Disconnected：快照应带上会话别名与激活角色。
    let variables = connection.session_vars.as_mut().expect("session variables");
    variables.session_alias = "alias123".to_owned();
    variables.active_roles = vec!["r1@%".to_owned()];
    onExtensionConnEvent(&connection, ConnEventTp::Disconnected, None);

    // HandshakeRejected：携带鉴权失败错误，连接 id 可与先前不同。
    connection.session_vars = Some(SessionVars {
        connection_info: Some(ConnectionInfo {
            connection_id: 43,
            client_host: "127.0.0.1".to_owned(),
        }),
        ..SessionVars::default()
    });
    onExtensionConnEvent(
        &connection,
        ConnEventTp::HandshakeRejected,
        Some(ExtensionError(
            "access denied for noexist@127.0.0.1".to_owned(),
        )),
    );

    let events = log.0.lock().expect("event log lock poisoned");
    assert_eq!(
        events.iter().map(|(event, _)| *event).collect::<Vec<_>>(),
        vec![
            ConnEventTp::Connected,
            ConnEventTp::HandshakeAccepted,
            ConnEventTp::Disconnected,
            ConnEventTp::HandshakeRejected,
        ]
    );
    assert_eq!(events[0].1.connection_info, transport_info);
    assert!(events[0].1.session_alias.is_empty());
    assert!(events[1].1.active_roles.is_empty());
    assert_eq!(events[2].1.session_alias, "alias123");
    assert_eq!(events[2].1.active_roles, ["r1@%"]);
    assert_eq!(
        events[3].1.error.as_ref().map(ToString::to_string),
        Some("access denied for noexist@127.0.0.1".to_owned())
    );
}

/// 可追踪关闭次数的 mock 会话。
struct TrackedSession {
    /// 累计 `close` 调用次数。
    closed: Arc<AtomicUsize>,
}

impl MockSession for TrackedSession {
    /// 将 SQL 原文写入输出缓冲。
    fn handle_query(&self, sql: &str, output: &mut Vec<u8>) -> Result<(), Error> {
        output.extend_from_slice(sql.as_bytes());
        Ok(())
    }

    /// 将原始协议包写入输出缓冲。
    fn dispatch(&self, packet: &[u8], output: &mut Vec<u8>) -> Result<(), Error> {
        output.extend_from_slice(packet);
        Ok(())
    }

    /// 关闭会话并递增计数。
    fn close(&self) -> Result<(), Error> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// root 鉴权恒成功。
    fn authenticate_root(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// 记录已创建会话弱引用与关闭计数的 mock 驱动。
#[derive(Default)]
struct TrackingDriver {
    /// 所有会话共享的关闭计数器。
    closed: Arc<AtomicUsize>,
    /// 已 open 会话的弱引用，用于断言关闭后无强引用残留。
    sessions: Mutex<Vec<Weak<TrackedSession>>>,
}

impl MockDriver for TrackingDriver {
    /// 打开会话：校验 collation=45（utf8mb4_general_ci），并登记弱引用。
    fn open(&self, _connection_id: u64, collation: u8) -> Result<Arc<dyn MockSession>, Error> {
        assert_eq!(collation, 45);
        let session = Arc::new(TrackedSession {
            closed: self.closed.clone(),
        });
        self.sessions
            .lock()
            .expect("session list lock poisoned")
            .push(Arc::downgrade(&session));
        Ok(session)
    }
}

/// 验证批量创建并关闭 100 个连接后，每次 close 都被调用且会话无强引用残留。
#[test]
fn closing_many_connections_releases_every_server_session() {
    let driver = Arc::new(TrackingDriver::default());
    let server = CreateMockServer(driver.clone());
    let connections = (0..100)
        .map(|_| CreateMockConn(&server).expect("mock connection"))
        .collect::<Vec<_>>();

    // GetOutput 返回刚写入的响应并轮换到空缓冲，避免后续请求复用旧响应。
    for (index, connection) in connections.iter().enumerate() {
        connection
            .HandleQuery(&format!("SELECT {index}"))
            .expect("query");
        let output = connection.GetOutput();
        assert_eq!(
            &*output.lock().expect("output lock poisoned"),
            format!("SELECT {index}").as_bytes()
        );
        assert!(
            connection
                .GetOutput()
                .lock()
                .expect("rotated output lock poisoned")
                .is_empty(),
            "GetOutput must rotate to an empty response buffer"
        );
    }
    for connection in &connections {
        connection.Close().expect("close connection");
    }
    drop(connections);

    assert_eq!(driver.closed.load(Ordering::SeqCst), 100);
    assert!(
        driver
            .sessions
            .lock()
            .expect("session list lock poisoned")
            .iter()
            .all(|session| session.upgrade().is_none()),
        "server must not retain closed sessions"
    );
}
