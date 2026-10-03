// Copyright 2026 AsterSQL.
//! Local stand-ins for SQL/MySQL/storage/HTTP/metrics/version/utils (arm64-safe).

//
// 本地桩模块：在 arm64/无 CGO 环境下替代 SQL driver、etcd、PD、prometheus、storage 等外部依赖。
// 对应 Go 侧多个 pkg 的最小可运行替身，供 dumpling/export 单测与离线编译。
// 能力边界：脚本化 Conn/DB、内存 Storage、简化的 OutputTemplate；非生产实现。
//

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
// 统一错误类型，兼容 Go errors + MySQL driver 错误包装。
pub struct Error {
    pub msg: String,
    pub mysql: Option<MySQLError>,
    pub exceed_upload_parts: bool,
}

// Error 构造与 MySQL 错误挂载。
impl Error {
    // 纯文本错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            mysql: None,
            exceed_upload_parts: false,
        }
    }
    // 附带 MySQL 错误码以便 IsRetryableError 使用。
    pub fn with_mysql(e: MySQLError) -> Self {
        Self {
            msg: e.to_string(),
            mysql: Some(e),
            exceed_upload_parts: false,
        }
    }
    // 兼容 Go Error() 方法名。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

// Display 输出 msg 字段。
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
// 实现 std::error::Error 以便 ? 与日志格式化。
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if self.exceed_upload_parts {
            Some(&astersql_objstore_storeapi::ErrExceedMaxUploadParts)
        } else {
            None
        }
    }
}
// PartialEq 仅比较 msg，忽略 mysql 子错误。
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.msg == other.msg
    }
}

// 本 crate 内 Result 别名，Err 固定为 Error。
pub type Result<T> = std::result::Result<T, Error>;

