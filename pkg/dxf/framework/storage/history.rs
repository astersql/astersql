// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 历史任务搬迁、分页查询与子任务历史 GC。
//
// 对应 Go 的 history.go：完成态任务从 tidb_global_task 转入
// tidb_global_task_history，子任务同步转入 history 表；列表使用 keyset
// 分页（按 id 降序），并提供过期子任务历史清理。

// 从 pkg/dxf/framework/storage/history.go 迁移，保持历史搬迁、分页查询和 GC 顺序一致。

// DefaultHistoryTaskPageSize is the default page size for history task listing.
/// 历史任务列表默认分页大小。
pub const DefaultHistoryTaskPageSize: i32 = 20;
// MinHistoryTaskPageSize is the minimum page size for history task listing.
/// 历史任务分页大小下限。
pub const MinHistoryTaskPageSize: i32 = 1;
// MaxHistoryTaskPageSize is the maximum page size for history task listing.
/// 历史任务分页大小上限。
pub const MaxHistoryTaskPageSize: i32 = 200;
// historyTaskSummaryColumns 对应 Go 中拼接 basicTaskColumns 的历史任务摘要列。
/// 历史任务摘要 SELECT 列清单（含 end_time）。
pub const historyTaskSummaryColumns: &str = "t.id, t.task_key, t.type, t.state, t.step, t.priority, t.concurrency, t.create_time, t.target_scope, t.max_node_count, t.extra_params, t.keyspace, t.error, t.start_time, t.state_update_time, t.end_time";

impl TaskManager {
    // TransferSubtasks2HistoryWithSession transfer the selected subtasks into tidb_background_subtask_history table by taskID.
    // Go 版本复用传入 session，先插入历史表再删除原表记录；这里保留 SQL 顺序和错误短路语义。
    /// 在给定 session 上将指定 task 的子任务插入 history 再删除原表记录。
    pub fn TransferSubtasks2HistoryWithSession(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        taskID: i64,
    ) -> Result<(), Error> {
        let exec = se.GetSQLExecutor();
        sqlexec::ExecSQL(
            ctx.clone(),
            exec.clone(),
            "insert into mysql.tidb_background_subtask_history select * from mysql.tidb_background_subtask where task_key = %?",
            vec![taskID.into()],
        )?;
        // 删除 taskID 对应的活跃 subtask；Go 中若删除失败直接返回该错误。
        sqlexec::ExecSQL(
            ctx,
            exec,
            "delete from mysql.tidb_background_subtask where task_key = %?",
            vec![taskID.into()],
        )?;
        Ok(())
    }

