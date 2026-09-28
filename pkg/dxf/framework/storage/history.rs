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
pub const historyTaskSummaryColumns: &str = "t.id, t.task_key, t.type, t.state, t.step, t.priority, t.concurrency, t.create_time, t.target_scope, t.max_node_count, t.extra_params, t.keyspace, t.start_time, t.state_update_time, t.end_time";

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
    /// 事务内：刷新 meta → 插入任务 history → 删除原任务 → 逐个搬迁子任务。
    pub fn TransferTasks2History(
        &self,
        ctx: Context,
        tasks: Vec<proto::Task>,
    ) -> Result<(), Error> {
        if tasks.is_empty() {
            return Ok(());
        }

        let taskIDStrs: Vec<String> = tasks.iter().map(|task| task.ID.to_string()).collect();
        injectfailpoint::DXFRandomErrorWithOnePercent()?;

        self.WithNewTxn(ctx.clone(), |se| {
            // Go 在转入历史表前先刷新 meta，避免 history 中留下已被脱敏的 meta。
            let exec = se.GetSQLExecutor();
            for t in &tasks {
                sqlexec::ExecSQL(
                    ctx.clone(),
                    exec.clone(),
                    "update mysql.tidb_global_task set meta= %?, state_update_time = CURRENT_TIMESTAMP() where id = %?",
                    vec![t.Meta.clone().into(), t.ID.into()],
                )?;
            }

            let ids = taskIDStrs.join(", ");
            sqlexec::ExecSQL(
                ctx.clone(),
                exec.clone(),
                format!(
                    "insert into mysql.tidb_global_task_history select * from mysql.tidb_global_task where id in({})",
                    ids
                ),
                vec![],
            )?;
            sqlexec::ExecSQL(
                ctx.clone(),
                exec.clone(),
                format!("delete from mysql.tidb_global_task where id in({})", ids),
                vec![],
            )?;

            for t in &tasks {
                self.TransferSubtasks2HistoryWithSession(ctx.clone(), se.clone(), t.ID)?;
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
/// 单条历史任务摘要：TaskBase + start/update/end 时间。
pub struct HistoryTaskSummary {
    /// 任务基础字段（来自 history 表前 12 列）。
    pub TaskBase: proto::TaskBase,
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
/// 将 history 表行转为 HistoryTaskSummary（列 12/13/14 为时间字段）。
fn row2HistoryTaskSummary(r: chunk::Row) -> HistoryTaskSummary {
    let mut item = HistoryTaskSummary {
        TaskBase: row2TaskBasic(r.clone()),
        StartTime: std::time::UNIX_EPOCH,
        StateUpdateTime: std::time::UNIX_EPOCH,
        EndTime: std::time::UNIX_EPOCH,
    };
    // Go 下标 12/13/14 分别是 start/state_update/end；空值保持零值时间。
    if !r.IsNull(12) {
        item.StartTime = r.GetTime(12).GoTime(time::Local).0;
    }
    if !r.IsNull(13) {
        item.StateUpdateTime = r.GetTime(13).GoTime(time::Local).0;
    }
    if !r.IsNull(14) {
        item.EndTime = r.GetTime(14).GoTime(time::Local).0;
    }
    item
}