// 构造纯文本 Error，对应 Go errors.New。
pub fn errors_new(msg: impl Into<String>) -> Error {
    Error::new(msg)
}
// Display 格式化后构造 Error。
pub fn errors_errorf(msg: impl fmt::Display) -> Error {
    Error::new(msg.to_string())
}
// 错误链 trace 占位：桩直接返回原错误。
pub fn errors_trace(err: Error) -> Error {
    err
}
// 在错误消息前追加上下文前缀。
pub fn errors_annotate(err: Error, msg: impl Into<String>) -> Error {
    Error {
        msg: format!("{}: {}", msg.into(), err.msg),
        mysql: err.mysql,
        exceed_upload_parts: err.exceed_upload_parts,
    }
}
// 带 Display 格式的前缀 annotate。
pub fn errors_annotatef(err: Error, fmt_msg: impl fmt::Display) -> Error {
    Error {
        msg: format!("{}: {}", fmt_msg, err.msg),
        mysql: err.mysql,
        exceed_upload_parts: err.exceed_upload_parts,
    }
}
// 返回根因 Error（桩无链式 cause）。
pub fn errors_cause(err: &Error) -> &Error {
    err
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
// 源端数据库类型，与 Go version.ServerType 枚举值对齐。
pub enum ServerType {
    #[default]
    // 未能识别或尚未探测。
    ServerTypeUnknown = 0,
    // 上游为 Oracle MySQL。
    ServerTypeMySQL = 1,
    // 上游为 MariaDB 分支。
    ServerTypeMariaDB = 2,
    // 上游为 TiDB（含 TiKV 部署）。
    ServerTypeTiDB = 3,
    // 通配类型，filter 解析时使用。
    ServerTypeAll = 4,
}

// ServerType 到可读字符串。
impl ServerType {
    // 返回类型名的静态字符串。
    pub fn String(self) -> &'static str {
        match self {
            Self::ServerTypeUnknown => "Unknown",
            Self::ServerTypeMySQL => "MySQL",
            Self::ServerTypeMariaDB => "MariaDB",
            Self::ServerTypeTiDB => "TiDB",
            Self::ServerTypeAll => "",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
// 语义化版本三元组 + pre_release，供 ServerInfo 解析。
pub struct SemVer {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
    pub pre_release: String,
}

// SemVer 解析与 LessThan 比较（不含 pre_release 序）。
impl SemVer {
    pub fn new(s: &str) -> Self {
        parse_semver(s)
    }
    // 仅比较 major/minor/patch 三元组。
    pub fn LessThan(&self, other: &SemVer) -> bool {
        (self.major, self.minor, self.patch) < (other.major, other.minor, other.patch)
    }
}

// 从版本字符串解析 SemVer，容忍前缀 v。
pub fn parse_semver(s: &str) -> SemVer {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('-');
    let nums = parts.next().unwrap_or("0.0.0");
    let pre = parts.next().unwrap_or("").to_string();
    let mut it = nums.split('.');
    SemVer {
        major: it.next().and_then(|x| x.parse().ok()).unwrap_or(0),
        minor: it.next().and_then(|x| x.parse().ok()).unwrap_or(0),
        patch: it.next().and_then(|x| x.parse().ok()).unwrap_or(0),
        pre_release: pre,
    }
}

#[derive(Clone, Debug, Default)]
// 连接目标的服务器类型、版本及是否 TiKV 后端。
// 连接目标的服务器类型、版本及是否 TiKV 后端。
pub struct ServerInfo {
    pub ServerType: ServerType,
    pub ServerVersion: Option<SemVer>,
    // HasTiKV 表示是否 TiKV 存储（snapshot 一致性相关）。
    pub HasTiKV: bool,
}

// 从 SHOW 类语句或连接串片段推断 ServerType/版本。
pub fn ParseServerInfo(src: &str) -> ServerInfo {
    let lower = src.to_ascii_lowercase();
    let mut info = ServerInfo::default();
    if lower.contains("release version:") || lower.contains("tidb") {
        info.ServerType = ServerType::ServerTypeTiDB;
    } else if lower.contains("mariadb") {
        info.ServerType = ServerType::ServerTypeMariaDB;
    } else if extract_version_nums(src).is_some() {
        info.ServerType = ServerType::ServerTypeMySQL;
    } else {
        info.ServerType = ServerType::ServerTypeUnknown;
    }
    if let Some(v) = extract_version_nums(src) {
        info.ServerVersion = Some(parse_semver(&v));
    }
    info
}

// 从混排文本中提取 x.y.z 形式版本号。
fn extract_version_nums(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut dots = 0;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_digit() {
                    i += 1;
                } else if c == b'.' {
                    dots += 1;
                    i += 1;
                } else {
                    break;
                }
            }
            if dots >= 2 {
                return Some(s[start..i].to_string());
            }
        } else {
            i += 1;
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
// MySQL 协议层错误码与消息，供 IsRetryableError 判断。
// MySQL 协议层错误码与消息，供 IsRetryableError 判断。
pub struct MySQLError {
    pub Number: u16,
    pub Message: String,
}
// MySQL 错误 Display：Error {code}: {msg}。
impl fmt::Display for MySQLError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error {}: {}", self.Number, self.Message)
    }
}
impl std::error::Error for MySQLError {}

#[derive(Clone, Debug, Default)]
// database/sql 打开参数的子集，FormatDSN 隐藏密码。
// database/sql 打开参数的子集，FormatDSN 隐藏密码。
pub struct MysqlConfig {
    pub User: String,
    pub Passwd: String,
    pub Net: String,
    pub Addr: String,
    pub DBName: String,
    pub Params: HashMap<String, String>,
    pub AllowCleartextPasswords: bool,
    pub Timeout: Duration,
    pub ReadTimeout: Duration,
    pub WriteTimeout: Duration,
    pub MaxAllowedPacket: usize,
}
// DSN 格式化（密码掩码）。
// DSN 格式化（密码掩码为 ***）。
impl MysqlConfig {
    // 生成日志安全 DSN，不泄露 Passwd。
    pub fn FormatDSN(&self) -> String {
        format!(
            "{}:***@{}({}/{})",
            self.User, self.Net, self.Addr, self.DBName
        )
    }
}

#[derive(Clone, Debug, Default)]
// Scan 目标：None 表示 SQL NULL。
pub struct RawBytes(pub Option<Vec<u8>>);
// NULL 判定与字节切片访问。
// NULL 判定与字节切片访问。
impl RawBytes {
    // 借用车内字节切片，NULL 时为 None。
    pub fn as_opt(&self) -> Option<&[u8]> {
        self.0.as_deref()
    }
    // 是否为 SQL NULL 列。
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
}

#[derive(Clone, Debug)]
// sql.ColumnType 替身，Nullable/DecimalSize 返回 (value, ok)。
// sql.ColumnType 替身，Nullable/DecimalSize 返回 (value, ok)。
pub struct ColumnType {
    pub name: String,
    pub database_type_name: String,
    pub nullable: bool,
    pub precision: i64,
    pub scale: i64,
}
// 列名/类型/Nullable/DecimalSize 访问器。
// 列名/类型/Nullable/DecimalSize 访问器。
impl ColumnType {
    // 结果集列名。
    pub fn Name(&self) -> &str {
        &self.name
    }
    // 数据库原生类型名，如 VARCHAR/INT。
    pub fn DatabaseTypeName(&self) -> &str {
        &self.database_type_name
    }
    // 返回 (nullable, true) 兼容 database/sql 签名。
    pub fn Nullable(&self) -> (bool, bool) {
        (self.nullable, true)
    }
    // 返回 (precision, scale, true)。
    pub fn DecimalSize(&self) -> (i64, i64, bool) {
        (self.precision, self.scale, true)
    }
}

#[derive(Debug, Default)]
// 脚本化结果集：内存二维表 + 游标 idx。
// 脚本化结果集：内存二维表 + 游标 idx。
pub struct Rows {
    pub columns: Vec<String>,
    pub col_types: Vec<ColumnType>,
    pub data: Vec<Vec<Option<Vec<u8>>>>,
    // idx 为 -1 表示尚未 Next；Scan 要求 idx 有效。
    pub idx: isize,
    pub closed: bool,
    pub err: Option<Error>,
    pub close_error: Option<Error>,
}
// 结果集迭代：Next/Scan/Columns/Close。
// 结果集迭代：Next/Scan/Columns/Close。
impl Rows {
    // 由列名与内存数据构造 Rows，列类型默认 VARCHAR。
    pub fn new(columns: Vec<String>, data: Vec<Vec<Option<Vec<u8>>>>) -> Self {
        let col_types = columns
            .iter()
            .map(|n| ColumnType {
                name: n.clone(),
                database_type_name: "VARCHAR".into(),
                nullable: true,
                precision: 0,
                scale: 0,
            })
            .collect();
        Self {
            columns,
            col_types,
            data,
            idx: -1,
            closed: false,
            err: None,
            close_error: None,
        }
    }
    // 游标前进，closed 或越界时返回 false。
    pub fn Next(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.idx += 1;
        (self.idx as usize) < self.data.len()
    }
    // 把当前行各列拷贝到 dest，列数由调用方保证。
    pub fn Scan(&mut self, dest: &mut [RawBytes]) -> Result<()> {
        if self.idx < 0 || (self.idx as usize) >= self.data.len() {
            return Err(errors_new("sql: Rows are closed"));
        }
        let row = &self.data[self.idx as usize];
        if dest.len() != self.columns.len() {
            return Err(errors_new(format!(
                "sql: expected {} destination arguments in Scan, not {}",
                self.columns.len(),
                dest.len()
            )));
        }
        for (i, d) in dest.iter_mut().enumerate() {
            d.0 = row.get(i).cloned().flatten();
        }
        Ok(())
    }
    // 返回列名克隆列表。
    pub fn Columns(&self) -> Result<Vec<String>> {
        Ok(self.columns.clone())
    }
    // 返回 ColumnType 克隆列表。
    pub fn ColumnTypes(&self) -> Result<Vec<ColumnType>> {
        Ok(self.col_types.clone())
    }
    // 标记 closed，后续 Next 恒 false。
    pub fn Close(&mut self) -> Result<()> {
        self.closed = true;
        self.close_error.take().map_or(Ok(()), Err)
    }
    // 迭代过程中设置的行级错误。
    pub fn Err(&self) -> Option<Error> {
        self.err.clone()
    }
}

#[derive(Debug, Default)]
// Exec 返回值，仅记录 rows_affected。
// Exec 返回值，仅记录 rows_affected。
pub struct SqlResult {
    pub rows_affected: i64,
}

#[derive(Clone, Debug, Default)]
// 单连接：共享 DB 级脚本队列，支持注入失败与 exec 日志。
pub struct Conn {
    // id 区分同 DB 派生的多个 Conn 实例。
    // closed 标记连接是否已 Close。
    // scripted 按 SQL 文本索引的应答队列（可 pop 消费）。
    // columns 与 scripted 配套的列名表。
    // exec_log 记录 ExecContext 提交的 SQL 便于断言。
    // fail_query 单次 Query/Exec 失败注入。
    // fail_queue 按序 pop 的失败队列。
    // ping_err PingContext 专用错误注入。
    pub id: u64,
    pub closed: Arc<AtomicBool>,
    pub scripted: Arc<Mutex<HashMap<String, VecDeque<Vec<Vec<Option<Vec<u8>>>>>>>>,
    pub columns: Arc<Mutex<HashMap<String, Vec<String>>>>,
    pub exec_log: Arc<Mutex<Vec<String>>>,
    pub fail_query: Arc<Mutex<Option<Error>>>,
    pub fail_queue: Arc<Mutex<VecDeque<Error>>>,
    pub ping_err: Arc<Mutex<Option<Error>>>,
    pub row_responses: Arc<Mutex<HashMap<String, std::collections::VecDeque<Rows>>>>,
}
// 脚本化 Query/Exec/Ping：exact 优先再模糊匹配 SQL。
impl Conn {
    pub fn new() -> Self {
        Self::default()
    }
    // 注册 Query 应答：columns 与 data 按 query 键入队。
    // DB 级 seed，派生 Conn 共享同一 scripted map。
    pub fn seed_query(&self, query: &str, columns: Vec<String>, data: Vec<Vec<Option<Vec<u8>>>>) {
        self.columns
            .lock()
            .unwrap()
            .insert(query.to_string(), columns);
        self.scripted
            .lock()
            .unwrap()
            .entry(query.to_string())
            .or_default()
            .push_back(data);
    }
    // Preserve driver column metadata and errors independently of whether any row exists.
    pub fn seed_rows(&self, query: &str, rows: Rows) {
        self.row_responses
            .lock()
            .unwrap()
            .entry(query.into())
            .or_default()
            .push_back(rows);
    }
    // 向 fail_queue 追加下一次 Query/Exec 应返回的错误。
    pub fn push_fail(&self, err: Error) {
        self.fail_queue.lock().unwrap().push_back(err);
    }
    // 优先消费 fail_queue，否则取 fail_query 单次注入。
    fn take_fail(&self) -> Option<Error> {
        if let Some(err) = self.fail_queue.lock().unwrap().pop_front() {
            return Some(err);
        }
        self.fail_query.lock().unwrap().clone()
    }
    // 按 query 查找脚本化结果，无匹配时返回默认单列空集。
    fn lookup_scripted(&self, query: &str) -> (Vec<String>, Vec<Vec<Option<Vec<u8>>>>) {
        let mut cols_g = self.columns.lock().unwrap();
        let mut data_g = self.scripted.lock().unwrap();
        // 优先精确匹配 query 文本（与 sqlmock ExpectQuery 一致）。
        if let Some(q) = data_g.get_mut(query) {
            let data = q.pop_front().unwrap_or_default();
            let cols = cols_g
                .get(query)
                .cloned()
                .unwrap_or_else(|| vec!["col".into()]);
            return (cols, data);
        }
        // 其次前缀/包含匹配，方便测试写短 SQL 片段。
        let key = cols_g
            .keys()
            .chain(data_g.keys())
            .find(|k| query.contains(k.as_str()) || k.contains(query))
            .cloned();
        if let Some(k) = key {
            let data = data_g
                .get_mut(&k)
                .and_then(|q| q.pop_front())
                .unwrap_or_default();
            let cols = cols_g
                .get(&k)
                .cloned()
                .unwrap_or_else(|| vec!["col".into()]);
            return (cols, data);
        }
        (vec!["col".into()], vec![])
    }
    // 执行 Query：先检查失败注入，再 lookup 构造 Rows。
    pub fn QueryContext(&self, query: &str) -> Result<Rows> {
        if let Some(err) = self.take_fail() {
            return Err(err);
        }
        if let Some(rows) = self
            .row_responses
            .lock()
            .unwrap()
            .get_mut(query)
            .and_then(|queue| queue.pop_front())
        {
            return Ok(rows);
        }
        let (cols, data) = self.lookup_scripted(query);
        Ok(Rows::new(cols, data))
    }
    // 执行 Exec：记入 exec_log，支持失败注入。
    pub fn ExecContext(&self, query: &str) -> Result<SqlResult> {
        self.exec_log.lock().unwrap().push(query.to_string());
        if let Some(err) = self.take_fail() {
            return Err(err);
        }
        Ok(SqlResult::default())
    }
    // Ping：检查 ping_err 与 closed 状态。
    pub fn PingContext(&self) -> Result<()> {
        if let Some(err) = self.ping_err.lock().unwrap().clone() {
            return Err(err);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(errors_new("sql: connection is already closed"));
        }
        Ok(())
    }
    // 标记 Conn 已关闭，后续 Ping 失败。
    // 关闭 DB 后 Conn() 返回 database is closed。
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
// 连接池替身：Conn() 分配递增 id，脚本数据 Conn 间共享。
// 连接池替身：Conn() 分配递增 id，脚本数据 Conn 间共享。
pub struct DB {
    // closed 关闭后 Conn() 返回错误。
    // scripted/columns 与 Conn 共享 Arc，seed 一次全局生效。
    // fail_query/fail_queue 失败注入共享给新 Conn。
    // next_id 单调递增分配 Conn.id。
    // query_row 供 QueryRow 类 API 返回单值。
    pub closed: Arc<AtomicBool>,
    pub scripted: Arc<Mutex<HashMap<String, VecDeque<Vec<Vec<Option<Vec<u8>>>>>>>>,
    pub columns: Arc<Mutex<HashMap<String, Vec<String>>>>,
    pub fail_query: Arc<Mutex<Option<Error>>>,
    pub fail_queue: Arc<Mutex<VecDeque<Error>>>,
    pub next_id: Arc<AtomicU64>,
    pub query_row: Arc<Mutex<HashMap<String, Option<i64>>>>,
}
// 分配 Conn、共享 seed 数据、Query 快捷路径。
// 分配 Conn、共享 seed 数据、Query 快捷路径。
impl DB {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn seed_query(&self, query: &str, columns: Vec<String>, data: Vec<Vec<Option<Vec<u8>>>>) {
        self.columns
            .lock()
            .unwrap()
            .insert(query.to_string(), columns);
        self.scripted
            .lock()
            .unwrap()
            .entry(query.to_string())
            .or_default()
            .push_back(data);
    }
    // 为 QueryRow 类调用 seed 单个 i64 返回值。
    pub fn seed_query_row(&self, query: &str, val: Option<i64>) {
        self.query_row
            .lock()
            .unwrap()
            .insert(query.to_string(), val);
    }
    // 派生新 Conn：共享脚本数据，独立 exec_log/ping_err。
    pub fn Conn(&self) -> Result<Conn> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(errors_new("sql: database is closed"));
        }
        Ok(Conn {
            id: self.next_id.fetch_add(1, Ordering::SeqCst) + 1,
            closed: Arc::new(AtomicBool::new(false)),
            scripted: self.scripted.clone(),
            columns: self.columns.clone(),
            exec_log: Arc::new(Mutex::new(Vec::new())),
            fail_query: self.fail_query.clone(),
            fail_queue: self.fail_queue.clone(),
            ping_err: Arc::new(Mutex::new(None)),
            row_responses: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
    // 便捷 Query：临时 Conn 执行 QueryContext。
    pub fn Query(&self, query: &str) -> Result<Rows> {
        self.Conn()?.QueryContext(query)
    }
}

// 重试 backoff 策略接口，对应 Go utils.Backoffer。
// 重试 backoff 策略接口，对应 Go utils.Backoffer。
pub trait BackoffStrategy {
    // 根据错误计算下次等待时长。
    fn NextBackoff(&mut self, err: &Error) -> Duration;
    // 剩余重试次数，<=0 时 WithRetry 终止。
    fn RemainingAttempts(&self) -> i32;
}

/// Go `utils.WithRetry` — retries while RemainingAttempts > 0 after NextBackoff.
// 在 RemainingAttempts 耗尽前循环调用 f，对齐 Go utils.WithRetry。
pub fn WithRetry<F>(done: bool, mut f: F, backoffer: &mut dyn BackoffStrategy) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    loop {
        // done 为 true 表示上层 context 已取消。
        if done {
            return Err(errors_new("context canceled"));
        }
        match f() {
            Ok(()) => return Ok(()),
            Err(err) => {
                // 无剩余次数则向上返回最后一次错误。
                if backoffer.RemainingAttempts() <= 0 {
                    return Err(err);
                }
                let _ = backoffer.NextBackoff(&err);
                if backoffer.RemainingAttempts() <= 0 {
                    return Err(err);
                }
            }
        }
    }
}

pub trait Filter: Send + Sync {
    // deny 规则在 MatchSchema 层生效，table 继承 schema 结果。
    fn MatchSchema(&self, schema: &str) -> bool;
    fn MatchTable(&self, schema: &str, table: &str) -> bool;
}

struct CaseInsensitiveFilter {
    inner: Arc<dyn Filter>,
}

impl Filter for CaseInsensitiveFilter {
    fn MatchSchema(&self, schema: &str) -> bool {
        self.inner.MatchSchema(&schema.to_lowercase())
    }

    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        self.inner
            .MatchTable(&schema.to_lowercase(), &table.to_lowercase())
    }
}

pub fn CaseInsensitive(filter: Arc<dyn Filter>) -> Arc<dyn Filter> {
    Arc::new(CaseInsensitiveFilter { inner: filter })
}

#[derive(Clone, Debug)]
// 基于 allow/deny 通配符的 Filter 实现。
// 基于 allow/deny 通配符的 Filter 实现。
pub struct PatternFilter {
    // allow 形如 db.table 或 *.* 的通配规则。
    pub allow: Vec<String>,
    // deny_schemas 命中则 MatchSchema 直接 false。
    pub deny_schemas: Vec<String>,
}
// 默认 allow *.*，不 deny 任何 schema。
impl Default for PatternFilter {
    fn default() -> Self {
        Self {
            allow: vec!["*.*".into()],
            deny_schemas: vec![],
        }
    }
}
// 通配符 * 匹配 schema/table，deny 优先。
impl Filter for PatternFilter {
    fn MatchSchema(&self, schema: &str) -> bool {
        if self
            .deny_schemas
            .iter()
            .any(|d| d.eq_ignore_ascii_case(schema))
        {
            return false;
        }
        self.allow.iter().any(|p| match p.split_once('.') {
            Some((db, _)) => db == "*" || db == schema,
            None => true,
        })
    }
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        if !self.MatchSchema(schema) {
            return false;
        }
        self.allow.iter().any(|p| match p.split_once('.') {
            Some((db, tbl)) => (db == "*" || db == schema) && (tbl == "*" || tbl == table),
            None => true,
        })
    }
}