    // TransferTasks2History transfer the selected tasks into tidb_global_task_history table by taskIDs.
    /// 事务内批量刷新 meta、迁移任务，再按字符串 task_key 批量迁移子任务。
    pub fn TransferTasks2History(
        &self,
        ctx: Context,
        tasks: Vec<proto::Task>,
    ) -> Result<(), Error> {
        if tasks.is_empty() {
            return Ok(());
        }

        let ids = tasks.iter().map(|task| task.ID.to_string()).collect::<Vec<_>>().join(", ");
        // task_key is VARCHAR: quoted IDs preserve exact comparison above 2^53.
        let keys = tasks.iter().map(|task| format!("'{}'", task.ID)).collect::<Vec<_>>().join(", ");
        let mut update = String::from("update mysql.tidb_global_task set meta = case id");
        let mut args = Vec::with_capacity(tasks.len() * 2);
        for task in &tasks {
            update.push_str(" when %? then %?");
            args.push(task.ID.into());
            args.push(task.Meta.clone().into());
        }
        update.push_str(&format!(" end, state_update_time = CURRENT_TIMESTAMP() where id in({ids})"));
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            let exec = se.GetSQLExecutor();
            sqlexec::ExecSQL(ctx.clone(), exec.clone(), update, args)?;
            for sql in [
                format!("insert into mysql.tidb_global_task_history select * from mysql.tidb_global_task where id in({ids})"),
                format!("delete from mysql.tidb_global_task where id in({ids})"),
                format!("insert into mysql.tidb_background_subtask_history select * from mysql.tidb_background_subtask where task_key in({keys})"),
                format!("delete from mysql.tidb_background_subtask where task_key in({keys})"),
            ] {
                sqlexec::ExecSQL(ctx.clone(), exec.clone(), sql, vec![])?;
            }
            Ok(())
        })
    }

    // ListHistoryTasks lists history tasks with keyset pagination and optional keyspace filter.
    /// 按 keyspace 可选过滤，以 pageToken(id) 做 keyset 分页列出历史任务。
    pub fn ListHistoryTasks(
        &self,
        ctx: Context,
        pageSize: i32,
        pageToken: i64,
        keyspace: String,
    ) -> Result<HistoryTaskPage, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        ValidateHistoryTaskPageSize(pageSize)?;

        let mut whereParts: Vec<String> = Vec::with_capacity(2);
        let mut countArgs: Vec<Value> = Vec::with_capacity(1);
        if !keyspace.is_empty() {
            whereParts.push("t.keyspace = %?".to_string());
            countArgs.push(keyspace.clone().into());
        }

        let mut dataArgs = countArgs.clone();
        if pageToken > 0 {
            whereParts.push("t.id < %?".to_string());
            dataArgs.push(pageToken.into());
        }
        let whereSQL = if whereParts.is_empty() {
            String::new()
        } else {
            format!(" where {}", whereParts.join(" and "))
        };
        dataArgs.push((pageSize + 1).into());

        let mut rows = self.ExecuteSQLWithNewSession(
            ctx.clone(),
            format!(
                "select {} from mysql.tidb_global_task_history t{} order by t.id desc limit %?",
                historyTaskSummaryColumns, whereSQL
            ),
            dataArgs,
        )?;

        // 计数查询故意和分页查询分开：并发迁移时可能看到略有不同的快照，Go 逻辑接受这种观测误差。
        let countWhere = if keyspace.is_empty() {
            String::new()
        } else {
            " where keyspace = %?".to_string()
        };
        let countRows = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select count(1) from mysql.tidb_global_task_history{}",
                countWhere
            ),
            countArgs,
        )?;

        let mut page = HistoryTaskPage {
            Items: Vec::with_capacity(rows.len().min(pageSize as usize)),
            HasMore: rows.len() > pageSize as usize,
            NextPageToken: 0,
            ApproxTotalCount: countRows[0].GetInt64(0),
        };
        if page.HasMore {
            rows.truncate(pageSize as usize);
        }
        for row in rows {
            page.Items.push(row2HistoryTaskSummary(row));
        }
        if page.HasMore {
            page.NextPageToken = page
                .Items
                .last()
                .map(|item| item.TaskBase.ID)
                .unwrap_or_default();
        }
        Ok(page)
    }

    // GCSubtasks deletes the history subtask which is older than the given days.
    /// 删除超过保留天数的子任务历史记录（可用 failpoint 调整保留秒数）。
    pub fn GCSubtasks(&self, ctx: Context) -> Result<(), Error> {
        let mut subtaskHistoryKeepSeconds = defaultSubtaskKeepDays * 24 * 60 * 60;
        // Go failpoint 会修改保留秒数；只保留注入点的可变参数语义。
        failpoint::InjectCall("subtaskHistoryKeepSeconds", &mut subtaskHistoryKeepSeconds);
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "DELETE FROM mysql.tidb_background_subtask_history WHERE state_update_time < UNIX_TIMESTAMP() - {} ;",
                subtaskHistoryKeepSeconds
            ),
            vec![],
        )?;
        Ok(())
    }
}

// HistoryTaskSummary contains summary fields for one history task.
/// History task fields with safe error metadata and start/update/end times.
pub struct HistoryTaskSummary {
    /// 任务基础字段（来自 history 表前 12 列）。
    pub TaskBase: proto::TaskBase,
    /// Effective RFC error code; empty for a plain error.
    pub ErrorCode: String,
    /// Coarse category; never contains the raw error message.
    pub ErrorCategory: String,
    /// 任务开始时间。
    pub StartTime: time::Time,
    /// 状态最近更新时间。
    pub StateUpdateTime: time::Time,
    /// 任务结束时间（转入 history 时写入）。
    pub EndTime: time::Time,
}

