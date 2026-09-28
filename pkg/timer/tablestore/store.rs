// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 基于系统表的 Timer 存储实现（TableTimerStore）。
//
// 通过 `syssession` 连接池执行 SQL CRUD，并用 etcd 或内存 notifier 广播
// Create/Update/Delete 监视事件；会话内强制 UTC 时区以保证时间列一致。

use crate::notifier::{EtcdClient, NewEtcdNotifier};
use crate::sql::{
    EventExtObj, ManualRequestObj, SqlArg, buildDeleteTimerSQL, buildInsertTimerSQL,
    buildSelectTimerSQL, buildUpdateTimerSQL, decode_timer_ext, indentString,
};
use astersql_session_syssession as syssession;
use astersql_timer_api as api;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::time::Duration;

/// 查询结果单元格的类型化取值。
#[derive(Clone, Debug, PartialEq)]
pub enum SqlCell {
    Null,
    String(String),
    Bytes(Vec<u8>),
    I64(i64),
    U64(u64),
    Bool(bool),
    Timestamp(api::Timestamp),
    Json(String),
}

/// 一行查询结果，按列下标访问。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlRow(pub Vec<SqlCell>);

impl SqlRow {
    /// 列数。
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// 指定列是否为 NULL 或越界。
    pub fn is_null(&self, index: usize) -> bool {
        matches!(self.0.get(index), Some(SqlCell::Null) | None)
    }
    /// 按字符串取出；类型不符则报错。
    pub fn string(&self, index: usize) -> api::TimerResult<String> {
        match self.0.get(index) {
            Some(SqlCell::String(value)) => Ok(value.clone()),
            other => Err(cell_error(index, "string", other)),
        }
    }
    /// 按字节取出。
    pub fn bytes(&self, index: usize) -> api::TimerResult<Vec<u8>> {
        match self.0.get(index) {
            Some(SqlCell::Bytes(value)) => Ok(value.clone()),
            other => Err(cell_error(index, "bytes", other)),
        }
    }
    /// 按 i64 取出；兼容 u64/bool 窄转换。
    pub fn i64(&self, index: usize) -> api::TimerResult<i64> {
        match self.0.get(index) {
            Some(SqlCell::I64(value)) => Ok(*value),
            Some(SqlCell::U64(value)) => {
                i64::try_from(*value).map_err(|_| api::TimerError::message("integer overflow"))
            }
            Some(SqlCell::Bool(value)) => Ok(i64::from(*value)),
            other => Err(cell_error(index, "i64", other)),
        }
    }
    /// 按 u64 取出；负 i64 报错。
    pub fn u64(&self, index: usize) -> api::TimerResult<u64> {
        match self.0.get(index) {
            Some(SqlCell::U64(value)) => Ok(*value),
            Some(SqlCell::I64(value)) => u64::try_from(*value)
                .map_err(|_| api::TimerError::message("negative unsigned integer")),
            other => Err(cell_error(index, "u64", other)),
        }
    }
    /// 按时间戳取出；i64 按 Unix 秒解释。
    pub fn timestamp(&self, index: usize) -> api::TimerResult<api::Timestamp> {
        match self.0.get(index) {
            Some(SqlCell::Timestamp(value)) => Ok(*value),
            Some(SqlCell::I64(value)) => Ok(timestamp_from_unix(*value)),
            other => Err(cell_error(index, "timestamp", other)),
        }
    }
    /// 按 JSON 文本取出；字符串列也可当作 JSON。
    pub fn json(&self, index: usize) -> api::TimerResult<String> {
        match self.0.get(index) {
            Some(SqlCell::Json(value)) => Ok(value.clone()),
            Some(SqlCell::String(value)) => Ok(value.clone()),
            other => Err(cell_error(index, "json", other)),
        }
    }
}

/// 将 Unix 秒转为相对当前整秒的 `Timestamp`。
fn timestamp_from_unix(value: i64) -> api::Timestamp {
    let now = api::now_timestamp();
    let base = now - Duration::from_nanos(now.timestamp_subsec_nanos() as u64);
    let current = base.timestamp();
    if value >= current {
        base + Duration::from_secs((value - current) as u64)
    } else {
        base - Duration::from_secs((current - value) as u64)
    }
}

