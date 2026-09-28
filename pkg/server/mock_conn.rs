// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Mock 连接与 Mock Server：单元测试用的轻量 MySQL 会话/驱动替身。
//
// 提供 MockSession/MockDriver 契约、带输出缓冲的 mockConn，以及 Auth Socket
// 场景下可注入的操作系统用户名桩。

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Mock 层简易错误，以消息字符串承载。
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 模拟会话：处理查询、分发协议包、关闭与 root 认证。
pub trait MockSession: Send + Sync {
    /// 执行 SQL 文本并将协议响应写入 output。
    fn handle_query(&self, sql: &str, output: &mut Vec<u8>) -> Result<(), Error>;
    /// 分发原始 MySQL 协议包到会话并收集响应。
    fn dispatch(&self, packet: &[u8], output: &mut Vec<u8>) -> Result<(), Error>;
    /// 关闭会话并释放相关资源。
    fn close(&self) -> Result<(), Error>;
    /// 以 root 身份完成测试用认证握手。
    fn authenticate_root(&self) -> Result<(), Error>;
}

/// 模拟驱动：按连接 ID 与 collation 打开 MockSession。
pub trait MockDriver: Send + Sync {
    /// 打开新会话；collation 为字符集校对规则编号（如 utf8mb4 常用 45）。
    fn open(&self, connection_id: u64, collation: u8) -> Result<Arc<dyn MockSession>, Error>;
}

/// 持有 MockDriver 与活跃会话表的简易测试服务器。
pub struct Server {
    /// 创建会话的驱动实现。
    pub driver: Arc<dyn MockDriver>,
    /// connection_id → 会话，Close 时移除。
    clients: Mutex<HashMap<u64, Arc<dyn MockSession>>>,
}

impl Server {
    /// 用给定驱动构造空客户端表的 Mock Server。
    pub fn new(driver: Arc<dyn MockDriver>) -> Self {
        Self {
            driver,
            clients: Mutex::new(HashMap::new()),
        }
    }
}

/// 测试侧可见的 Mock 连接接口（查询、分发、输出缓冲）。
pub trait MockConn {
    /// 执行查询并将结果写入内部输出缓冲。
    fn HandleQuery(&self, sql: &str) -> Result<(), Error>;
    /// 返回底层 MockSession。
    fn Context(&self) -> Arc<dyn MockSession>;
    /// 分发原始协议字节到会话。
    fn Dispatch(&self, data: &[u8]) -> Result<(), Error>;
    /// 关闭会话并从 Server 客户端表注销。
    fn Close(&self) -> Result<(), Error>;
    /// 连接 ID。
    fn ID(&self) -> u64;
    /// 取输出缓冲；调用时会先清空，便于下次读取干净响应。
    fn GetOutput(&self) -> Arc<Mutex<Vec<u8>>>;
}

/// Mock 连接实现：弱引用 Server，持有会话与输出缓冲。
pub struct mockConn {
    connection_id: u64,
    session: Arc<dyn MockSession>,
    server: Weak<Server>,
    output: Mutex<Arc<Mutex<Vec<u8>>>>,
}

impl MockConn for mockConn {
    fn HandleQuery(&self, sql: &str) -> Result<(), Error> {
        let output = self
            .output
            .lock()
            .map_err(|_| Error("output endpoint lock poisoned".into()))?
            .clone();
        let mut output = output
            .lock()
            .map_err(|_| Error("output buffer lock poisoned".into()))?;
        self.session.handle_query(sql, &mut output)
    }

    fn Context(&self) -> Arc<dyn MockSession> {
        self.session.clone()
    }

    fn Dispatch(&self, data: &[u8]) -> Result<(), Error> {
        let output = self
            .output
            .lock()
            .map_err(|_| Error("output endpoint lock poisoned".into()))?
            .clone();
        let mut output = output
            .lock()
            .map_err(|_| Error("output buffer lock poisoned".into()))?;
        self.session.dispatch(data, &mut output)
    }

    fn Close(&self) -> Result<(), Error> {
        let result = self.session.close();
        // 连接关闭时从 Server 客户端表移除，避免泄漏。
        if let Some(server) = self.server.upgrade()
            && let Ok(mut clients) = server.clients.lock()
        {
            clients.remove(&self.connection_id);
        }
        result
    }

    fn ID(&self) -> u64 {
        self.connection_id
    }

    fn GetOutput(&self) -> Arc<Mutex<Vec<u8>>> {
        // Go 版会返回当前 writer 的 buffer，再将连接切换到新 buffer。
        // 保留旧 Arc 中已写入的响应，以便测试在后续请求发生后仍可检查它。
        let mut output = self.output.lock().expect("output endpoint lock poisoned");
        std::mem::replace(&mut *output, Arc::new(Mutex::new(Vec::new())))
    }
}

/// 创建共享的 Mock Server。
pub fn CreateMockServer(driver: Arc<dyn MockDriver>) -> Arc<Server> {
    Arc::new(Server::new(driver))
}

/// 分配递增连接 ID，打开会话、root 认证并登记到 Server。
pub fn CreateMockConn(server: &Arc<Server>) -> Result<Box<dyn MockConn>, Error> {
    // 全局递增连接号，从 1 起（0 常保留）。
    static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);
    let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
    // 45 = utf8mb4_general_ci 等默认 collation 编号。
    let session = server.driver.open(connection_id, 45)?;
    session.authenticate_root()?;
    server
        .clients
        .lock()
        .map_err(|_| Error("server client lock poisoned".into()))?
        .insert(connection_id, session.clone());
    Ok(Box::new(mockConn {
        connection_id,
        session,
        server: Arc::downgrade(server),
        output: Mutex::new(Arc::new(Mutex::new(Vec::new()))),
    }))
}

/// Auth Socket 插件测试用的进程级 OS 用户名桩。
fn mock_os_user() -> &'static Mutex<Option<String>> {
    static MOCK_OS_USER: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    MOCK_OS_USER.get_or_init(|| Mutex::new(None))
}

/// 注入 Auth Socket 校验时看到的操作系统用户名。
pub fn MockOSUserForAuthSocket(username: String) -> Result<(), Error> {
    *mock_os_user()
        .lock()
        .map_err(|_| Error("mock OS user lock poisoned".into()))? = Some(username);
    Ok(())
}

/// 清除注入的 OS 用户名，恢复未设置状态。
pub fn ClearOSUserForAuthSocket() -> Result<(), Error> {
    *mock_os_user()
        .lock()
        .map_err(|_| Error("mock OS user lock poisoned".into()))? = None;
    Ok(())
}

/// 按 Go auth_socket 契约校验 Unix socket 登录用户与 OS 用户/映射名。
pub fn AuthSocketUserMatches(
    mysql_user: &str,
    auth_string: &str,
    unix_socket: bool,
) -> Result<bool, Error> {
    if !unix_socket {
        return Ok(false);
    }
    let os_user = mock_os_user()
        .lock()
        .map_err(|_| Error("mock OS user lock poisoned".into()))?
        .clone();
    let Some(os_user) = os_user else {
        return Ok(false);
    };
    Ok(os_user == mysql_user || (!auth_string.is_empty() && os_user == auth_string))
}