// HistoryTaskPage is the paged result for history task listing.
/// 历史任务分页结果：条目、是否还有下一页、下一页 token、近似总数。
pub struct HistoryTaskPage {
    /// 当前页任务摘要列表。
    pub Items: Vec<HistoryTaskSummary>,
    /// 是否还有更多页（请求了 pageSize+1 行判定）。
    pub HasMore: bool,
    /// 下一页 token：本页最后一条任务的 ID。
    pub NextPageToken: i64,
    /// 近似总数（独立 count 查询，可能与分页快照略有偏差）。
    pub ApproxTotalCount: i64,
}

// ValidateHistoryTaskPageSize validates page size for history task listing.
/// 校验 pageSize 落在 [Min, Max] 区间。
pub fn ValidateHistoryTaskPageSize(pageSize: i32) -> Result<(), Error> {
    if pageSize < MinHistoryTaskPageSize || pageSize > MaxHistoryTaskPageSize {
        return Err(Error::new(format!(
            "page size should be within [{}, {}]",
            MinHistoryTaskPageSize, MaxHistoryTaskPageSize
        )));
    }
    Ok(())
}

// row2HistoryTaskSummary 对应 Go 的 chunk.Row 到历史任务摘要转换。
/// 将 history 表行转为 HistoryTaskSummary（列 12 为错误，13/14/15 为时间字段）。
fn row2HistoryTaskSummary(r: chunk::Row) -> HistoryTaskSummary {
    let mut item = HistoryTaskSummary {
        TaskBase: row2TaskBasic(r.clone()),
        ErrorCode: String::new(),
        ErrorCategory: String::new(),
        StartTime: std::time::UNIX_EPOCH,
        StateUpdateTime: std::time::UNIX_EPOCH,
        EndTime: std::time::UNIX_EPOCH,
    };
    if let Some(error) = row2TaskError(&r, 12) {
        item.ErrorCode = taskErrorCode(&error);
        item.ErrorCategory = ClassifyTaskError(item.TaskBase.State, Some(&error));
    }
    if !r.IsNull(13) {
        item.StartTime = r.GetTime(13).GoTime(time::Local).0;
    }
    if !r.IsNull(14) {
        item.StateUpdateTime = r.GetTime(14).GoTime(time::Local).0;
    }
    if !r.IsNull(15) {
        item.EndTime = r.GetTime(15).GoTime(time::Local).0;
    }
    item
}

/// Safe terminal error categories from Go commit 57b5f2268096ba75eee34c930d6464339460ced7.
pub fn ClassifyTaskError(state: proto::TaskState, error: Option<&Error>) -> String {
    let Some(error) = error else {
        return String::new();
    };
    match state {
        proto::TaskStateFailed => "failed",
        proto::TaskStateReverted if error.to_string().contains("cancelled by user") => "cancelled",
        proto::TaskStateReverted if isDataError(error) => "data-error",
        proto::TaskStateReverted => "failed",
        _ => "",
    }
    .to_owned()
}

fn isDataError(error: &Error) -> bool {
    let message = format!("[{}]{}", taskErrorCode(error), error);
    let import_data = message.contains("ErrEncodeKV")
        && (message.contains("Value conversion failed for column")
            || (message.contains("Check constraint '") && message.contains("' is violated"))
            || message.contains("Table has no partition for value"));
    let import_conflict = (message.contains("[executor:8167]")
        && message.contains("Duplicate key conflict found"))
        || (message.contains("ErrFoundDataConflictRecords")
            && message.contains("found data conflict records"))
        || (message.contains("ErrFoundIndexConflictRecords")
            && message.contains("found index conflict records"));
    import_data
        || import_conflict
        || (message.contains("[kv:1062]") && message.contains("Duplicate entry"))
}

fn taskErrorCode(error: &Error) -> String {
    if !error.RFCCode().is_empty() && error.RFCCode() != "0" {
        error.RFCCode().to_owned()
    } else if error.RFCCode().is_empty() && error.Code() != 0 {
        error.Code().to_string()
    } else {
        String::new()
    }
}