/// 构造列类型不匹配错误。
fn cell_error(index: usize, expected: &str, actual: Option<&SqlCell>) -> api::TimerError {
    api::TimerError::message(format!(
        "column {index} expected {expected}, got {actual:?}"
    ))
}

/// 将会话时区解出的时间值转换到 timer 自身 Location，对齐 Go `time.In`。
fn timestamp_in_location(value: api::Timestamp, location: &api::TimerLocation) -> api::Timestamp {
    match location {
        api::TimerLocation::Named(location) => value.with_timezone(location).fixed_offset(),
        api::TimerLocation::Fixed(location) => value.with_timezone(location),
    }
}

/// `executeSQL` 返回的多行结果集。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlResult {
    pub rows: Vec<SqlRow>,
}

/// TableTimerStore 的核心状态：会话池、表名与事件通知器。
pub(crate) struct TableTimerStoreCore {
    pub(crate) pool: Arc<dyn syssession::Pool + Send + Sync>,
    pub(crate) db_name: String,
    pub(crate) table_name: String,
    pub(crate) notifier: Arc<dyn api::TimerWatchEventNotifier>,
}

/// 创建基于系统表的 `TimerStore`；有 etcd 则用 etcd notifier，否则用内存 notifier。
pub fn NewTableTimerStore(
    cluster_id: u64,
    pool: Arc<dyn syssession::Pool + Send + Sync>,
    db_name: impl Into<String>,
    table_name: impl Into<String>,
    etcd: Option<Arc<dyn EtcdClient>>,
) -> api::TimerStore {
    let notifier = etcd
        .map(|client| NewEtcdNotifier(cluster_id, client))
        .unwrap_or_else(api::NewMemTimerWatchEventNotifier);
    api::TimerStore::from_core(TableTimerStoreCore {
        pool,
        db_name: db_name.into(),
        table_name: table_name.into(),
        notifier,
    })
}

impl api::TimerStoreCore for TableTimerStoreCore {
    /// 插入定时器：禁止调用方指定 ID/Version/CreateTime，成功后通知 Create 事件。
    fn Create(
        &self,
        _ctx: &api::Context,
        record: Option<api::TimerRecord>,
    ) -> api::TimerResult<String> {
        let record = record.ok_or_else(|| api::TimerError::message("timer should not be nil"))?;
        if !record.ID.is_empty() {
            return Err(api::TimerError::message(
                "ID should not be specified when create record",
            ));
        }
        if record.Version != 0 {
            return Err(api::TimerError::message(
                "Version should not be specified when create record",
            ));
        }
        if record.CreateTime.is_some() {
            return Err(api::TimerError::message(
                "CreateTime should not be specified when create record",
            ));
        }
        record.Validate()?;
        self.with_session(|session| {
            let (sql, args) = buildInsertTimerSQL(&self.db_name, &self.table_name, &record)?;
            executeSQL(session, &sql, args)?;
            // 自增主键作为 timer ID 返回。
            let rows = executeSQL(session, "select @@last_insert_id", vec![])?;
            let id = rows
                .first()
                .ok_or_else(|| api::TimerError::message("last_insert_id returned no rows"))?
                .u64(0)?
                .to_string();
            self.notifier.Notify(api::WatchTimerEventCreate, &id);
            Ok(id)
        })
    }

    /// 按条件列出定时器记录并解码为 `TimerRecord`。
    fn List(
        &self,
        _ctx: &api::Context,
        cond: Option<&dyn api::Cond>,
    ) -> api::TimerResult<Vec<api::TimerRecord>> {
        self.with_session(|session| {
            with_index_merge(session, || {
                let (sql, args) = buildSelectTimerSQL(&self.db_name, &self.table_name, cond)?;
                let rows = executeSQL(session, &sql, args)?;
                let tidb_time_zone = executeSQL(session, "SELECT @@global.time_zone", vec![])?
                    .first()
                    .filter(|row| row.len() > 0)
                    .ok_or_else(|| {
                        api::TimerError::message("failed to get TiDB global time zone of session")
                    })?
                    .string(0)?;
                rows.iter()
                    .map(|row| decode_timer(row, &tidb_time_zone))
                    .collect()
            })
        })
    }

