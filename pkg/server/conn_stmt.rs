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

// MySQL 预处理语句（Prepared Statement）协议处理。
//
// 覆盖 COM_STMT_PREPARE/EXECUTE/FETCH/CLOSE/SEND_LONG_DATA/RESET 与 COM_SET_OPTION，
// 并通过 `StatementRuntime` 注入实际执行、计划缓存重试与 TiFlash 回退逻辑。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// 游标类型与 COM_SET_OPTION 取值。
const CURSOR_TYPE_NO_CURSOR: u8 = 0;
const CURSOR_TYPE_READ_ONLY: u8 = 1;
const MYSQL_OPTION_MULTI_STATEMENTS_ON: u16 = 0;
const MYSQL_OPTION_MULTI_STATEMENTS_OFF: u16 = 1;
const CLIENT_MULTI_STATEMENTS: u32 = 1 << 16;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 预处理语句协议与运行时错误。
pub enum Error {
    MalformedPacket,
    NetPacketTooLarge,
    MemoryQuotaExceeded(u64),
    StatementNotFound(u32),
    WrongArguments(&'static str),
    Runtime(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NetPacketTooLarge => write!(
                f,
                "{}",
                *astersql_server_err::server_err::ErrNetPacketTooLarge
            ),
            Self::MemoryQuotaExceeded(id) => write!(
                f,
                "{}",
                astersql_util_dbterror_exeerrors::exeerrors::ErrMemoryExceedForQuery
                    .GenWithStackByArgs(&[(*id).into()])
            ),
            Self::MalformedPacket => f.write_str("malformed packet"),
            Self::StatementNotFound(id) => write!(f, "prepared statement {id} not found"),
            Self::WrongArguments(command) => write!(f, "wrong arguments to {command}"),
            Self::Runtime(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, Default, PartialEq)]
/// 二进制协议参数：类型、无符号、空值与原始字节。
pub struct BinaryParam {
    pub tp: u8,
    pub unsigned: bool,
    pub is_null: bool,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 预处理结果列的简化元信息。
pub struct ColumnInfo {
    pub name: String,
    pub column_type: u8,
}

/// 可迭代结果集抽象，支持游标拉取与懒游标。
pub trait ResultSet: Send {
    fn columns(&self) -> &[ColumnInfo];
    fn next(&mut self) -> Result<Option<Vec<Vec<u8>>>, Error>;
    fn close(&mut self) -> Result<(), Error>;
    fn exhausted(&self) -> bool;
    /// 是否支持不物化全量行的懒游标；默认否。
    fn supports_lazy_cursor(&self) -> bool {
        false
    }
}

/// Session-owned accounting boundary. Positive charges are refused before
/// Consume when the session quota would be reached; negative charges release.
pub trait LongDataMemory: Send + Sync {
    fn charge(&self, bytes: i64) -> Result<(bool, u64), Error>;
}

/// Long-data bytes charged by one statement and its deferred quota error.
#[derive(Default)]
pub struct LongDataState {
    memory: Option<Arc<dyn LongDataMemory>>,
    bytes: i64,
    rejected: Option<u64>,
}

impl LongDataState {
    pub fn new(memory: Option<Arc<dyn LongDataMemory>>) -> Self {
        Self {
            memory,
            ..Default::default()
        }
    }
    pub fn append(
        &mut self,
        params: &mut [Option<Vec<u8>>],
        too_large: &mut bool,
        max_packet: u64,
        param_id: usize,
        data: &[u8],
    ) -> Result<(), Error> {
        let param = params
            .get_mut(param_id)
            .ok_or(Error::WrongArguments("stmt_send_longdata"))?;
        if data.is_empty() {
            let released = param.as_ref().map_or(0, Vec::len) as i64;
            if released > 0 {
                if let Some(memory) = &self.memory {
                    memory.charge(-released)?;
                }
                self.bytes -= released;
            }
            *param = Some(Vec::new());
            return Ok(());
        }
        if *too_large || self.rejected.is_some() {
            return Ok(());
        }
        if param.as_ref().map_or(0, Vec::len) as u64 + data.len() as u64 > max_packet {
            *too_large = true;
            return Ok(());
        }
        let chunk = data.len() as i64;
        if let Some(memory) = &self.memory {
            let (accepted, connection_id) = memory.charge(chunk)?;
            if !accepted {
                self.rejected = Some(connection_id);
                return Ok(());
            }
        }
        param.get_or_insert_with(Vec::new).extend_from_slice(data);
        self.bytes += chunk;
        Ok(())
    }

    pub fn check(
        &self,
        params: &[Option<Vec<u8>>],
        too_large: bool,
        max_packet: u64,
    ) -> Result<(), Error> {
        if let Some(id) = self.rejected {
            return Err(Error::MemoryQuotaExceeded(id));
        }
        check_long_data_size(params, too_large, max_packet)
    }

    pub fn release(
        &mut self,
        params: &mut [Option<Vec<u8>>],
        too_large: &mut bool,
    ) -> Result<(), Error> {
        if self.bytes > 0 {
            if let Some(memory) = &self.memory {
                memory.charge(-self.bytes)?;
            }
            self.bytes = 0;
        }
        params.fill(None);
        *too_large = false;
        self.rejected = None;
        Ok(())
    }
}

/// 连接上缓存的一条预处理语句及其绑定参数与游标状态。
pub struct PreparedStatement {
    pub id: u32,
    pub sql: String,
    pub num_params: usize,
    pub columns: Vec<ColumnInfo>,
    pub bound_params: Vec<Option<Vec<u8>>>,
    pub bound_params_too_large: bool,
    pub long_data: LongDataState,
    pub max_allowed_packet: u64,
    pub params_type: Vec<u8>,
    pub last_params: Vec<BinaryParam>,
    pub cursor: Option<Box<dyn ResultSet>>,
    pub cursor_active: bool,
    pub protocol_cursor: Option<crate::conn::QueryResult>,
}

impl PreparedStatement {
    /// 清空绑定参数并关闭活动游标。
    pub(crate) fn reset(&mut self) -> Result<(), Error> {
        self.long_data
            .release(&mut self.bound_params, &mut self.bound_params_too_large)?;
        self.cursor_active = false;
        if let Some(cursor) = self.protocol_cursor.take() {
            if let Some(source) = &cursor.result_set {
                source
                    .close()
                    .map_err(|error| Error::Runtime(error.to_string()))?;
            }
            if let Some(lifecycle) = cursor.response_lifecycle {
                lifecycle.finish();
            }
        }
        if let Some(mut cursor) = self.cursor.take() {
            cursor.close()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 写回给测试/上层观察的协议事件（Prepare/Result/Ok）。
pub enum ProtocolEvent {
    Prepared {
        statement_id: u32,
        columns: Vec<ColumnInfo>,
        params: usize,
    },
    Result {
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<Vec<u8>>>,
        cursor_exists: bool,
        last_row_sent: bool,
    },
    Ok,
}

/// 预处理执行运行时：准备、执行、缓存文本、RU 统计与 TiFlash 回退。
/// TiFlash 为列存引擎，出错时可回退到行存重试。
pub trait StatementRuntime: Send + Sync {
    fn long_data_memory(&self) -> Option<Arc<dyn LongDataMemory>> {
        None
    }
    fn max_allowed_packet(&self) -> u64 {
        astersql_sessionctx_vardef::DefMaxAllowedPacket
    }
    fn prepare(&self, sql: &str) -> Result<(u32, usize, Vec<ColumnInfo>), Error>;
    fn execute(
        &self,
        statement_id: u32,
        args: &[BinaryParam],
    ) -> Result<Option<Box<dyn ResultSet>>, Error>;
    fn close_statement(&self, statement_id: u32) -> Result<(), Error>;
    fn statement_cache_text(&self, statement_id: u32) -> Option<String>;
    fn statement_cache_valid(&self, statement_id: u32) -> bool;
    fn cursor_ru_delta(&self, statement_id: u32, rows: usize);
    fn retry_after_statement_error(&self, error: &Error) -> Result<bool, Error>;
    fn should_fallback_tiflash(&self, error: &Error) -> bool;
    fn set_tiflash_enabled(&self, enabled: bool);
    fn append_statement_warning(&self, error: Error);
}

#[allow(non_camel_case_types)]
/// 本模块使用的精简连接视图：能力位、语句表、输出事件与运行时。
pub struct clientConn {
    pub capability: u32,
    pub statements: HashMap<u32, PreparedStatement>,
    pub output: Vec<ProtocolEvent>,
    pub runtime: Arc<dyn StatementRuntime>,
}

/// 当前由 MySQL 协议连接持有的预处理语句数，对齐 Go `variable.PreparedStmtCount`。
pub static PreparedStmtCount: AtomicI64 = AtomicI64::new(0);

/// COM_STMT_PREPARE：准备语句并记录到连接语句表，输出 Prepared 事件。
pub fn HandleStmtPrepare(cc: &mut clientConn, sql: &str) -> Result<(), Error> {
    let (id, num_params, columns) = cc.runtime.prepare(sql)?;
    let statement = PreparedStatement {
        id,
        sql: sql.to_owned(),
        num_params,
        columns: columns.clone(),
        bound_params: vec![None; num_params],
        bound_params_too_large: false,
        long_data: LongDataState::new(cc.runtime.long_data_memory()),
        max_allowed_packet: cc.runtime.max_allowed_packet(),
        params_type: Vec::new(),
        last_params: Vec::new(),
        cursor: None,
        cursor_active: false,
        protocol_cursor: None,
    };
    if cc.statements.insert(id, statement).is_none() {
        PreparedStmtCount.fetch_add(1, Ordering::AcqRel);
    }
    cc.output.push(ProtocolEvent::Prepared {
        statement_id: id,
        columns,
        params: num_params,
    });
    Ok(())
}

/// COM_STMT_EXECUTE：解析游标标志、null bitmap 与参数后执行。
pub fn handleStmtExecute(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() < 9 {
        return Err(Error::MalformedPacket);
    }
    let statement_id = read_u32(data, 0)?;
    let statement = cc
        .statements
        .get_mut(&statement_id)
        .ok_or(Error::StatementNotFound(statement_id))?;
    statement.max_allowed_packet = cc.runtime.max_allowed_packet();
    let (args, use_cursor) = ParseExecuteParams(statement, data)?;
    executePlanCacheStmt(cc, statement_id, args, use_cursor)
}

/// Decode one COM_STMT_EXECUTE payload while preserving long-data and the
/// previous parameter type array for `new-params-bound-flag = 0` reuse.
pub(crate) fn ParseExecuteParams(
    statement: &mut PreparedStatement,
    data: &[u8],
) -> Result<(Vec<BinaryParam>, bool), Error> {
    if data.len() < 9 {
        return Err(Error::MalformedPacket);
    }
    let statement_id = read_u32(data, 0)?;
    let cursor_flag = data[4];
    if !matches!(cursor_flag, CURSOR_TYPE_NO_CURSOR | CURSOR_TYPE_READ_ONLY) {
        return Err(Error::MalformedPacket);
    }
    if statement.id != statement_id {
        return Err(Error::StatementNotFound(statement_id));
    }
    let mut pos = 9;
    let bitmap_len = statement.num_params.div_ceil(8);
    let null_bitmap = data
        .get(pos..pos + bitmap_len)
        .ok_or(Error::MalformedPacket)?;
    pos += bitmap_len;
    let mut param_types = statement.params_type.clone();
    if statement.num_params > 0 {
        let new_params_bound = *data.get(pos).ok_or(Error::MalformedPacket)?;
        pos += 1;
        if new_params_bound == 1 {
            let size = statement
                .num_params
                .checked_mul(2)
                .ok_or(Error::MalformedPacket)?;
            param_types = data
                .get(pos..pos + size)
                .ok_or(Error::MalformedPacket)?
                .to_vec();
            statement.params_type = param_types.clone();
            pos += size;
        }
    }
    let parsed = statement
        .long_data
        .check(
            &statement.bound_params,
            statement.bound_params_too_large,
            statement.max_allowed_packet,
        )
        .and_then(|()| {
            if param_types.len() != statement.num_params * 2 {
                return Err(Error::MalformedPacket);
            }
            parse_params(statement, null_bitmap, &param_types, &data[pos..])
        });
    // EXECUTE 会消费 long-data 参数并关闭旧游标；保留 param_types 供后续包省略类型。
    // EXECUTE consumes long-data arguments and closes a previous cursor. Keep
    // param_types because later packets may omit them. Go performs this reset
    // even when binary parameter decoding fails, so a malformed packet cannot
    // leave a previous cursor or long-data binding alive.
    let _ = statement.reset();
    let args = parsed?;
    statement.last_params = args.clone();
    Ok((args, cursor_flag == CURSOR_TYPE_READ_ONLY))
}

/// 执行计划缓存语句：失败时按运行时策略重试或禁用 TiFlash 后重试。
/// 执行计划（execution plan）是优化器为 SQL 选定的算子树。
pub fn executePlanCacheStmt(
    cc: &mut clientConn,
    statement_id: u32,
    args: Vec<BinaryParam>,
    use_cursor: bool,
) -> Result<(), Error> {
    let first = cc
        .runtime
        .execute(statement_id, &args)
        .and_then(|result_set| {
            executePreparedStmtAndWriteResult(cc, statement_id, result_set, use_cursor).map(|_| ())
        });
    let error = match first {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if cc.runtime.retry_after_statement_error(&error)? {
        let result_set = cc.runtime.execute(statement_id, &args)?;
        return executePreparedStmtAndWriteResult(cc, statement_id, result_set, use_cursor)
            .map(|_| ());
    }
    if cc.runtime.should_fallback_tiflash(&error) {
        cc.runtime.set_tiflash_enabled(false);
        let retry = cc
            .runtime
            .execute(statement_id, &args)
            .and_then(|result_set| {
                executePreparedStmtAndWriteResult(cc, statement_id, result_set, use_cursor)
                    .map(|_| ())
            });
        cc.runtime.set_tiflash_enabled(true);
        cc.runtime.append_statement_warning(error);
        return retry;
    }
    Err(error)
}

/// 将执行结果写为 Ok 或完整 Result 事件；若请求游标则转入游标路径。
pub fn executePreparedStmtAndWriteResult(
    cc: &mut clientConn,
    statement_id: u32,
    result_set: Option<Box<dyn ResultSet>>,
    use_cursor: bool,
) -> Result<bool, Error> {
    let Some(result_set) = result_set else {
        if let Some(statement) = cc.statements.get_mut(&statement_id) {
            statement.bound_params.fill(None);
        }
        cc.output.push(ProtocolEvent::Ok);
        return Ok(false);
    };
    if use_cursor {
        return executeWithCursor(cc, statement_id, result_set);
    }
    let mut result_set = result_set;
    let columns = result_set.columns().to_vec();
    let mut rows = Vec::new();
    loop {
        match result_set.next() {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => break,
            Err(error) => {
                // The Go caller defers ResultSet.Close whenever execution
                // returned a result set, including protocol/iteration errors.
                let _ = result_set.close();
                return Err(error);
            }
        }
    }
    result_set.close()?;
    if let Some(statement) = cc.statements.get_mut(&statement_id) {
        statement.bound_params.fill(None);
    }
    cc.output.push(ProtocolEvent::Result {
        columns,
        rows,
        cursor_exists: false,
        last_row_sent: true,
    });
    Ok(false)
}

/// 游标执行：懒游标直接挂载，否则物化全部行后再挂载 MaterializedResultSet。
pub fn executeWithCursor(
    cc: &mut clientConn,
    statement_id: u32,
    result_set: Box<dyn ResultSet>,
) -> Result<bool, Error> {
    if result_set.supports_lazy_cursor() {
        return executeWithLazyCursor(cc, statement_id, result_set);
    }
    let mut source = result_set;
    let columns = source.columns().to_vec();
    let mut rows = Vec::new();
    loop {
        match source.next() {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => break,
            Err(error) => {
                // Go closes the row container/result set on every eager
                // materialization error so temporary files and trackers are
                // detached before the error reaches COM_STMT_EXECUTE.
                let _ = source.close();
                return Err(error);
            }
        }
    }
    source.close()?;
    let statement = cc
        .statements
        .get_mut(&statement_id)
        .ok_or(Error::StatementNotFound(statement_id))?;
    statement.cursor = Some(Box::new(MaterializedResultSet {
        columns: columns.clone(),
        rows,
        offset: 0,
        closed: false,
    }));
    statement.cursor_active = true;
    statement.bound_params.fill(None);
    cc.output.push(ProtocolEvent::Result {
        columns,
        rows: Vec::new(),
        cursor_exists: true,
        last_row_sent: false,
    });
    Ok(false)
}

/// 懒游标：直接保存 ResultSet，首包只返回列信息。
pub fn executeWithLazyCursor(
    cc: &mut clientConn,
    statement_id: u32,
    result_set: Box<dyn ResultSet>,
) -> Result<bool, Error> {
    let columns = result_set.columns().to_vec();
    let statement = cc
        .statements
        .get_mut(&statement_id)
        .ok_or(Error::StatementNotFound(statement_id))?;
    statement.cursor = Some(result_set);
    statement.cursor_active = true;
    statement.bound_params.fill(None);
    cc.output.push(ProtocolEvent::Result {
        columns,
        rows: Vec::new(),
        cursor_exists: true,
        last_row_sent: false,
    });
    Ok(true)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 游标资源使用（RU）追踪：语句 ID 与已拉取行数。
pub struct CursorRUV2Tracker {
    pub statement_id: u32,
    pub fetched_rows: usize,
}
/// 构造初始拉取计数为 0 的 RU 追踪器。
pub fn buildCursorRUV2Tracker(statement_id: u32) -> CursorRUV2Tracker {
    CursorRUV2Tracker {
        statement_id,
        fetched_rows: 0,
    }
}

/// 按 fetch_size 从游标拉取行，更新 RU，并在耗尽时关闭游标。
pub fn writeExecuteResultWithCursor(
    cc: &mut clientConn,
    statement_id: u32,
    fetch_size: usize,
) -> Result<(), Error> {
    match cc.statements.get(&statement_id) {
        None => return Err(Error::StatementNotFound(statement_id)),
        Some(statement) if !statement.cursor_active => {
            return Err(Error::WrongArguments("stmt_fetch"));
        }
        Some(_) => {}
    }
    let result = (|| {
        let statement = cc
            .statements
            .get_mut(&statement_id)
            .ok_or(Error::StatementNotFound(statement_id))?;
        let cursor = statement
            .cursor
            .as_mut()
            .ok_or(Error::WrongArguments("stmt_fetch"))?;
        let columns = cursor.columns().to_vec();
        let mut rows = Vec::with_capacity(fetch_size);
        while rows.len() < fetch_size {
            match cursor.next()? {
                Some(row) => rows.push(row),
                None => break,
            }
        }
        let end = cursor.exhausted();
        cc.runtime.cursor_ru_delta(statement_id, rows.len());
        if end {
            statement.cursor_active = false;
            if let Some(mut cursor) = statement.cursor.take() {
                cursor.close()?;
            }
        }
        cc.output.push(ProtocolEvent::Result {
            columns,
            rows,
            cursor_exists: !end,
            last_row_sent: end,
        });
        Ok(())
    })();
    if result.is_err() {
        // Once an active cursor reaches the FETCH path, Go defers stmt.Reset so
        // any read/encoding failure cannot leave an open cursor or resources.
        if let Some(statement) = cc.statements.get_mut(&statement_id) {
            let _ = statement.reset();
        }
    }
    result
}

/// COM_STMT_FETCH：解析 statement_id 与 fetch_size 后拉取游标行。
pub fn handleStmtFetch(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() != 8 {
        return Err(Error::MalformedPacket);
    }
    let statement_id = read_u32(data, 0)?;
    // Go's parse.StmtFetchCmd caps a client request to maxFetchSize=1024.
    let fetch_size = read_u32(data, 4)?.min(1024) as usize;
    writeExecuteResultWithCursor(cc, statement_id, fetch_size)
}

/// COM_STMT_CLOSE：重置并移除本地语句，通知运行时关闭。
pub fn handleStmtClose(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() < 4 {
        return Ok(());
    }
    let statement_id = read_u32(data, 0)?;
    if let Some(mut statement) = cc.statements.remove(&statement_id) {
        statement.reset()?;
        cc.runtime.close_statement(statement_id)?;
        PreparedStmtCount.fetch_sub(1, Ordering::AcqRel);
    }
    Ok(())
}

/// COM_STMT_SEND_LONG_DATA：向指定参数槽追加二进制分片。
pub fn handleStmtSendLongData(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() < 6 {
        return Err(Error::MalformedPacket);
    }
    let statement_id = read_u32(data, 0)?;
    let param_id = u16::from_le_bytes([data[4], data[5]]) as usize;
    let statement = cc
        .statements
        .get_mut(&statement_id)
        .ok_or(Error::StatementNotFound(statement_id))?;
    statement.max_allowed_packet = cc.runtime.max_allowed_packet();
    statement.long_data.append(
        &mut statement.bound_params,
        &mut statement.bound_params_too_large,
        statement.max_allowed_packet,
        param_id,
        &data[6..],
    )
}

/// COM_STMT_RESET：重置语句绑定状态并写 Ok。
pub fn handleStmtReset(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() < 4 {
        return Err(Error::MalformedPacket);
    }
    let statement_id = read_u32(data, 0)?;
    let _ = cc
        .statements
        .get_mut(&statement_id)
        .ok_or(Error::StatementNotFound(statement_id))?
        .reset();
    cc.output.push(ProtocolEvent::Ok);
    Ok(())
}

/// COM_SET_OPTION：开关 CLIENT_MULTI_STATEMENTS 能力位。
pub fn handleSetOption(cc: &mut clientConn, data: &[u8]) -> Result<(), Error> {
    if data.len() < 2 {
        return Err(Error::MalformedPacket);
    }
    match u16::from_le_bytes([data[0], data[1]]) {
        MYSQL_OPTION_MULTI_STATEMENTS_ON => cc.capability |= CLIENT_MULTI_STATEMENTS,
        MYSQL_OPTION_MULTI_STATEMENTS_OFF => cc.capability &= !CLIENT_MULTI_STATEMENTS,
        _ => return Err(Error::MalformedPacket),
    }
    cc.output.push(ProtocolEvent::Ok);
    Ok(())
}

/// 将预处理 SQL 与最近参数渲染为诊断字符串。
pub fn preparedStmt2String(cc: &clientConn, stmt_id: u32) -> String {
    let Some(statement) = cc.statements.get(&stmt_id) else {
        return String::new();
    };
    let params = statement
        .last_params
        .iter()
        .map(|value| {
            if value.is_null {
                "NULL".into()
            } else {
                render_param(&value.value)
            }
        })
        .collect::<Vec<_>>();
    format!("{} [{}]", statement.sql, params.join(", "))
}
/// 仅返回预处理 SQL 文本（不含参数）。
pub fn preparedStmt2StringNoArgs(cc: &clientConn, stmt_id: u32) -> String {
    cc.statements
        .get(&stmt_id)
        .map(|statement| statement.sql.clone())
        .unwrap_or_default()
}
/// 查询计划缓存中的语句文本及是否失效。
pub fn preparedStmtID2CachePreparedStmt(cc: &clientConn, stmt_id: u32) -> (Option<String>, bool) {
    let valid = cc.runtime.statement_cache_valid(stmt_id);
    (cc.runtime.statement_cache_text(stmt_id), !valid)
}

/// 按 null bitmap、类型数组与值区解析二进制参数；优先使用已绑定的 long-data。
fn parse_params(
    statement: &PreparedStatement,
    bitmap: &[u8],
    types: &[u8],
    values: &[u8],
) -> Result<Vec<BinaryParam>, Error> {
    let mut params = Vec::with_capacity(statement.num_params);
    let mut pos = 0;
    for index in 0..statement.num_params {
        if let Some(bound) = &statement.bound_params[index] {
            params.push(BinaryParam {
                tp: types.get(index * 2).copied().unwrap_or(252),
                value: bound.clone(),
                ..BinaryParam::default()
            });
            continue;
        }
        if bitmap[index >> 3] & (1 << (index & 7)) != 0 {
            params.push(BinaryParam {
                tp: 6,
                is_null: true,
                ..BinaryParam::default()
            });
            continue;
        }
        let tp = *types.get(index * 2).ok_or(Error::MalformedPacket)?;
        let unsigned = types.get(index * 2 + 1).ok_or(Error::MalformedPacket)? & 0x80 != 0;
        let length = match tp {
            6 => 0,
            1 => 1,
            2 | 13 => 2,
            3 | 4 | 9 => 4,
            5 | 8 => 8,
            7 | 10 | 11 | 12 => {
                let length = *values.get(pos).ok_or(Error::MalformedPacket)? as usize;
                pos += 1;
                length
            }
            0 | 15 | 16 | 246..=255 => {
                let (length, is_null, used) = read_length_encoded_int(&values[pos..])?;
                pos += used;
                if is_null {
                    params.push(BinaryParam {
                        tp,
                        unsigned,
                        is_null: true,
                        value: Vec::new(),
                    });
                    continue;
                }
                length
            }
            _ => return Err(Error::Runtime(format!("stmt unknown field type {tp}"))),
        };
        let end = pos.checked_add(length).ok_or(Error::MalformedPacket)?;
        let value = values.get(pos..end).ok_or(Error::MalformedPacket)?.to_vec();
        pos = end;
        params.push(BinaryParam {
            tp,
            unsigned,
            is_null: false,
            value,
        });
    }
    Ok(params)
}

/// 解析 length-encoded integer，返回 (长度, 是否 NULL, 消耗字节数)。
fn read_length_encoded_int(data: &[u8]) -> Result<(usize, bool, usize), Error> {
    let first = *data.first().ok_or(Error::MalformedPacket)?;
    let (length, is_null, used) = match first {
        0..=250 => (usize::from(first), false, 1),
        251 => (0, true, 1),
        252 => (usize::from(read_u16(data, 1)?), false, 3),
        253 => {
            let bytes = data.get(1..4).ok_or(Error::MalformedPacket)?;
            (
                usize::from(bytes[0])
                    | (usize::from(bytes[1]) << 8)
                    | (usize::from(bytes[2]) << 16),
                false,
                4,
            )
        }
        254 => {
            let bytes: [u8; 8] = data
                .get(1..9)
                .ok_or(Error::MalformedPacket)?
                .try_into()
                .map_err(|_| Error::MalformedPacket)?;
            (
                usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| Error::MalformedPacket)?,
                false,
                9,
            )
        }
        _ => return Err(Error::MalformedPacket),
    };
    Ok((length, is_null, used))
}
/// 小端读取 u32。
fn read_u32(data: &[u8], offset: usize) -> Result<u32, Error> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or(Error::MalformedPacket)?
        .try_into()
        .map_err(|_| Error::MalformedPacket)?;
    Ok(u32::from_le_bytes(bytes))
}
/// 小端读取 u16。
fn read_u16(data: &[u8], offset: usize) -> Result<u16, Error> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .ok_or(Error::MalformedPacket)?
        .try_into()
        .map_err(|_| Error::MalformedPacket)?;
    Ok(u16::from_le_bytes(bytes))
}
/// 将参数字节渲染为 SQL 字面量或十六进制串。
fn render_param(value: &[u8]) -> String {
    match std::str::from_utf8(value) {
        Ok(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
        Err(_) => value.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

/// 物化后的内存结果集，供非懒游标 FETCH。
struct MaterializedResultSet {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<Vec<u8>>>,
    offset: usize,
    closed: bool,
}
impl ResultSet for MaterializedResultSet {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    fn next(&mut self) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if self.closed {
            return Ok(None);
        }
        let row = self.rows.get(self.offset).cloned();
        if row.is_some() {
            self.offset += 1;
        }
        Ok(row)
    }
    fn close(&mut self) -> Result<(), Error> {
        self.closed = true;
        Ok(())
    }
    fn exhausted(&self) -> bool {
        self.closed || self.offset >= self.rows.len()
    }
}

pub(crate) fn check_long_data_size(
    params: &[Option<Vec<u8>>],
    too_large: bool,
    max: u64,
) -> Result<(), Error> {
    if too_large
        || params
            .iter()
            .any(|value| value.as_ref().map_or(0, Vec::len) as u64 > max)
    {
        Err(Error::NetPacketTooLarge)
    } else {
        Ok(())
    }
}