// 把 schema 列表转成 db.* allow 规则。
pub fn NewSchemasFilter(schemas: &[&str]) -> Arc<dyn Filter> {
    let allow: Vec<String> = schemas.iter().map(|s| format!("{s}.*")).collect();
    Arc::new(PatternFilter {
        allow,
        deny_schemas: vec![],
    })
}

// 解析 filter 配置：! 前缀 deny，!/ 展开系统库 deny。
pub fn filter_parse(patterns: &[String]) -> Result<Arc<dyn Filter>> {
    let mut deny = Vec::new();
    let mut allow = Vec::new();
    for p in patterns {
        // !/ 是“排除系统库”的简写，展开为固定 deny 列表。
        if p.starts_with("!/") {
            for s in [
                "mysql",
                "sys",
                "INFORMATION_SCHEMA",
                "PERFORMANCE_SCHEMA",
                "METRICS_SCHEMA",
                "INSPECTION_SCHEMA",
            ] {
                deny.push(s.to_string());
            }
        } else if let Some(rest) = p.strip_prefix('!') {
            deny.push(rest.to_string());
        } else {
            allow.push(p.clone());
        }
    }
    if allow.is_empty() {
        allow.push("*.*".into());
    }
    Ok(Arc::new(PatternFilter {
        allow,
        deny_schemas: deny,
    }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// 备份压缩类型枚举（桩仅保留常量）。
// 备份压缩类型枚举（桩仅保留常量）。
pub enum CompressType {
    #[default]
    NoCompression = 0,
    Gzip,
    Snappy,
    Zstd,
    Lzo,
}
// 默认不压缩。
pub const COMPRESS_NONE: CompressType = CompressType::NoCompression;

#[derive(Clone, Debug, Default)]
// 外部存储 backend 选项占位 struct。
// 外部存储 backend 选项占位 struct。
pub struct BackendOptions {}

// 顺序写字节的 sink，Close 提交缓冲。
pub trait ObjectWriter: Send {
    // Write 返回已写入字节数。
    fn Write(&mut self, data: &[u8]) -> Result<usize>;
    // Close 刷盘或提交内存缓冲。
    fn Close(&mut self) -> Result<()>;
}

// 外部存储抽象：WriteFile/ReadFile/Create/URI。
pub trait Storage: Send + Sync {
    // WriteFile 一次性写完整文件；Create 返回流式 Writer。
    // 覆盖或新建内存文件。
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()>;
    // 读取内存文件，缺失返回 file not found。
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>>;
    // 创建 MemWriter，Close 时写入 map。
    fn Create(&self, name: &str) -> Result<Box<dyn ObjectWriter>>;
    fn CreateWithOptions(
        &self,
        name: &str,
        _option: Option<&astersql_objstore_storeapi::WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        self.Create(name)
    }
    // 本地或逻辑根路径。
    fn FilePath(&self) -> String;
    // 生成 file:// URI，供日志与 manifest 使用。
    fn URI(&self) -> String {
        let p = self.FilePath();
        if p.starts_with("file:") {
            p
        } else {
            format!("file://{p}")
        }
    }
}

#[derive(Clone, Default)]
// 内存 map 实现的 Storage，单测默认后端。
// 内存 map 实现的 Storage，单测默认后端。
pub struct MemStorage {
    pub files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub path: String,
}
// 内存文件 CRUD。
impl MemStorage {
    // path 仅用于 FilePath/URI，不参与 map 键。
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            path: path.into(),
        }
    }
}
// 内存文件 CRUD。
impl Storage for MemStorage {
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| errors_new(format!("file not found: {name}")))
    }
    fn Create(&self, name: &str) -> Result<Box<dyn ObjectWriter>> {
        Ok(Box::new(MemWriter {
            files: self.files.clone(),
            name: name.to_string(),
            buf: Vec::new(),
        }))
    }
    fn FilePath(&self) -> String {
        self.path.clone()
    }
}