    /// 在悲观事务中校验版本/事件 ID 后更新，并通知 Update 事件。
    fn Update(
        &self,
        _ctx: &api::Context,
        timer_id: &str,
        update: Option<api::TimerUpdate>,
    ) -> api::TimerResult<()> {
        let update = update.ok_or_else(|| api::TimerError::message("update should not be nil"))?;
        self.with_session(|session| runInTxn(session, |session| {
            // 先读当前 EVENT_ID/VERSION/调度策略，供乐观锁与策略校验。
            let sql = format!("SELECT EVENT_ID, VERSION, SCHED_POLICY_TYPE, SCHED_POLICY_EXPR FROM {} WHERE ID=%?", indentString(&self.db_name, &self.table_name));
            let rows = executeSQL(session, &sql, vec![SqlArg::String(timer_id.to_string())])?;
            let row = rows.first().ok_or(api::ErrTimerNotExist)?;
            checkUpdateConstraints(&update, &row.string(0)?, row.u64(1)?, &row.string(2)?, &row.string(3)?)?;
            let (sql, args) = buildUpdateTimerSQL(&self.db_name, &self.table_name, timer_id, &update)?;
            executeSQL(session, &sql, args)?;
            Ok(())
        }))?;
        self.notifier.Notify(api::WatchTimerEventUpdate, timer_id);
        Ok(())
    }

    /// 删除定时器；`ROW_COUNT()>0` 时通知 Delete。
    fn Delete(&self, _ctx: &api::Context, timer_id: &str) -> api::TimerResult<bool> {
        let deleted = self.with_session(|session| {
            let (sql, args) = buildDeleteTimerSQL(&self.db_name, &self.table_name, timer_id);
            executeSQL(session, &sql, args)?;
            let rows = executeSQL(session, "SELECT ROW_COUNT()", vec![])?;
            Ok(rows
                .first()
                .ok_or_else(|| api::TimerError::message("ROW_COUNT returned no rows"))?
                .i64(0)?
                > 0)
        })?;
        if deleted {
            self.notifier.Notify(api::WatchTimerEventDelete, timer_id);
        }
        Ok(deleted)
    }

    /// 表存储始终支持 Watch。
    fn WatchSupported(&self) -> bool {
        true
    }
    /// 订阅定时器变更事件通道。
    fn Watch(&self, ctx: &api::Context) -> api::WatchTimerChan {
        self.notifier.Watch(ctx)
    }
    /// 关闭底层 notifier。
    fn Close(&self) {
        self.notifier.Close();
    }
}

impl TableTimerStoreCore {
    /// 从池中取会话：先 ROLLBACK 清状态，强制 UTC，执行业务后恢复时区。
    pub(crate) fn with_session<T>(
        &self,
        operation: impl FnOnce(&syssession::Session) -> api::TimerResult<T>,
    ) -> api::TimerResult<T> {
        let mut operation = Some(operation);
        let mut output = None;
        let mut callback = |session: &syssession::Session| -> syssession::Result<()> {
            // 清掉可能残留的事务，再保存并切换到 UTC。
            executeSQL(session, "ROLLBACK", vec![]).map_err(session_error)?;
            let rows = executeSQL(session, "SELECT @@time_zone", vec![]).map_err(session_error)?;
            let original = rows
                .first()
                .filter(|row| row.len() > 0)
                .ok_or_else(|| {
                    syssession::SessionError::new("failed to get original time zone of session")
                })?
                .string(0)
                .map_err(session_error)?;
            executeSQL(session, "SET @@time_zone='UTC'", vec![]).map_err(session_error)?;
            let result = catch_unwind(AssertUnwindSafe(|| {
                operation.take().expect("session operation called once")(session)
            }));
            let rollback = executeSQL(session, "ROLLBACK", vec![]);
            // Go defer 在 ROLLBACK 失败后立即返回，不再尝试恢复时区。
            let restore = rollback.as_ref().ok().map(|_| {
                executeSQL(
                    session,
                    "SET @@time_zone=%?",
                    vec![SqlArg::String(original)],
                )
            });
            if rollback.is_err() || restore.as_ref().is_some_and(Result::is_err) {
                session.AvoidReuse();
            }
            match result {
                Ok(result) => {
                    // 清理错误只标记 AvoidReuse；Go 不覆盖业务回调的返回值。
                    let callback_result = result
                        .as_ref()
                        .map(|_| ())
                        .map_err(|error| session_error(error.clone()));
                    output = Some(result);
                    callback_result
                }
                Err(panic) => resume_unwind(panic),
            }
        };
        let pool_result = self.pool.WithSession(&mut callback);
        match output {
            Some(result) => result,
            None => Err(api::TimerError::message(
                pool_result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "session callback did not run".into()),
            )),
        }
    }
}

