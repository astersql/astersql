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

// 子任务（Subtask）状态流转相关的存储操作。
//
// 对应 Go 的 subtask_state.go：在 mysql.tidb_background_subtask 上
// 执行 start/finish/fail/cancel/pause/resume 等状态更新 SQL，
// 并保持 AffectedRows 校验与事务边界与 Go 一致。

// 从 pkg/dxf/framework/storage/subtask_state.go 迁移，保持子任务状态流转 SQL 一致。
//

impl TaskManager {
    // StartSubtask updates the subtask state to running.
    /// 将子任务置为 running，并校验 AffectedRows（归属 exec_id）非 0。
    pub fn StartSubtask(&self, ctx: Context, subtaskID: i64, execID: String) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            let vars = se.GetSessionVars();
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_background_subtask set state = %?, start_time = unix_timestamp(), state_update_time = unix_timestamp() where id = %? and exec_id = %?",
                vec![proto::SubtaskStateRunning.into(), subtaskID.into(), execID.clone().into()],
            )?;
            // Go 通过 AffectedRows 判断是否仍归该 exec_id 所有，失败映射为 ErrSubtaskNotFound。
            if vars.StmtCtx().AffectedRows() == 0 {
                return Err(ErrSubtaskNotFound.into());
            }
            Ok(())
        })
    }

    // FinishSubtask updates the subtask meta and mark state to succeed.
    /// 写入 meta 并将子任务标记为 succeed，同时更新 end_time。
    pub fn FinishSubtask(
        &self,
        ctx: Context,
        execID: String,
        id: i64,
        meta: Vec<u8>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set meta = %?, state = %?, state_update_time = unix_timestamp(), end_time = CURRENT_TIMESTAMP() where id = %? and exec_id = %?",
            vec![meta.into(), proto::SubtaskStateSucceed.into(), id.into(), execID.into()],
        )?;
        Ok(())
    }

    // FailSubtask update the task's subtask state to failed and set the err.
    /// 将当前节点上 pending/running 的一条子任务置为 failed 并写入 error。
    pub fn FailSubtask(
        &self,
        ctx: Context,
        execID: String,
        taskID: i64,
        err: Option<Error>,
    ) -> Result<(), Error> {
        if err.is_none() {
            return Ok(());
        }
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set state = %?, error = %?, start_time = unix_timestamp(), state_update_time = unix_timestamp(), end_time = CURRENT_TIMESTAMP() where exec_id = %? and task_key = %? and state in (%?, %?) limit 1;",
            vec![
                proto::SubtaskStateFailed.into(),
                serializeErr(err.unwrap()).into(),
                execID.into(),
                TaskIDToKey(taskID).into(),
                proto::SubtaskStatePending.into(),
                proto::SubtaskStateRunning.into(),
            ],
        )?;
        Ok(())
    }

    // CancelSubtask update the task's subtasks' state to canceled.
    /// 将该 exec_id + task 下 pending/running 子任务批量置为 canceled。
    pub fn CancelSubtask(&self, ctx: Context, execID: String, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set state = %?, start_time = unix_timestamp(), state_update_time = unix_timestamp(), end_time = CURRENT_TIMESTAMP() where exec_id = %? and task_key = %? and state in (%?, %?);",
            vec![
                proto::SubtaskStateCanceled.into(),
                execID.into(),
                TaskIDToKey(taskID).into(),
                proto::SubtaskStatePending.into(),
                proto::SubtaskStateRunning.into(),
            ],
        )?;
        Ok(())
    }

    // PauseSubtasks update all running/pending subtasks to pasued state.
    /// 将该节点上该任务的 running/pending 子任务置为 paused。
    pub fn PauseSubtasks(&self, ctx: Context, execID: String, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set state = \"paused\" where task_key = %? and state in (\"running\", \"pending\") and exec_id = %?",
            vec![TaskIDToKey(taskID).into(), execID.into()],
        )?;
        Ok(())
    }

    // ResumeSubtasks update all paused subtasks to pending state.
    /// 将 paused 子任务恢复为 pending，并清空 error。
    pub fn ResumeSubtasks(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set state = \"pending\", error = null where task_key = %? and state = \"paused\"",
            vec![TaskIDToKey(taskID).into()],
        )?;
        Ok(())
    }

    // RunningSubtasksBack2Pending implements the taskexecutor.TaskTable interface.
    /// 把仍标记为 running 的子任务按 id+exec_id CAS 回 pending（故障恢复）。
    pub fn RunningSubtasksBack2Pending(
        &self,
        ctx: Context,
        subtasks: Vec<proto::SubtaskBase>,
    ) -> Result<(), Error> {
        // 空列表跳过更新，沿用 Go 的快速返回。
        if subtasks.is_empty() {
            return Ok(());
        }
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            for subtask in &subtasks {
                sqlexec::ExecSQL(
                    ctx.clone(),
                    se.GetSQLExecutor(),
                    "update mysql.tidb_background_subtask set state = %?, state_update_time = unix_timestamp() where id = %? and exec_id = %? and state = %?",
                    vec![
                        proto::SubtaskStatePending.into(),
                        subtask.ID.into(),
                        subtask.ExecID.clone().into(),
                        proto::SubtaskStateRunning.into(),
                    ],
                )?;
            }
            Ok(())
        })
    }

    // UpdateSubtaskStateAndError updates the subtask state.
    /// 按 id+exec_id 更新子任务状态与错误字段。
    pub fn UpdateSubtaskStateAndError(
        &self,
        ctx: Context,
        execID: String,
        id: i64,
        state: proto::SubtaskState,
        subTaskErr: Option<Error>,
    ) -> Result<(), Error> {
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set state = %?, error = %?, state_update_time = unix_timestamp() where id = %? and exec_id = %?",
            vec![state.into(), serializeErrOption(subTaskErr).into(), id.into(), execID.into()],
        )?;
        Ok(())
    }
}
