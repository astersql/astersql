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

// 全局任务状态机写路径：取消、失败、回滚、暂停/恢复、修改参数、成功等。
//
// 各方法通过条件 UPDATE（带当前 state 谓词）推进 `mysql.tidb_global_task`，
// 避免并发 owner 切换时的状态竞争；部分操作在同一事务内同步更新 subtask。

// 从 pkg/dxf/framework/storage/task_state.go 迁移，保持任务状态流转与事务检查一致。
//

/// Cancellation marker shared by storage, schedulers and import completion.
pub const TaskCancelMessage: &str = "cancelled by user";

/// Recognize user cancellation even when the message is wrapped or annotated.
pub fn IsCancelledErr(error: Option<&dyn std::fmt::Display>) -> bool {
    error.is_some_and(|error| error.to_string().contains(TaskCancelMessage))
}

impl TaskManager {
    /// 将 pending/running/awaiting_resolution 任务标记为 cancelling（取消中）。
    // CancelTask cancels task.
    pub fn CancelTask(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state in (%?, %?, %?)",
            vec![
                proto::TaskStateCancelling.into(),
                taskID.into(),
                proto::TaskStatePending.into(),
                proto::TaskStateRunning.into(),
                proto::TaskStateAwaitingResolution.into(),
            ],
        )?;
        Ok(())
    }

    /// 在调用方提供的 session 上按 task_key 发起取消，避免另开会话。
    // CancelTaskByKeySession cancels task by key using input session.
    pub fn CancelTaskByKeySession(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        taskKey: String,
    ) -> Result<(), Error> {
        sqlexec::ExecSQL(
            ctx,
            se.GetSQLExecutor(),
            "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP() where task_key = %? and state in (%?, %?, %?)",
            vec![
                proto::TaskStateCancelling.into(),
                taskKey.into(),
                proto::TaskStatePending.into(),
                proto::TaskStateRunning.into(),
                proto::TaskStateAwaitingResolution.into(),
            ],
        )?;
        Ok(())
    }

    /// 在期望当前状态下将任务置为 failed，并写入序列化 error、结束时间。
    // FailTask implements the scheduler.TaskManager interface.
    pub fn FailTask(
        &self,
        ctx: Context,
        taskID: i64,
        currentState: proto::TaskState,
        taskErr: impl Into<Option<Error>>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let taskErr = taskErr.into();
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, error = %?, state_update_time = CURRENT_TIMESTAMP(), end_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
            vec![proto::TaskStateFailed.into(), serializeErrOption(taskErr).into(), taskID.into(), currentState.into()],
        )?;
        Ok(())
    }

    /// 将任务转入 reverting（回滚中），并记录导致回滚的错误。
    // RevertTask implements the scheduler.TaskManager interface.
    pub fn RevertTask(
        &self,
        ctx: Context,
        taskID: i64,
        taskState: proto::TaskState,
        taskErr: impl Into<Option<Error>>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.transitTaskStateOnErr(
            ctx,
            taskID,
            taskState,
            proto::TaskStateReverting,
            taskErr.into(),
        )
    }

    /// 内部 helper：在 currState 匹配时写入 targetState 与 error。
    // transitTaskStateOnErr 对应 Go 的内部 helper：带 error 字段更新目标状态。
    fn transitTaskStateOnErr(
        &self,
        ctx: Context,
        taskID: i64,
        currState: proto::TaskState,
        targetState: proto::TaskState,
        taskErr: Option<Error>,
    ) -> Result<(), Error> {
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, error = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
            vec![targetState.into(), serializeErrOption(taskErr).into(), taskID.into(), currState.into()],
        )?;
        Ok(())
    }

    /// 出错时暂停：任务进 pausing，失败 subtask 转 paused 并清空 end_time 以便 resume。
    // PauseTaskOnError updates task state to pausing with error and converts failed subtasks to paused.
    pub fn PauseTaskOnError(
        &self,
        ctx: Context,
        taskID: i64,
        taskState: proto::TaskState,
        step: proto::Step,
        taskErr: impl Into<Option<Error>>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let taskErr = taskErr.into();
        self.WithNewTxn(ctx.clone(), |se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, error = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
                vec![proto::TaskStatePausing.into(), serializeErrOption(taskErr).into(), taskID.into(), taskState.into()],
            )?;
            // AffectedRows==0 表示并发下任务状态已变，映射为 ErrTaskChanged。
            if se.GetSessionVars().StmtCtx().AffectedRows() == 0 {
                return Err(ErrTaskChanged.into());
            }
            // 失败 subtask 转 paused，同时清 end_time，方便后续 resume 继续执行。
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_background_subtask set state = %?, state_update_time = unix_timestamp(), end_time = null where task_key = %? and step = %? and state = %?",
                vec![proto::SubtaskStatePaused.into(), taskID.into(), step.into(), proto::SubtaskStateFailed.into()],
            )?;
            Ok(())
        })
    }

    /// 转入 awaiting_resolution（等待人工/策略决议，如不可自动恢复的冲突）。
    // AwaitingResolveTask implements the scheduler.TaskManager interface.
    pub fn AwaitingResolveTask(
        &self,
        ctx: Context,
        taskID: i64,
        taskState: proto::TaskState,
        taskErr: impl Into<Option<Error>>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.transitTaskStateOnErr(
            ctx,
            taskID,
            taskState,
            proto::TaskStateAwaitingResolution,
            taskErr.into(),
        )
    }

    /// 回滚完成：reverting → reverted，并写入 end_time。
    // RevertedTask implements the scheduler.TaskManager interface.
    pub fn RevertedTask(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP(), end_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
            vec![proto::TaskStateReverted.into(), taskID.into(), proto::TaskStateReverting.into()],
        )?;
        Ok(())
    }

    /// 按 task_key 将 pending/running 置为 pausing；返回是否命中行。
    // PauseTask pauses the task.
    pub fn PauseTask(&self, ctx: Context, taskKey: String) -> Result<bool, Error> {
        let mut found = false;
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewSession(|se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP() where task_key = %? and state in (%?, %?)",
                vec![proto::TaskStatePausing.into(), taskKey.clone().into(), proto::TaskStatePending.into(), proto::TaskStateRunning.into()],
            )?;
            if se.GetSessionVars().StmtCtx().AffectedRows() != 0 {
                found = true;
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// pausing → paused，表示暂停流程已落定。
    // PausedTask update the task state from pausing to paused.
    pub fn PausedTask(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
            vec![proto::TaskStatePaused.into(), taskID.into(), proto::TaskStatePausing.into()],
        )?;
        Ok(())
    }

    /// paused → resuming，并清空 error 字段；返回是否命中。
    // ResumeTask resumes the task.
    pub fn ResumeTask(&self, ctx: Context, taskKey: String) -> Result<bool, Error> {
        let mut found = false;
        self.WithNewSession(|se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, error = null, state_update_time = CURRENT_TIMESTAMP() where task_key = %? and state = %?",
                vec![proto::TaskStateResuming.into(), taskKey.clone().into(), proto::TaskStatePaused.into()],
            )?;
            if se.GetSessionVars().StmtCtx().AffectedRows() != 0 {
                found = true;
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// resuming → running，恢复调度执行。
    // ResumedTask implements the scheduler.TaskManager interface.
    pub fn ResumedTask(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_global_task set state = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
            vec![proto::TaskStateRunning.into(), taskID.into(), proto::TaskStateResuming.into()],
        )?;
        Ok(())
    }

    /// 进入 modifying：校验 PrevState 允许修改后写入 modify_params。
    // ModifyTaskByID modifies the task by the task ID.
    pub fn ModifyTaskByID(
        &self,
        ctx: Context,
        taskID: i64,
        param: proto::ModifyParam,
    ) -> Result<(), Error> {
        if !proto::TaskStateExt::CanMoveToModifying(&param.PrevState) {
            return Err(ErrTaskStateNotAllow.into());
        }
        let bytes = json::Marshal(&param).map_err(errors::Trace)?;
        self.WithNewTxn(ctx.clone(), |se| {
            let task = self.getTaskBaseByID(ctx.clone(), se.GetSQLExecutor(), taskID)?;
            if task.State != param.PrevState {
                return Err(ErrTaskChanged.into());
            }
            failpoint::InjectCall("beforeMoveToModifying", ());
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, modify_params = %?, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
                vec![proto::TaskStateModifying.into(), bytes.clone().into(), taskID.into(), param.PrevState.into()],
            )?;
            if se.GetSessionVars().StmtCtx().AffectedRows() == 0 {
                // pessimistic txn 下可能被其他事务提前改状态且没有写冲突，Go 映射为 ErrTaskChanged。
                return Err(ErrTaskChanged.into());
            }
            Ok(())
        })
    }

    /// 完成修改：从 modifying 回到 PrevState，并同步活跃 subtask 并发度。
    // ModifiedTask implements the scheduler.TaskManager interface.
    pub fn ModifiedTask(&self, ctx: Context, task: proto::Task) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let prevState = task.ModifyParam.PrevState;
        self.WithNewTxn(ctx.clone(), |se| {
            failpoint::InjectCall("beforeModifiedTask", ());
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, concurrency = %?, max_node_count = %?, meta = %?, modify_params = null, state_update_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
                vec![prevState.into(), task.RequiredSlots.into(), task.MaxNodeCount.into(), task.Meta.clone().into(), task.ID.into(), proto::TaskStateModifying.into()],
            )?;
            if se.GetSessionVars().StmtCtx().AffectedRows() == 0 {
                // 可能已被其他 owner 处理，Go 选择跳过。
                return Ok(());
            }
            // final state 的 subtask 不变；后续若支持不同 subtask concurrency，还要补更多处理。
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_background_subtask set concurrency = %?, state_update_time = unix_timestamp() where task_key = %? and state in (%?, %?, %?)",
                vec![
                    task.RequiredSlots.into(),
                    task.ID.into(),
                    proto::SubtaskStatePending.into(),
                    proto::SubtaskStateRunning.into(),
                    proto::SubtaskStatePaused.into(),
                ],
            )?;
            Ok(())
        })
    }

    /// running → succeed，step 置为 Done，写入 end_time。
    // SucceedTask update task state from running to succeed.
    pub fn SucceedTask(&self, ctx: Context, taskID: i64) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewSession(|se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set state = %?, step = %?, state_update_time = CURRENT_TIMESTAMP(), end_time = CURRENT_TIMESTAMP() where id = %? and state = %?",
                vec![proto::TaskStateSucceed.into(), proto::StepDone.into(), taskID.into(), proto::TaskStateRunning.into()],
            )?;
            Ok(())
        })
    }
}