/// TiDB 的布尔系统变量既可能以 ON/OFF，也可能以 1/0 返回。
pub(crate) fn index_merge_is_disabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false"
    )
}

/// 对齐 Go `listWithSctx`：仅当原会话关闭了 index merge 时临时开启，并在
/// 正常返回、错误或 panic 后恢复关闭状态。恢复失败时丢弃该会话，避免状态泄漏。
fn with_index_merge<T>(
    session: &syssession::Session,
    operation: impl FnOnce() -> api::TimerResult<T>,
) -> api::TimerResult<T> {
    let rows = executeSQL(session, "SELECT @@tidb_enable_index_merge", vec![])?;
    let original = rows
        .first()
        .filter(|row| row.len() > 0)
        .ok_or_else(|| api::TimerError::message("failed to get index merge state of session"))?
        .string(0)?;
    let restore_disabled = index_merge_is_disabled(&original);
    if restore_disabled {
        executeSQL(session, "SET @@tidb_enable_index_merge=ON", vec![])?;
    }

    let result = catch_unwind(AssertUnwindSafe(operation));
    if restore_disabled && executeSQL(session, "SET @@tidb_enable_index_merge=OFF", vec![]).is_err()
    {
        session.AvoidReuse();
    }
    match result {
        Ok(result) => result,
        Err(panic) => resume_unwind(panic),
    }
}

/// 将 Timer 错误包装为会话错误。
fn session_error(error: api::TimerError) -> syssession::SessionError {
    syssession::SessionError::new(error.to_string())
}

/// 校验更新约束：事件 ID/版本乐观锁、时区与调度策略合法性。
pub fn checkUpdateConstraints(
    update: &api::TimerUpdate,
    event_id: &str,
    version: u64,
    policy: &str,
    expression: &str,
) -> api::TimerResult<()> {
    if update
        .CheckEventID
        .Get()
        .is_some_and(|value| event_id != value)
    {
        return Err(api::ErrEventIDNotMatch);
    }
    if update
        .CheckVersion
        .Get()
        .is_some_and(|value| version != *value)
    {
        return Err(api::ErrVersionNotMatch);
    }
    if let Some(time_zone) = update.TimeZone.Get() {
        api::ValidateTimeZone(time_zone)?;
    }
    // 若修改了调度策略类型或表达式，用合并后的最终值做合法性检查。
    let next_policy = update
        .SchedPolicyType
        .Get()
        .map(String::as_str)
        .unwrap_or(policy);
    let next_expression = update
        .SchedPolicyExpr
        .Get()
        .map(String::as_str)
        .unwrap_or(expression);
    if update.SchedPolicyType.Present() || update.SchedPolicyExpr.Present() {
        api::CreateSchedEventPolicy(next_policy, next_expression.to_string()).map_err(|error| {
            api::TimerError::message(format!(
                "schedule event configuration is not valid: {error}"
            ))
        })?;
    }
    Ok(())
}