// MemStorage.Create 返回的缓冲 Writer，Close 时落盘到 map。
// MemStorage.Create 返回的缓冲 Writer，Close 时落盘到 map。
pub struct MemWriter {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    name: String,
    buf: Vec<u8>,
}
// MemWriter 缓冲写入，Close 提交。
impl ObjectWriter for MemWriter {
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn Close(&mut self) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(self.name.clone(), self.buf.clone());
        Ok(())
    }
}

// --- prometheus / promutil 指标桩 ---
// --- prometheus / promutil stubs ---

#[derive(Clone, Debug, Default)]
// Prometheus 标签 map 替身。
pub struct Labels(pub HashMap<String, String>);

#[derive(Clone, Debug, Default)]
// 原子 f64 计数器，Add/Inc 用 CAS 更新 bits。
// 原子 f64 计数器，Add/Inc 用 CAS 更新 bits。
pub struct Counter {
    // 用 AtomicU64 存 f64.to_bits()，避免浮点原子类型缺失。
    pub v: Arc<AtomicU64>, // store as bits of f64 via to_bits
}
// CAS 循环更新 f64 bits。
// CAS 循环更新 f64 bits。
impl Counter {
    pub fn new() -> Self {
        Self {
            v: Arc::new(AtomicU64::new(0f64.to_bits())),
        }
    }
    // CAS 循环累加，避免 lost update。
    pub fn Add(&self, x: f64) {
        loop {
            let cur = self.v.load(Ordering::SeqCst);
            let n = f64::from_bits(cur) + x;
            if self
                .v
                .compare_exchange(cur, n.to_bits(), Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
    }
    // Add(1.0) 快捷方式。
    pub fn Inc(&self) {
        self.Add(1.0);
    }
    // 读取当前计数值。
    pub fn get(&self) -> f64 {
        f64::from_bits(self.v.load(Ordering::SeqCst))
    }
}

#[derive(Clone, Debug, Default)]
// 原子 f64  gauge，支持 Add/Sub/Inc/Dec。
// 原子 f64 gauge，支持 Add/Sub/Inc/Dec。
pub struct Gauge {
    pub v: Arc<AtomicU64>,
}
// 原子读写 f64 gauge 值。
// 原子读写 f64 gauge 值。
impl Gauge {
    pub fn new() -> Self {
        Self {
            v: Arc::new(AtomicU64::new(0f64.to_bits())),
        }
    }
    // 从 AtomicU64 解码 f64。
    fn bits(&self) -> f64 {
        f64::from_bits(self.v.load(Ordering::SeqCst))
    }
    // 写入 f64 的 bit 表示。
    fn set_bits(&self, x: f64) {
        self.v.store(x.to_bits(), Ordering::SeqCst);
    }
    pub fn Add(&self, x: f64) {
        self.set_bits(self.bits() + x);
    }
    pub fn Sub(&self, x: f64) {
        self.set_bits(self.bits() - x);
    }
    pub fn Inc(&self) {
        self.Add(1.0);
    }
    pub fn Dec(&self) {
        self.Sub(1.0);
    }
    pub fn get(&self) -> f64 {
        self.bits()
    }
}

#[derive(Clone, Debug, Default)]
// 简化为 sum 累加，Observe 追加样本值。
// 简化为 sum 累加，Observe 追加样本值。
pub struct Histogram {
    pub sum: Arc<AtomicU64>,
}
// Observe 累加样本到 sum。
// Observe 累加样本到 sum。
impl Histogram {
    pub fn new() -> Self {
        Self {
            sum: Arc::new(AtomicU64::new(0f64.to_bits())),
        }
    }
    // 把样本值累加到 sum（非分桶 histogram）。
    pub fn Observe(&self, v: f64) {
        loop {
            let cur = self.sum.load(Ordering::SeqCst);
            let n = f64::from_bits(cur) + v;
            if self
                .sum
                .compare_exchange(cur, n.to_bits(), Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
// 忽略 label 的 Counter 向量桩。
// 忽略 label 的 Counter 向量桩。
pub struct CounterVec {
    pub inner: Counter,
}
#[derive(Clone, Debug, Default)]
// 忽略 label 的 Gauge 向量桩。
// 忽略 label 的 Gauge 向量桩。
pub struct GaugeVec {
    pub inner: Gauge,
}
#[derive(Clone, Debug, Default)]
// 忽略 label 的 Histogram 向量桩。
// 忽略 label 的 Histogram 向量桩。
pub struct HistogramVec {
    pub inner: Histogram,
}

// With 忽略 labels 返回 inner。
// With 忽略 labels 返回 inner。
impl CounterVec {
    // 桩实现忽略 label 选择，始终返回 inner。
    pub fn With(&self, _l: Option<&Labels>) -> &Counter {
        &self.inner
    }
}
// With 忽略 labels 返回 inner。
// With 忽略 labels 返回 inner。
impl GaugeVec {
    pub fn With(&self, _l: Option<&Labels>) -> &Gauge {
        &self.inner
    }
}
// With 忽略 labels 返回 inner。
// With 忽略 labels 返回 inner。
impl HistogramVec {
    pub fn With(&self, _l: Option<&Labels>) -> &Histogram {
        &self.inner
    }
}

// Prometheus metric 工厂接口。
pub trait Factory: Send + Sync {
    fn NewGaugeVec(&self, _opts_name: &str) -> GaugeVec;
    fn NewCounterVec(&self, _opts_name: &str) -> CounterVec;
    fn NewHistogramVec(&self, _opts_name: &str) -> HistogramVec;
}

#[derive(Clone, Debug, Default)]
// 返回独立 Counter/Gauge/Histogram 的默认工厂。
pub struct DefaultFactory;
// 各 Vec 均新建独立 inner metric。
impl Factory for DefaultFactory {
    fn NewGaugeVec(&self, _: &str) -> GaugeVec {
        GaugeVec {
            inner: Gauge::new(),
        }
    }
    fn NewCounterVec(&self, _: &str) -> CounterVec {
        CounterVec {
            inner: Counter::new(),
        }
    }
    fn NewHistogramVec(&self, _: &str) -> HistogramVec {
        HistogramVec {
            inner: Histogram::new(),
        }
    }
}

// Metric 注册表：MustRegister/Unregister。
pub trait Registry: Send + Sync {
    fn MustRegister(&self, _name: &str);
    fn Unregister(&self, _name: &str) -> bool;
}

#[derive(Clone, Debug, Default)]
// 仅记录已注册名称的 Registry 桩。
// 仅记录已注册名称的 Registry 桩。
pub struct DefaultRegistry {
    pub names: Arc<Mutex<Vec<String>>>,
}
// 名称列表式注册/注销。
impl Registry for DefaultRegistry {
    fn MustRegister(&self, name: &str) {
        self.names.lock().unwrap().push(name.to_string());
    }
    fn Unregister(&self, name: &str) -> bool {
        let mut g = self.names.lock().unwrap();
        if let Some(i) = g.iter().position(|n| n == name) {
            g.remove(i);
            true
        } else {
            false
        }
    }
}

// 构造 DefaultFactory 的 Arc<dyn Factory>。
pub fn NewDefaultFactory() -> Arc<dyn Factory> {
    Arc::new(DefaultFactory)
}
// 构造 DefaultRegistry 的 Arc<dyn Registry>。
pub fn NewDefaultRegistry() -> Arc<dyn Registry> {
    Arc::new(DefaultRegistry::default())
}

// --- 容量单位解析与格式化 ---
// --- units ---
// 1 MiB 常量，HumanSize/RAMInBytes 共用。
pub const MiB: i64 = 1024 * 1024;
// 把字节数格式化为 B/KB/MB/GB 人类可读串。
pub fn HumanSize(n: f64) -> String {
    if n < 1024.0 {
        return format!("{n:.0}B");
    }
    if n < 1048576.0 {
        return format!("{:.1}KB", n / 1024.0);
    }
    if n < 1073741824.0 {
        return format!("{:.1}MB", n / 1048576.0);
    }
    format!("{:.1}GB", n / 1073741824.0)
}
// 解析 KiB/MiB/KB/MB 等带单位容量字符串为字节。
pub fn RAMInBytes(s: &str) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    let lower = s.to_ascii_lowercase();
    let (num, mul) = if let Some(rest) = lower.strip_suffix("kib") {
        (rest, 1024i64)
    } else if let Some(rest) = lower.strip_suffix("mib") {
        (rest, 1024 * 1024)
    } else if let Some(rest) = lower.strip_suffix("gib") {
        (rest, 1024 * 1024 * 1024)
    } else if let Some(rest) = lower.strip_suffix("kb") {
        (rest, 1000i64)
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest, 1000 * 1000)
    } else if let Some(rest) = lower.strip_suffix("gb") {
        (rest, 1000 * 1000 * 1000)
    } else if let Some(rest) = lower.strip_suffix(['b', 'B']) {
        let rest = rest.trim();
        if let Some(r) = rest.strip_suffix(['k', 'K']) {
            (r, 1024i64)
        } else if let Some(r) = rest.strip_suffix(['m', 'M']) {
            (r, 1024 * 1024)
        } else if let Some(r) = rest.strip_suffix(['g', 'G']) {
            (r, 1024 * 1024 * 1024)
        } else {
            (rest, 1)
        }
    } else {
        (lower.as_str(), 1)
    };
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| errors_new(format!("invalid size: {s}")))?;
    Ok((v * mul as f64) as i64)
}

// --- failpoint 空实现 ---
// --- failpoint no-op ---
// failpoint 桩：恒 false，不触发注入。
pub fn failpoint_inject(_name: &str) -> bool {
    false
}

// --- 可重试错误判定（dbutil 子集）---
// --- dbutil ---
// 根据 MySQL 错误码或消息关键字判断是否可重试。
pub fn IsRetryableError(err: &Error) -> bool {
    if let Some(m) = &err.mysql {
        // 常见可重试 MySQL 错误码：1205/1213/2013/2006/1105。
        matches!(m.Number, 1205 | 1213 | 2013 | 2006 | 1105)
    } else {
        // 非 MySQL 错误则退化为消息关键字匹配。
        let s = err.msg.to_ascii_lowercase();
        s.contains("connection") || s.contains("timeout") || s.contains("deadlock")
    }
}

// --- Parquet 列元数据常量 ---
// --- parquet ColumnInfo ---
#[derive(Clone, Debug, Default)]
// Parquet/表元数据列描述，字段名与 Go model.ColumnInfo 一致。
// Parquet/表元数据列描述，字段名与 Go model.ColumnInfo 一致。
pub struct ColumnInfo {
    pub Name: String,
    pub DatabaseTypeName: String,
    pub Nullable: bool,
    pub Precision: i64,
    pub Scale: i64,
}

// Parquet 默认压缩：无。
pub const DefaultCompressionType: CompressType = CompressType::NoCompression;
// Parquet row group 内存上限默认值。
pub const DefaultRowGroupMemoryLimitBytes: i64 = 120 * 1024 * 1024;

// --- 输出路径模板（Go text/template 子集）---
// --- output template (minimal Go text/template subset used by dumpling) ---
#[derive(Clone, Debug)]
// 输出文件名模板：内置 schema/table/data 等 define。
// 输出文件名模板：内置 schema/table/data 等 define。
pub struct OutputTemplate {
    pub text: String,
    // defines 存各模板段（schema/table/data 等）的 pattern。
    pub defines: HashMap<String, String>,
}
// 默认使用 dumpling 内置 define 集。
impl Default for OutputTemplate {
    fn default() -> Self {
        Self::default_dumpling()
    }
}
// Parse 可覆盖 data define；Execute 做简单占位符替换。
// Parse 可覆盖 data define；Execute 做简单占位符替换。
impl OutputTemplate {
    // 内置 dumpling 默认 schema/table/view/data/policy 路径模式。
    pub fn default_dumpling() -> Self {
        let mut defines = HashMap::new();
        defines.insert("schema".into(), "{{fn .DB}}-schema-create".into());
        defines.insert("table".into(), "{{fn .DB}}.{{fn .Table}}-schema".into());
        defines.insert("view".into(), "{{fn .DB}}.{{fn .Table}}-schema-view".into());
        defines.insert(
            "sequence".into(),
            "{{fn .DB}}.{{fn .Table}}-schema-sequence".into(),
        );
        defines.insert("data".into(), "{{fn .DB}}.{{fn .Table}}.{{.Index}}".into());
        defines.insert(
            "placement-policy".into(),
            "{{fn .Policy}}-placement-policy-create".into(),
        );
        Self {
            text: String::new(),
            defines,
        }
    }
    // 深拷贝 defines 与 text。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    // 解析用户模板，可覆盖 data define。
    pub fn Parse(&mut self, text: &str) -> Result<()> {
        // 若模板文本含 define "data" 或 result.{{.Index}} 则覆盖 data 路径。
        if text.contains("define \"data\"") || text.contains("result.{{.Index}}") {
            if text.contains("result.{{.Index}}") {
                self.defines
                    .insert("data".into(), "result.{{.Index}}".into());
            }
        }
        // 无 define 的非空文本整体视为 data 文件名模式。
        if !text.contains("define") && !text.is_empty() {
            self.defines.insert("data".into(), text.to_string());
        }
        self.text = text.to_string();
        Ok(())
    }
    // 按 define 名展开 DB/Table/Index/Policy 占位符。
    pub fn Execute(
        &self,
        name: &str,
        db: &str,
        table: &str,
        index: &str,
        policy: &str,
    ) -> Result<String> {
        let tmpl = self
            .defines
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string());
        let mut out = tmpl;
        out = out.replace("{{fn .DB}}", &fn_escape(db));
        out = out.replace("{{fn .Table}}", &fn_escape(table));
        out = out.replace("{{fn .Policy}}", &fn_escape(policy));
        out = out.replace("{{.DB}}", db);
        out = out.replace("{{.Table}}", table);
        out = out.replace("{{.Index}}", index);
        out = out.replace("{{.Policy}}", policy);
        // objectName 类占位已在 defines 默认值内联展开。
        Ok(out)
    }
}

// 对象存储路径安全转义：非法字符变为 %XX。
fn fn_escape(input: &str) -> String {
    let mut out = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let bad = matches!(
            c,
            0..=0x1f | b'%' | b'"' | b'*' | b'.' | b'/' | b':' | b'<' | b'>' | b'?' | b'\\' | b'|'
        ) || (c == b'-' && input[i..].to_ascii_lowercase().starts_with("-schema"));
        if bad {
            out.push_str(&format!("%{c:02X}"));
            i += 1;
            // 仅对可能歧义的 -schema 前缀做转义，其余字符按规则处理。
        } else {
            let ch = input[i..]
                .chars()
                .next()
                .expect("i always points at a UTF-8 character boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

// 对象名转义，委托 fn_escape 处理非法字符。
pub fn filename_escape(input: &str) -> String {
    fn_escape(input)
}

// --- BR summary 桩 ---
// --- summary stub ---
// BR summary 桩：无操作。
pub fn summary_set_success_status(_ok: bool) {}
// BR summary 收集桩：无操作。
pub fn summary_collect() {}

// --- HTTP 状态服务桩 ---
// --- HTTP service stub ---
#[derive(Clone, Debug, Default)]
// HTTP 状态服务桩：不绑定真实端口，invalid 地址返回错。
// HTTP 状态服务桩：不绑定真实端口，invalid 地址返回错。
pub struct HttpServiceHandle {
    pub addr: String,
    pub started: Arc<AtomicBool>,
    pub stopped: Arc<AtomicBool>,
}
// start 记录 addr；stop 置 stopped 标志。
// start 记录 addr；stop 置 stopped 标志。
impl HttpServiceHandle {
    pub fn start(addr: &str) -> Result<Self> {
        // 单测不监听真实端口，仅记录 addr 与 started 状态。
        if addr.contains("invalid") {
            return Err(errors_new(format!("start listening: invalid addr {addr}")));
        }
        Ok(Self {
            addr: addr.to_string(),
            started: Arc::new(AtomicBool::new(true)),
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }
    // 标记 stopped，配合 isErrNetClosing 测试。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

// 判断是否为“连接已关闭”类网络错误。
pub fn isErrNetClosing(err: &Error) -> bool {
    err.msg.contains("use of closed network connection")
}

// --- etcd / PD 客户端桩 ---
// --- etcd / pd stubs ---
#[derive(Clone, Debug, Default)]
pub struct EtcdClientConfig {
    pub endpoints: Vec<String>,
    pub dial_timeout: Duration,
    pub auto_sync_interval: Duration,
}

#[derive(Clone, Debug, Default)]
// etcd 客户端桩：保留 Go clientv3 的构造配置与读取错误边界，
// 并用内存 HashMap 模拟前缀读取。
pub struct EtcdClient {
    pub config: EtcdClientConfig,
    pub kvs: Arc<Mutex<HashMap<String, String>>>,
    pub get_error: Arc<Mutex<Option<Error>>>,
    pub last_get_timeout: Arc<Mutex<Option<Duration>>>,
}
impl EtcdClient {
    pub fn New(config: EtcdClientConfig) -> Result<Self> {
        if config.endpoints.is_empty() {
            return Err(errors_new("etcdclient: no available endpoints"));
        }
        Ok(Self {
            config,
            ..Self::default()
        })
    }

    pub fn GetPrefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        self.GetPrefixWithTimeout(prefix, Duration::MAX)
    }

    pub fn GetPrefixWithTimeout(
        &self,
        prefix: &str,
        timeout: Duration,
    ) -> Result<Vec<(String, String)>> {
        *self.last_get_timeout.lock().unwrap() = Some(timeout);
        if let Some(err) = self.get_error.lock().unwrap().clone() {
            return Err(err);
        }
        let g = self.kvs.lock().unwrap();
        Ok(g.iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

#[derive(Clone, Debug, Default)]
// PD GC barrier 元信息，与 Go gcstates 结构对齐。
// PD GC barrier 元信息，与 Go gcstates 结构对齐。
pub struct GCBarrierInfo {
    pub BarrierID: String,
    pub BarrierTS: u64,
    pub TTL: Duration,
}

#[derive(Clone, Debug, Default)]
// 可注入 SetGCBarrier 错误的 GC states 客户端 mock。
// 可注入 SetGCBarrier 错误的 GC states 客户端 mock。
pub struct mockGCStatesClient {
    pub set_barrier_err: Arc<Mutex<Option<Error>>>,
    pub set_calls: Arc<AtomicU64>,
    pub del_calls: Arc<AtomicU64>,
    pub last_barrier: Arc<Mutex<Option<GCBarrierInfo>>>,
}
// Set/Delete GC barrier 计数与错误注入。
// Set/Delete GC barrier 计数与错误注入。
impl mockGCStatesClient {
    // 记录 barrier 并可选返回注入错误。
    pub fn SetGCBarrier(
        &self,
        barrier_id: &str,
        barrier_ts: u64,
        ttl: Duration,
    ) -> Result<GCBarrierInfo> {
        self.set_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = self.set_barrier_err.lock().unwrap().clone() {
            return Err(err);
        }
        let info = GCBarrierInfo {
            BarrierID: barrier_id.to_string(),
            BarrierTS: barrier_ts,
            TTL: ttl,
        };
        *self.last_barrier.lock().unwrap() = Some(info.clone());
        Ok(info)
    }
    // 删除 barrier 桩：递增 del_calls 并返回 None。
    pub fn DeleteGCBarrier(&self, _barrier_id: &str) -> Result<Option<GCBarrierInfo>> {
        self.del_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

#[derive(Clone, Debug, Default)]
// PD 客户端桩：记录 safe point 调用参数，支持 mock GC。
// PD 客户端桩：记录 safe point 调用参数，支持 mock GC。
pub struct PdClient {
    // closed 标记客户端已 Close。
    // update_safe_point_* 记录 UpdateServiceGCSafePoint 调用。
    // gc_states_client 嵌套 GC barrier mock。
    pub closed: Arc<AtomicBool>,
    pub update_safe_point_calls: Arc<AtomicU64>,
    pub update_safe_point_err: Arc<Mutex<Option<Error>>>,
    pub last_safe_point_service_id: Arc<Mutex<String>>,
    pub last_safe_point_ttl: Arc<Mutex<i64>>,
    pub last_safe_point_ts: Arc<Mutex<u64>>,
    pub gc_states_client: Arc<mockGCStatesClient>,
}
// UpdateServiceGCSafePoint 与 GetGCStatesClient。
// UpdateServiceGCSafePoint 与 GetGCStatesClient。
impl PdClient {
    // 构造全默认 mock，测试按需设置 err 字段。
    pub fn new_mock() -> Self {
        Self::default()
    }
    // 更新 GC safe point：记录参数并可选失败。
    pub fn UpdateServiceGCSafePoint(
        &self,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64> {
        self.update_safe_point_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_safe_point_service_id.lock().unwrap() = service_id.to_string();
        *self.last_safe_point_ttl.lock().unwrap() = ttl;
        *self.last_safe_point_ts.lock().unwrap() = safe_point;
        if let Some(err) = self.update_safe_point_err.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(safe_point)
    }
    // 返回共享的 mockGCStatesClient。
    pub fn GetGCStatesClient(&self, _keyspace_id: u32) -> Arc<mockGCStatesClient> {
        self.gc_states_client.clone()
    }
    // 标记 PD 客户端已关闭。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

// 打开 MySQL 连接桩：忽略 DSN 返回空 DB。
pub fn openDB(_cfg: &MysqlConfig) -> Result<DB> {
    Ok(DB::new())
}

// 合并多个 Error 消息为一条，对应 Go multierr。
pub fn multierr_combine(errs: Vec<Error>) -> Option<Error> {
    if errs.is_empty() {
        None
    } else {
        Some(Error::new(
            errs.into_iter()
                .map(|e| e.msg)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

impl From<astersql_objstore_storeapi::ExceedMaxUploadParts> for Error {
    fn from(error: astersql_objstore_storeapi::ExceedMaxUploadParts) -> Self {
        Self {
            msg: error.to_string(),
            mysql: None,
            exceed_upload_parts: true,
        }
    }
}
