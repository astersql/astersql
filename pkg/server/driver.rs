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

// Server Driver 抽象：连接会话、结果集与预处理语句生命周期契约。
//
// IDriver 为每个客户端连接打开 DriverContext；PreparedStatement 描述
// COM_STMT_* 命令所需的参数绑定、游标结果集与行容器状态。

use std::fmt;

use crate::conn::CancellationToken;

/// Driver 层简易错误，以消息字符串承载。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// TLS 握手后的传输状态快照（协议版本、密码套件、对端证书）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TlsState {
    /// 协商得到的 TLS 协议版本字符串。
    pub protocol: Option<String>,
    /// 协商得到的密码套件。
    pub cipher_suite: Option<String>,
    /// 对端证书 DER 字节列表。
    pub peer_certificates: Vec<Vec<u8>>,
}

/// 会话扩展点名称列表（插件/扩展注册名）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionExtensions {
    /// 已启用的扩展名称。
    pub names: Vec<String>,
}

/// 协议层表达式取值：对应预处理参数或结果单元格的基本类型。
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Signed(i64),
    /// 无符号整数。
    Unsigned(u64),
    /// 浮点值。
    Float(f64),
    /// 字节/字符串载荷。
    Bytes(Vec<u8>),
}

/// 可迭代结果集：逐行拉取 Expression 向量，结束时 close。
pub trait ResultSet: Send {
    fn next(&mut self, cancel: &CancellationToken) -> Result<Option<Vec<Expression>>, Error>;
    fn close(&mut self) -> Result<(), Error>;
}

/// 带游标语义的结果集：在 COM_STMT_FETCH 场景下需显式 finish。
pub trait CursorResultSet: ResultSet {
    fn finish(&mut self) -> Result<(), Error>;
}

/// 行容器：缓冲或跟踪结果行，关闭时释放资源。
pub trait RowContainer: Send {
    fn close(&mut self) -> Result<(), Error>;
}

/// 单连接会话上下文：关闭时释放会话侧资源。
pub trait DriverContext: Send {
    fn close(&mut self) -> Result<(), Error>;
}

/// Opens a live session context for a client connection.
///
/// 为客户端连接打开活跃会话上下文（DriverContext）。
pub trait IDriver: Send + Sync {
    fn OpenCtx(
        &self,
        conn_id: u64,
        capability: u32,
        collation: u8,
        db_name: &str,
        tls_state: Option<TlsState>,
        extensions: Option<SessionExtensions>,
    ) -> Result<Box<dyn DriverContext>, Error>;
}

/// Lifecycle contract used by COM_STMT_* handlers.
///
/// COM_STMT_* 处理器使用的预处理语句生命周期契约：绑定参数、执行、游标与重置。
pub trait PreparedStatement: Send {
    fn ID(&self) -> i32;
    fn Execute(
        &mut self,
        cancel: &CancellationToken,
        args: &[Expression],
    ) -> Result<Box<dyn ResultSet>, Error>;
    fn AppendParam(&mut self, param_id: usize, data: &[u8]) -> Result<(), Error>;
    fn NumParams(&self) -> usize;
    fn BoundParams(&self) -> &[Option<Vec<u8>>];
    fn SetParamsType(&mut self, params_type: Vec<u8>);
    fn GetParamsType(&self) -> &[u8];
    fn StoreResultSet(&mut self, result_set: Option<Box<dyn CursorResultSet>>);
    fn GetResultSet(&mut self) -> Option<&mut (dyn CursorResultSet + '_)>;
    fn Reset(&mut self) -> Result<(), Error>;
    fn Close(&mut self) -> Result<(), Error>;
    fn GetCursorActive(&self) -> bool;
    fn SetCursorActive(&mut self, active: bool);
    fn StoreRowContainer(&mut self, container: Option<Box<dyn RowContainer>>);
    fn GetRowContainer(&mut self) -> Option<&mut (dyn RowContainer + '_)>;
}