/// 在会话上执行带参数 SQL，并将结果 downcast 为 `SqlResult` 行列表。
pub fn executeSQL(
    session: &syssession::Session,
    sql: &str,
    args: Vec<SqlArg>,
) -> api::TimerResult<Vec<SqlRow>> {
    let args = args
        .into_iter()
        .map(|value| Box::new(value) as syssession::SqlValue)
        .collect::<Vec<_>>();
    let result = session
        .ExecuteInternal(sql, &args)
        .map_err(|error| api::TimerError::message(error.to_string()))?;
    result
        .downcast::<SqlResult>()
        .map(|result| result.rows)
        .map_err(|_| api::TimerError::message("session returned an unsupported timer record set"))
}

/// 以悲观事务（BEGIN PESSIMISTIC）包裹操作；失败则 ROLLBACK。
pub fn runInTxn<T>(
    session: &syssession::Session,
    operation: impl FnOnce(&syssession::Session) -> api::TimerResult<T>,
) -> api::TimerResult<T> {
    executeSQL(session, "BEGIN PESSIMISTIC", vec![])?;
    match operation(session) {
        Ok(value) => {
            if let Err(error) = executeSQL(session, "COMMIT", vec![]) {
                let _ = executeSQL(session, "ROLLBACK", vec![]);
                Err(error)
            } else {
                Ok(value)
            }
        }
        Err(error) => {
            let _ = executeSQL(session, "ROLLBACK", vec![]);
            Err(error)
        }
    }
}

/// 将 SELECT 行按固定列序解码为 `TimerRecord`（含 TIMER_EXT 与时区 Location）。
fn decode_timer(row: &SqlRow, tidb_time_zone: &str) -> api::TimerResult<api::TimerRecord> {
    if row.len() < 19 {
        return Err(api::TimerError::message(
            "timer row has fewer than 19 columns",
        ));
    }
    let timezone = row.string(4)?;
    // 兼容 7.3.0：空或 "TIDB" 使用 TiDB 全局时区，而非当前主机时区。
    let parse_timezone = if timezone.is_empty() || timezone.eq_ignore_ascii_case("TIDB") {
        tidb_time_zone
    } else {
        &timezone
    };
    let location = api::parse_location(parse_timezone).or_else(|_| api::parse_location(""))?;
    let ext = if row.is_null(10) {
        Default::default()
    } else {
        decode_timer_ext(&row.json(10)?)?
    };
    Ok(api::TimerRecord {
        ID: row.u64(0)?.to_string(),
        TimerSpec: api::TimerSpec {
            Namespace: row.string(1)?,
            Key: row.string(2)?,
            Data: if row.is_null(3) {
                Vec::new()
            } else {
                row.bytes(3)?
            },
            TimeZone: timezone,
            SchedPolicyType: row.string(5)?,
            SchedPolicyExpr: row.string(6)?,
            HookClass: row.string(7)?,
            Watermark: if row.is_null(8) {
                None
            } else {
                Some(timestamp_in_location(row.timestamp(8)?, &location))
            },
            Enable: row.i64(9)? != 0,
            Tags: ext.tags,
        },
        ManualRequest: ext
            .manual
            .as_ref()
            .map(ManualRequestObj::ToManualRequest)
            .unwrap_or_default(),
        EventStatus: row.string(11)?,
        EventID: row.string(12)?,
        EventData: if row.is_null(13) {
            Vec::new()
        } else {
            row.bytes(13)?
        },
        EventStart: if row.is_null(14) {
            None
        } else {
            Some(timestamp_in_location(row.timestamp(14)?, &location))
        },
        EventExtra: ext
            .event
            .as_ref()
            .map(EventExtObj::ToEventExtra)
            .unwrap_or_default(),
        SummaryData: if row.is_null(15) {
            Vec::new()
        } else {
            row.bytes(15)?
        },
        CreateTime: if row.is_null(16) {
            None
        } else {
            Some(timestamp_in_location(row.timestamp(16)?, &location))
        },
        Version: row.u64(18)?,
        Location: Some(location),
    })
}
