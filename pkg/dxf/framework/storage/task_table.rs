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

// DXF 任务/子任务表的持久化管理：创建、查询、切步、槽位与 checkpoint。
//
// 任务元数据落在 `mysql.tidb_global_task`，子任务落在
// `mysql.tidb_background_subtask`（完成后可迁入 history）。
// 通过 Session 池与事务封装保证 SQL 顺序与 Go 版错误语义一致。

// 本文件保持 pkg/dxf/framework/storage/task_table.go 的事务、SQL 顺序和错误语义。

/// 子任务历史默认保留天数。
pub const defaultSubtaskKeepDays: i32 = 14;
/// 任务基行列（不含 meta/error 等大字段），用于列表与调度扫描。
pub const basicTaskColumns: &str = "t.id, t.task_key, t.type, t.state, t.step, t.priority, t.concurrency, t.create_time, t.target_scope, t.max_node_count, t.extra_params, t.keyspace";
/// 完整任务列，含 meta、dispatcher_id、error、modify_params 等。
// TaskColumns is the columns for task.
// TODO: dispatcher_id will update to scheduler_id later
pub const TaskColumns: &str = "t.id, t.task_key, t.type, t.state, t.step, t.priority, t.concurrency, t.create_time, t.target_scope, t.max_node_count, t.extra_params, t.keyspace, t.start_time, t.state_update_time, t.meta, t.dispatcher_id, t.error, t.modify_params";
/// 插入任务时使用的列清单。
// InsertTaskColumns is the columns used in insert task.
pub const InsertTaskColumns: &str = "task_key, type, state, priority, concurrency, step, meta, create_time, target_scope, max_node_count, extra_params, keyspace";
/// 子任务基行列（不含 meta/summary）。
pub const basicSubtaskColumns: &str =
    "id, step, task_key, type, exec_id, state, concurrency, create_time, ordinal, start_time";
/// 完整子任务列，含 state_update_time、meta、summary。
// SubtaskColumns is the columns for subtask.
pub const SubtaskColumns: &str = "id, step, task_key, type, exec_id, state, concurrency, create_time, ordinal, start_time, state_update_time, meta, summary";
/// 插入子任务时使用的列清单。
// InsertSubtaskColumns is the columns used in insert subtask.
pub const InsertSubtaskColumns: &str = "step, task_key, exec_id, meta, state, type, concurrency, ordinal, create_time, checkpoint, summary";

/// 批量插入 subtask 时单批 meta 总大小上限（与事务总大小取 min）。
// maxSubtaskBatchSize 对应 Go 的可变包级变量，用原子值避免并发读写数据竞争。
pub static maxSubtaskBatchSize: AtomicUsize = AtomicUsize::new(16 * units::MiB);

/// 测试用：覆盖 maxSubtaskBatchSize。
pub fn setMaxSubtaskBatchSizeForTest(size: usize) {
    maxSubtaskBatchSize.store(size, Ordering::SeqCst);
}

/// 临时设置系统变量，并按 Go `defer` 语义在动作结束后恢复原值。
fn runWithRestoredSystemVar<T, S, F>(
    mut set: S,
    temporary: String,
    original: String,
    action: F,
) -> Result<T, Error>
where
    S: FnMut(String) -> Result<(), Error>,
    F: FnOnce() -> Result<T, Error>,
{
    set(temporary)?;
    let result = action();
    let _ = set(original);
    result
}

/// 子任务数量不稳定（批量切步时已有行数超过预期）。
// 以下错误值保留 Go 的包级哨兵错误语义。
pub static ErrUnstableSubtasks: GoError = GoError::new("unstable subtasks");
/// 任务不存在。
pub static ErrTaskNotFound: GoError = GoError::new("task not found");
/// 任务已存在（唯一键冲突等）。
pub static ErrTaskAlreadyExists: GoError = GoError::new("task already exists");
/// 当前任务状态不允许该操作。
pub static ErrTaskStateNotAllow: GoError = GoError::new("task state not allow to do the operation");
/// 任务已被其他操作并发修改。
pub static ErrTaskChanged: GoError = GoError::new("task changed by other operation");
/// 子任务不存在或不属于当前执行节点。
pub static ErrSubtaskNotFound: GoError = GoError::new("subtask not found");

/// 应用侧可见的任务管理接口（便于 mock）。
// Manager is the interface for task manager.
// those methods are used by application side, we expose them through interface to make tests easier.
pub trait Manager {
    fn GetCPUCountOfNode(&self, ctx: Context) -> Result<i32, Error>;
    fn GetTaskByID(&self, ctx: Context, taskID: i64) -> Result<proto::Task, Error>;
    fn ModifyTaskByID(
        &self,
        ctx: Context,
        taskID: i64,
        param: proto::ModifyParam,
    ) -> Result<(), Error>;
}

/// 某执行节点上某任务的调度信息（含当前步 subtask 并发度）。
// TaskExecInfo is the execution information of a task, on some exec node.
pub struct TaskExecInfo {
    pub TaskBase: proto::TaskBase,
    // SubtaskConcurrency is the concurrency of subtask in current task step.
    // TODO: will be used when support subtask have smaller concurrency than task.
    pub SubtaskConcurrency: i32,
}

/// 在新 Session / 新事务中执行闭包的能力抽象。
// SessionExecutor defines the interface for executing SQLs in a session.
pub trait SessionExecutor {
    fn WithNewSession<F>(&self, fn_: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>;
    fn WithNewTxn<F>(&self, ctx: Context, fn_: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>;
}

/// Scheduler 所需的任务句柄：读取前序 step 的 meta/summary。
// TaskHandle provides the interface for operations needed by Scheduler.
pub trait TaskHandle: SessionExecutor {
    fn GetPreviousSubtaskMetas(
        &self,
        taskID: i64,
        step: proto::Step,
    ) -> Result<Vec<Vec<u8>>, Error>;
    fn GetPreviousSubtaskSummary(
        &self,
        taskID: i64,
        step: proto::Step,
    ) -> Result<Vec<execute::SubtaskSummary>, Error>;
}

/// 任务与子任务的表管理器，持有 Session 池。
// TaskManager is the manager of task and subtask.
#[derive(Clone)]
pub struct TaskManager {
    pub sePool: util::SessionPool,
}

// 进程内默认 TaskManager 单例（指向本地 TiDB 存储）。
static taskManagerInstance: AtomicGoPointer<TaskManager> = AtomicGoPointer::new();
// nextgen：用户 keyspace 内初始化，实际指向 SYSTEM keyspace 的 DXF 服务 TaskManager。
// this one is only used on nextgen, and it's only initialized in user ks and point to SYSTEM KS
static dxfSvcTaskMgr: AtomicGoPointer<TaskManager> = AtomicGoPointer::new();

/// 测试用：记录最近一次插入的 task ID。
// TestLastTaskID is used for test to set the last task ID.
pub static TestLastTaskID: AtomicI64 = AtomicI64::new(0);

/// 用给定 Session 池构造 TaskManager。
// NewTaskManager creates a new task manager.
pub fn NewTaskManager(sePool: util::SessionPool) -> TaskManager {
    TaskManager { sePool }
}

/// 获取本地存储 TaskManager；未初始化则报错。
// GetTaskManager gets the task manager.
// This task manager always points to local TiDB storage; nextgen DXF service should use GetDXFSvcTaskMgr.
pub fn GetTaskManager() -> Result<TaskManager, Error> {
    taskManagerInstance
        .Load()
        .ok_or_else(|| Error::new("task manager is not initialized"))
}

/// 设置本地 TaskManager 单例。
// SetTaskManager sets the task manager.
pub fn SetTaskManager(is: TaskManager) {
    taskManagerInstance.Store(is);
}

/// nextgen 用户 KS 返回 DXF 服务侧 TaskManager，否则回退本地。
// GetDXFSvcTaskMgr returns the task manager to access DXF service.
pub fn GetDXFSvcTaskMgr() -> Result<TaskManager, Error> {
    if kerneltype::IsNextGen() && config::GetGlobalKeyspaceName() != keyspace::System {
        return dxfSvcTaskMgr
            .Load()
            .ok_or_else(|| Error::new("DXF service task manager is not initialized"));
    }
    GetTaskManager()
}

/// 设置 DXF 服务 TaskManager。
// SetDXFSvcTaskMgr sets the task manager for DXF service.
pub fn SetDXFSvcTaskMgr(mgr: TaskManager) {
    dxfSvcTaskMgr.Store(mgr);
}

impl TaskManager {
    /// 从池取 Session，临时抬高 TxnEntrySizeLimit 后执行闭包并归还。
    // WithNewSession executes the function with a new session.
    pub fn WithNewSession<F>(&self, fn_: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>,
    {
        injectfailpoint::DXFRandomErrorWithOnePerThousand()?;
        let v = self.sePool.Get()?;
        let se = v.downcast::<sessionctx::Context>();
        let limitBak = se.TxnEntrySizeLimit();
        se.SetTxnEntrySizeLimit(vardef::TxnEntrySizeLimit::Load());
        let ret = fn_(se.clone());
        // Go defer 会恢复 TxnEntrySizeLimit 并把 session 放回 pool；显式保留资源收尾顺序。
        se.SetTxnEntrySizeLimit(limitBak);
        self.sePool.Put(v);
        ret
    }

    /// 在新事务中执行：BEGIN → 闭包 → Commit/Rollback。
    // WithNewTxn executes the fn in a new transaction.
    pub fn WithNewTxn<F>(&self, ctx: Context, fn_: F) -> Result<(), Error>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), Error>,
    {
        let ctx = clitutil::WithInternalSourceType(ctx, kv::InternalDistTask);
        self.WithNewSession(|se| {
            // BEGIN 仍走 SQL path；commit/rollback 使用 session 方法，避免取消上下文时清理失败。
            sqlexec::ExecSQL(ctx.clone(), se.GetSQLExecutor(), "begin", vec![])?;
            let result = fn_(se.clone());
            if result.is_ok() {
                se.CommitTxn(ctx.clone())?;
            } else {
                se.RollbackTxn(clitutil::WithInternalSourceType(
                    Context::background(),
                    kv::InternalDistTask,
                ));
            }
            result
        })
    }

    /// 开新 Session 执行单条 SQL 并返回结果行。
    // ExecuteSQLWithNewSession executes one SQL with new session.
    pub fn ExecuteSQLWithNewSession<S: Into<String>>(
        &self,
        ctx: Context,
        sql: S,
        args: Vec<Value>,
    ) -> Result<Vec<chunk::Row>, Error> {
        let mut rs = Vec::new();
        self.WithNewSession(|se| {
            rs = sqlexec::ExecSQL(ctx.clone(), se.GetSQLExecutor(), sql.into(), args.clone())?;
            Ok(())
        })?;
        Ok(rs)
    }

    /// 创建全局任务（内部开 Session）。
    // CreateTask adds a new task to task table.
    pub fn CreateTask(
        &self,
        ctx: Context,
        key: String,
        tp: proto::TaskType,
        keyspace: String,
        requiredSlots: i32,
        targetScope: String,
        maxNodeCnt: i32,
        extraParams: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64, Error> {
        let mut taskID = 0;
        self.WithNewSession(|se| {
            taskID = self.CreateTaskWithSession(
                ctx.clone(),
                se,
                key.clone(),
                tp,
                keyspace.clone(),
                requiredSlots,
                targetScope.clone(),
                maxNodeCnt,
                extraParams.clone(),
                meta.clone(),
            )?;
            Ok(())
        })?;
        Ok(taskID)
    }

    /// 在给定 Session 上插入 pending 任务；校验 requiredSlots 不超过节点 CPU。
    // CreateTaskWithSession adds a new task to task table with session.
    pub fn CreateTaskWithSession(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        key: String,
        tp: proto::TaskType,
        keyspace: String,
        requiredSlots: i32,
        targetScope: String,
        maxNodeCount: i32,
        extraParams: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64, Error> {
        let cpuCount =
            self.getCPUCountOfNodeByRole(ctx.clone(), se.clone(), String::new(), true)?;
        if requiredSlots > cpuCount {
            return Err(Error::new(format!(
                "task required slots({}) larger than cpu count({}) of managed node",
                requiredSlots, cpuCount
            )));
        }
        failpoint::InjectCall("beforeSubmitTask", (&requiredSlots, &extraParams));
        let extraParamBytes = json::Marshal(&extraParams).map_err(errors::Trace)?;
        sqlexec::ExecSQL(
            ctx.clone(),
            se.GetSQLExecutor(),
            format!(
                "insert into mysql.tidb_global_task({}) values (%?, %?, %?, %?, %?, %?, %?, CURRENT_TIMESTAMP(), %?, %?, %?, %?)",
                InsertTaskColumns
            ),
            vec![
                key.into(),
                tp.into(),
                proto::TaskStatePending.into(),
                proto::NormalPriority.into(),
                requiredSlots.into(),
                proto::StepInit.into(),
                meta.into(),
                targetScope.into(),
                maxNodeCount.into(),
                json::RawMessage(extraParamBytes).into(),
                keyspace.into(),
            ],
        )?;
        let rs = sqlexec::ExecSQL(ctx, se.GetSQLExecutor(), "select @@last_insert_id", vec![])?;
        let taskID = rs[0].GetUint64(0) as i64;
        failpoint::Inject("testSetLastTaskID", || {
            TestLastTaskID.store(taskID, Ordering::SeqCst)
        });
        Ok(taskID)
    }
}

impl TaskManager {
    /// 将子任务运行摘要（行数/进度等）序列化为 JSON 写回。
    // UpdateSubtaskSummary updates the subtask summary.
    pub fn UpdateSubtaskSummary(
        &self,
        ctx: Context,
        subtaskID: i64,
        summary: execute::SubtaskSummary,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let summaryBytes = json::Marshal(&summary).map_err(errors::Trace)?;
        self.ExecuteSQLWithNewSession(
            ctx,
            "update mysql.tidb_background_subtask set summary = %? where id = %?",
            vec![hack::String(summaryBytes).into(), subtaskID.into()],
        )?;
        Ok(())
    }

    /// 按 state 聚合当前 step 的子任务数量。
    // GetSubtaskCntGroupByStates gets the subtask count by states.
    pub fn GetSubtaskCntGroupByStates(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
    ) -> Result<HashMap<proto::SubtaskState, i64>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select state, count(*) from mysql.tidb_background_subtask where task_key = %? and step = %? group by state",
            vec![TaskIDToKey(taskID).into(), step.into()],
        )?;
        let mut res = HashMap::with_capacity(rs.len());
        for row in rs {
            res.insert(intern(row.GetString(0)), row.GetInt64(1));
        }
        Ok(res)
    }

    /// 一次读出各 state 计数，并收集 failed/canceled 的错误。
    // GetSubtaskStateCntAndErrorsByStep gets the subtask count by state and failed/canceled errors in one step-scoped read.
    pub fn GetSubtaskStateCntAndErrorsByStep(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
    ) -> Result<(HashMap<proto::SubtaskState, i64>, Vec<Error>), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select state, error from mysql.tidb_background_subtask where task_key = %? and step = %?",
            vec![TaskIDToKey(taskID).into(), step.into()],
        )?;
        let mut cntByStates = HashMap::with_capacity(rs.len());
        let mut subTaskErrors = Vec::new();
        for row in rs {
            let state = intern(row.GetString(0));
            *cntByStates.entry(state).or_insert(0) += 1;
            if state != proto::SubtaskStateFailed && state != proto::SubtaskStateCanceled {
                continue;
            }
            let subTaskErr = unmarshalSubtaskError(row.GetBytes(1), row.IsNull(1))?;
            if let Some(err) = subTaskErr {
                subTaskErrors.push(err);
            }
        }
        Ok((cntByStates, subTaskErrors))
    }

    /// 读取任务下 failed/canceled 子任务的错误列表。
    // GetSubtaskErrors gets subtasks' errors.
    pub fn GetSubtaskErrors(&self, ctx: Context, taskID: i64) -> Result<Vec<Option<Error>>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select error from mysql.tidb_background_subtask where task_key = %? AND state in (%?, %?)",
            vec![TaskIDToKey(taskID).into(), proto::SubtaskStateFailed.into(), proto::SubtaskStateCanceled.into()],
        )?;
        let mut subTaskErrors = Vec::with_capacity(rs.len());
        for row in rs {
            subTaskErrors.push(unmarshalSubtaskError(row.GetBytes(0), row.IsNull(0))?);
        }
        Ok(subTaskErrors)
    }

    /// 批量更新子任务执行节点 ID（调度重分配）。
    // UpdateSubtasksExecIDs update subtasks' execID.
    pub fn UpdateSubtasksExecIDs(
        &self,
        ctx: Context,
        subtasks: Vec<proto::SubtaskBase>,
    ) -> Result<(), Error> {
        // 空列表跳过更新，避免无意义事务。
        if subtasks.is_empty() {
            return Ok(());
        }
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            for subtask in &subtasks {
                sqlexec::ExecSQL(
                    ctx.clone(),
                    se.GetSQLExecutor(),
                    "update mysql.tidb_background_subtask set exec_id = %? where id = %? and state = %?",
                    vec![subtask.ExecID.clone().into(), subtask.ID.into(), subtask.State.into()],
                )?;
            }
            Ok(())
        })
    }

    /// 事务内切换任务 state/step，并插入本步新 subtask。
    // SwitchTaskStep implements the scheduler.TaskManager interface.
    pub fn SwitchTaskStep(
        &self,
        ctx: Context,
        task: proto::Task,
        nextState: proto::TaskState,
        nextStep: proto::Step,
        subtasks: Vec<proto::Subtask>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            let vars = se.GetSessionVars();
            let switch = || {
                self.updateTaskStateStep(
                    ctx.clone(),
                    se.clone(),
                    task.clone(),
                    nextState,
                    nextStep.clone(),
                )?;
                if vars.StmtCtx.AffectedRows() == 0 {
                    // 网络分区或 owner 切换可能导致其他 scheduler 已经切步；Go 选择跳过后续插入。
                    return Ok(());
                }
                self.insertSubtasks(ctx.clone(), se, subtasks)
            };

            // Go defer 覆盖完整事务动作：CAS 更新和后续 subtask 插入都使用抬高后的配额。
            if vars.MemQuotaQuery < vardef::DefTiDBMemQuotaQuery {
                runWithRestoredSystemVar(
                    |value| vars.SetSystemVar(vardef::TiDBMemQuotaQuery, value),
                    vardef::DefTiDBMemQuotaQuery.to_string(),
                    vars.MemQuotaQuery.to_string(),
                    switch,
                )
            } else {
                switch()
            }
        })
    }

    /// prepare 完成：pending+init → pending+prepared，返回是否真正切换。
    // SwitchTaskStepAfterPrepare atomically persists prepare completion from pending+init to pending+prepared.
    pub fn SwitchTaskStepAfterPrepare(
        &self,
        ctx: Context,
        task: proto::Task,
    ) -> Result<bool, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let mut switched = false;
        self.WithNewTxn(ctx.clone(), |se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set step = %?, state_update_time = CURRENT_TIMESTAMP(), meta = %?, concurrency = %?, max_node_count = %? where id = %? and state = %? and step = %?",
                vec![
                    proto::StepPrepared.into(),
                    task.Meta.clone().into(),
                    task.RequiredSlots.into(),
                    task.MaxNodeCount.into(),
                    task.ID.into(),
                    proto::TaskStatePending.into(),
                    proto::StepInit.into(),
                ],
            )?;
            switched = se.GetSessionVars().StmtCtx.AffectedRows() > 0;
            Ok(())
        })?;
        Ok(switched)
    }

    /// 条件更新任务 state/step/meta；pending 时额外写 start_time。
    fn updateTaskStateStep(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        task: proto::Task,
        nextState: proto::TaskState,
        nextStep: proto::Step,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let extraUpdateStr = if task.State == proto::TaskStatePending {
            "start_time = CURRENT_TIMESTAMP(),"
        } else {
            ""
        };
        // TODO: during generating subtask, task meta might change, maybe move meta update to another place.
        sqlexec::ExecSQL(
            ctx,
            se.GetSQLExecutor(),
            format!(
                "update mysql.tidb_global_task set state = %?, step = %?, {} state_update_time = CURRENT_TIMESTAMP(), meta = %? where id = %? and state = %? and step = %?",
                extraUpdateStr
            ),
            vec![
                nextState.into(),
                nextStep.into(),
                task.Meta.clone().into(),
                task.ID.into(),
                task.State.into(),
                task.Step.into(),
            ],
        )?;
        Ok(())
    }

    /// 批量 INSERT 子任务；占位符与参数分离以匹配 ExecSQL 形状。
    // insertSubtasks 对应 Go 的批量 insert；marker/args 分离保留 ExecSQL 参数化形状。
    fn insertSubtasks(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        subtasks: Vec<proto::Subtask>,
    ) -> Result<(), Error> {
        if subtasks.is_empty() {
            return Ok(());
        }
        failpoint::Inject("waitBeforeInsertSubtasks", || {
            TestChannel.recv();
            TestChannel.recv();
        });
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let mut markerList = Vec::with_capacity(subtasks.len());
        let mut args = Vec::with_capacity(subtasks.len() * 7);
        for subtask in subtasks {
            markerList.push("(%?, %?, %?, %?, %?, %?, %?, %?, CURRENT_TIMESTAMP(), '{}', '{}')");
            args.extend(vec![
                subtask.Step.into(),
                subtask.TaskID.into(),
                subtask.ExecID.clone().into(),
                subtask.Meta.clone().into(),
                proto::SubtaskStatePending.into(),
                proto::Type2Int(subtask.Type).into(),
                subtask.Concurrency.into(),
                subtask.Ordinal.into(),
            ]);
        }
        let sql = format!(
            "insert into mysql.tidb_background_subtask({}) values {}",
            InsertSubtaskColumns,
            markerList.join(",")
        );
        sqlexec::ExecSQL(ctx, se.GetSQLExecutor(), sql, args)?;
        Ok(())
    }

    /// 分批插入 subtask 后再切步；已有行数超过预期则报不稳定。
    // SwitchTaskStepInBatch implements the scheduler.TaskManager interface.
    pub fn SwitchTaskStepInBatch(
        &self,
        ctx: Context,
        task: proto::Task,
        nextState: proto::TaskState,
        nextStep: proto::Step,
        subtasks: Vec<proto::Subtask>,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewSession(|se| {
            // 其他 scheduler 可能已经插入一部分 subtask；Go 通过已有数量跳过已插入前缀。
            let rs = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "select count(1) from mysql.tidb_background_subtask where task_key = %? and step = %?",
                vec![TaskIDToKey(task.ID).into(), nextStep.clone().into()],
            )?;
            let existingTaskCnt = rs[0].GetInt64(0) as usize;
            if existingTaskCnt > subtasks.len() {
                return Err(errors::Annotatef(
                    ErrUnstableSubtasks,
                    format!("expected {}, got {}", subtasks.len(), existingTaskCnt),
                ));
            }
            for batch in self.splitSubtasks(subtasks[existingTaskCnt..].to_vec()) {
                self.insertSubtasks(ctx.clone(), se.clone(), batch)?;
            }
            self.updateTaskStateStep(ctx.clone(), se, task.clone(), nextState, nextStep.clone())
        })
    }

    /// 按 meta 大小将 subtask 切成多批，避免单事务过大。
    pub fn splitSubtasks(&self, subtasks: Vec<proto::Subtask>) -> Vec<Vec<proto::Subtask>> {
        let mut res = Vec::with_capacity(10);
        let mut currBatch = Vec::with_capacity(10);
        let mut size = 0usize;
        let maxSize = min(
            kv::TxnTotalSizeLimit::Load() as usize,
            maxSubtaskBatchSize.load(Ordering::SeqCst),
        );
        for s in subtasks {
            // 加上本条会超限则先封批；单条超限时仍单独成批，避免空批。
            if !currBatch.is_empty() && size + s.Meta.len() > maxSize {
                res.push(currBatch);
                currBatch = Vec::new();
                size = 0;
            }
            size += s.Meta.len();
            currBatch.push(s);
        }
        if !currBatch.is_empty() {
            res.push(currBatch);
        }
        res
    }

    /// 合并查询活跃表与 history 表中的子任务（避免刚归档时漏读）。
    // GetSubtasksWithHistory gets the subtasks from tidb_global_task and tidb_global_task_history.
    pub fn GetSubtasksWithHistory(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
    ) -> Result<Option<Vec<proto::Subtask>>, Error> {
        let mut rs: Vec<chunk::Row> = Vec::new();
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            rs = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                format!("select {} from mysql.tidb_background_subtask where task_key = %? and step = %?", SubtaskColumns),
                vec![TaskIDToKey(taskID).into(), step.clone().into()],
            )?;
            // 避免 show import jobs 时任务刚被 TransferTasks2History 搬走，Go 会再读 history 表并合并。
            let mut rsFromHistory = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                format!("select {} from mysql.tidb_background_subtask_history where task_key = %? and step = %?", SubtaskColumns),
                vec![TaskIDToKey(taskID).into(), step.clone().into()],
            )?;
            rs.append(&mut rsFromHistory);
            Ok(())
        })?;
        if rs.is_empty() {
            return Ok(None);
        }
        Ok(Some(rs.into_iter().map(Row2SubTask).collect()))
    }

    /// 列出全部任务基行。
    // GetAllTasks gets all tasks with basic columns.
    pub fn GetAllTasks(&self, ctx: Context) -> Result<Option<Vec<proto::TaskBase>>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!("select {} from mysql.tidb_global_task t", basicTaskColumns),
            vec![],
        )?;
        if rs.is_empty() {
            return Ok(None);
        }
        Ok(Some(rs.into_iter().map(row2TaskBasic).collect()))
    }

    /// 按 keyspace 汇总任务数量。
    // GetActiveTaskCountsByKeyspace gets active task summary grouped by keyspace.
    pub fn GetActiveTaskCountsByKeyspace(&self, ctx: Context) -> Result<ActiveTaskSummary, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select keyspace, count(1) from mysql.tidb_global_task group by keyspace",
            vec![],
        )?;
        let mut summary = ActiveTaskSummary {
            Total: 0,
            PerKeyspace: HashMap::with_capacity(rs.len()),
        };
        for row in rs {
            let keyspace = row.GetString(0);
            let cnt = row.GetInt64(1);
            summary.Total += cnt;
            summary.PerKeyspace.insert(keyspace, cnt);
        }
        Ok(summary)
    }

    /// 列出全部子任务基行。
    // GetAllSubtasks gets all subtasks with basic columns.
    pub fn GetAllSubtasks(&self, ctx: Context) -> Result<Option<Vec<proto::SubtaskBase>>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_background_subtask",
                basicSubtaskColumns
            ),
            vec![],
        )?;
        if rs.is_empty() {
            return Ok(None);
        }
        Ok(Some(rs.into_iter().map(row2BasicSubTask).collect()))
    }

    /// 纠正升级遗留的过大 concurrency（如 v7.5 硬编码 16）为当前节点 CPU。
    // AdjustTaskOverflowConcurrency change the task concurrency to a max value supported by current cluster.
    // This is a workaround for an upgrade bug in v7.5.x where task concurrency was hard-coded to 16.
    pub fn AdjustTaskOverflowConcurrency(
        &self,
        ctx: Context,
        se: sessionctx::Context,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let cpuCount =
            self.getCPUCountOfNodeByRole(ctx.clone(), se.clone(), String::new(), true)?;
        let sql = "update mysql.tidb_global_task set concurrency = %? where concurrency > %?;";
        sqlexec::ExecSQL(
            ctx,
            se.GetSQLExecutor(),
            sql,
            vec![cpuCount.into(), cpuCount.into()],
        )?;
        Ok(())
    }

    /// 写入子任务 checkpoint（断点，用于失败后续跑）。
    // UpdateSubtaskCheckpoint updates the checkpoint of a subtask.
    pub fn UpdateSubtaskCheckpoint(
        &self,
        ctx: Context,
        subtaskID: i64,
        checkpoint: Value,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let data = json::Marshal(&checkpoint)?;
        let checkpointJSON = String::from_utf8(data).unwrap_or_default();
        self.ExecuteSQLWithNewSession(
            ctx,
            "UPDATE mysql.tidb_background_subtask SET checkpoint = %? WHERE id = %?",
            vec![checkpointJSON.into(), subtaskID.into()],
        )?;
        Ok(())
    }

    /// 读取子任务 checkpoint JSON 字符串。
    // GetSubtaskCheckpoint gets the checkpoint of a subtask.
    pub fn GetSubtaskCheckpoint(&self, ctx: Context, subtaskID: i64) -> Result<String, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "SELECT checkpoint FROM mysql.tidb_background_subtask WHERE id = %?",
            vec![subtaskID.into()],
        )?;
        if rs.is_empty() || rs[0].IsNull(0) {
            return Ok(String::new());
        }
        Ok(rs[0].GetString(0))
    }

    /// 更新任务 ExtraParams。
    // UpdateTaskExtraParams updates the extra params of a task.
    pub fn UpdateTaskExtraParams(
        &self,
        ctx: Context,
        taskID: i64,
        extraParams: proto::ExtraParams,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let extraParamBytes = json::Marshal(&extraParams).map_err(errors::Trace)?;
        self.WithNewTxn(ctx.clone(), |se| {
            sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "update mysql.tidb_global_task set extra_params = %? where id = %?",
                vec![
                    json::RawMessage(extraParamBytes.clone()).into(),
                    taskID.into(),
                ],
            )?;
            Ok(())
        })
    }
}

/// 测试用 failpoint 同步通道。
// TestChannel is used for test.
// Go 使用 chan struct{} 作为 failpoint 同步点；保留为测试通道占位。
pub static TestChannel: GoChannel<()> = GoChannel::new();

/// 将 Error 序列化为 JSON；PingCAP 错误保留 RFC/MySQL code。
// serializeErr 对应 Go 的错误 JSON 序列化。PingCAP errors.Error 会保留 RFC code/mysql code。
pub fn serializeErr(inErr: Error) -> Vec<u8> {
    if inErr.is_nil() {
        return Vec::new();
    }
    let tErr = if let Some(e) = inErr.as_pingcap_error() {
        Error::pingcap(
            errors::GetErrStackMsg(&inErr),
            e.RFCCode().to_string(),
            e.Code() as i32,
        )
    } else {
        Error::new(inErr.to_string())
    };
    tErr.MarshalJSON().unwrap_or_default()
}

/// Option<Error> 序列化：None 对应 Go 的 nil，返回空字节。
// serializeErrOption 是本为 Rust Option<Error> 形状补的轻量辅助，语义仍对应 Go 的 nil error。
pub fn serializeErrOption(inErr: Option<Error>) -> Vec<u8> {
    inErr.map(serializeErr).unwrap_or_default()
}

/// 反序列化子任务 error 列；NULL/空视为无错误。
fn unmarshalSubtaskError(errBytes: Vec<u8>, isNull: bool) -> Result<Option<Error>, Error> {
    if isNull || errBytes.is_empty() {
        return Ok(None);
    }
    let mut stdErr = Error::new("");
    stdErr.UnmarshalJSON(errBytes)?;
    Ok(Some(stdErr.into()))
}

/// 活跃任务按 keyspace 汇总。
// ActiveTaskSummary is the summary of active tasks in `mysql.tidb_global_task`.
pub struct ActiveTaskSummary {
    pub Total: i64,
    pub PerKeyspace: HashMap<String, i64>,
}

impl TaskManager {
    /// 取出未完成态任务（按优先级/创建时间排序，带 limit）。
    // GetTopUnfinishedTasks implements the scheduler.TaskManager interface.
    pub fn GetTopUnfinishedTasks(&self, ctx: Context) -> Result<Vec<proto::TaskBase>, Error> {
        self.getTopTasks(
            ctx,
            vec![
                proto::TaskStatePending,
                proto::TaskStateRunning,
                proto::TaskStateReverting,
                proto::TaskStateCancelling,
                proto::TaskStatePausing,
                proto::TaskStateResuming,
                proto::TaskStateModifying,
            ],
        )
    }

    /// 取出不需再占资源即可推进的任务（回滚/取消/暂停/修改中）。
    // GetTopNoNeedResourceTasks implements the scheduler.TaskManager interface.
    pub fn GetTopNoNeedResourceTasks(&self, ctx: Context) -> Result<Vec<proto::TaskBase>, Error> {
        self.getTopTasks(
            ctx,
            vec![
                proto::TaskStateReverting,
                proto::TaskStateCancelling,
                proto::TaskStatePausing,
                proto::TaskStateModifying,
            ],
        )
    }

    /// 按给定 state 集合取 Top 任务基行。
    fn getTopTasks(
        &self,
        ctx: Context,
        states: Vec<proto::TaskState>,
    ) -> Result<Vec<proto::TaskBase>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let holders = vec!["%?"; states.len()].join(",");
        let sql = format!(
            "select {} from mysql.tidb_global_task t where state in ({}) order by priority asc, create_time asc, id asc limit %?",
            basicTaskColumns, holders
        );
        let mut args: Vec<Value> = states.into_iter().map(Value::from).collect();
        args.push((proto::GetMaxConcurrentTask() * 2).into());
        let rs = self.ExecuteSQLWithNewSession(ctx, sql, args)?;
        Ok(rs.into_iter().map(row2TaskBasic).collect())
    }

    /// 按执行节点汇总其 pending/running 子任务所属任务及 max 并发。
    // GetTaskExecInfoByExecID implements the scheduler.TaskManager interface.
    pub fn GetTaskExecInfoByExecID(
        &self,
        ctx: Context,
        execID: String,
    ) -> Result<Vec<TaskExecInfo>, Error> {
        let r = tracing::StartRegion(ctx.clone(), "TaskManager.GetTaskExecInfoByExecID");
        let mut res = Vec::new();
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewSession(|se| {
            // pending/running subtask 的 step 等于当前 task step，因此 Go 这里不按 step 过滤。
            let rs = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "select st.task_key, max(st.concurrency) from mysql.tidb_background_subtask st where st.exec_id = %? and st.state in (%?, %?) group by st.task_key",
                vec![execID.clone().into(), proto::SubtaskStatePending.into(), proto::SubtaskStateRunning.into()],
            )?;
            if rs.is_empty() {
                return Ok(());
            }
            let mut maxSubtaskCon = HashMap::with_capacity(rs.len());
            let mut taskIDs = Vec::with_capacity(rs.len());
            for row in rs {
                let taskIDStr = row.GetString(0);
                let taskID = taskIDStr.parse::<i64>().map_err(errors::Trace)?;
                maxSubtaskCon.insert(taskID, row.GetInt64(1) as i32);
                taskIDs.push(taskIDStr);
            }
            let rs = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                format!(
                    "select {} from mysql.tidb_global_task t where t.id in ({}) and t.state in (%?, %?, %?) order by priority asc, create_time asc, id asc",
                    basicTaskColumns,
                    taskIDs.join(",")
                ),
                vec![
                    proto::TaskStateRunning.into(),
                    proto::TaskStateReverting.into(),
                    proto::TaskStatePausing.into(),
                ],
            )?;
            for row in rs {
                let taskBase = row2TaskBasic(row);
                res.push(TaskExecInfo {
                    SubtaskConcurrency: *maxSubtaskCon.get(&taskBase.ID).unwrap_or(&0),
                    TaskBase: taskBase,
                });
            }
            Ok(())
        })?;
        r.End();
        Ok(res)
    }

    /// Fetch finished tasks with the owner-local cleanup bound, without sorting.
    pub fn GetCleanupTasks(&self, ctx: Context) -> Result<Vec<proto::Task>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rows = self.ExecuteSQLWithNewSession(ctx,
            format!("select {} from mysql.tidb_global_task t where state in (%?, %?, %?) limit %?", TaskColumns),
            vec![proto::TaskStateFailed.into(), proto::TaskStateReverted.into(), proto::TaskStateSucceed.into(), proto_crate::GetTaskCleanupBatchSize().into()])?;
        Ok(rows.into_iter().map(Row2Task).collect())
    }

    /// 按状态集合查询完整任务行。
    // GetTasksInStates gets the tasks in the states(order by priority asc, create_time acs, id asc).
    pub fn GetTasksInStates(
        &self,
        ctx: Context,
        states: Vec<Value>,
    ) -> Result<Vec<proto::Task>, Error> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let placeholders = format!("{}%?", "%?,".repeat(states.len() - 1));
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where state in ({}) order by priority asc, create_time asc, id asc",
                TaskColumns, placeholders
            ),
            states,
        )?;
        Ok(rs.into_iter().map(Row2Task).collect())
    }

    /// 按状态集合查询任务基行。
    // GetTaskBasesInStates gets the task bases in the states(order by priority asc, create_time acs, id asc).
    pub fn GetTaskBasesInStates(
        &self,
        ctx: Context,
        states: Vec<Value>,
    ) -> Result<Vec<proto::TaskBase>, Error> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let placeholders = format!("{}%?", "%?,".repeat(states.len() - 1));
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where state in ({}) order by priority asc, create_time asc, id asc",
                basicTaskColumns, placeholders
            ),
            states,
        )?;
        Ok(rs.into_iter().map(row2TaskBasic).collect())
    }

    /// 按 ID 取完整任务；缺失返回 ErrTaskNotFound。
    // GetTaskByID gets the task by the task ID.
    pub fn GetTaskByID(&self, ctx: Context, taskID: i64) -> Result<proto::Task, Error> {
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where id = %?",
                TaskColumns
            ),
            vec![taskID.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(Row2Task(rs[0].clone()))
    }

    /// 按 ID 取任务基行（开新 Session）。
    // GetTaskBaseByID implements the TaskManager.GetTaskBaseByID interface.
    pub fn GetTaskBaseByID(&self, ctx: Context, taskID: i64) -> Result<proto::TaskBase, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let mut task = None;
        self.WithNewSession(|se| {
            task = Some(self.getTaskBaseByID(ctx.clone(), se.GetSQLExecutor(), taskID)?);
            Ok(())
        })?;
        Ok(task.unwrap())
    }

    /// 在给定 SQLExecutor 上按 ID 读任务基行。
    fn getTaskBaseByID(
        &self,
        ctx: Context,
        exec: sqlexec::SQLExecutor,
        taskID: i64,
    ) -> Result<proto::TaskBase, Error> {
        let rs = sqlexec::ExecSQL(
            ctx,
            exec,
            format!(
                "select {} from mysql.tidb_global_task t where id = %?",
                basicTaskColumns
            ),
            vec![taskID.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(row2TaskBasic(rs[0].clone()))
    }

    /// 活跃表 UNION history 表按 ID 取任务。
    // GetTaskByIDWithHistory gets the task by the task ID from both tidb_global_task and tidb_global_task_history.
    pub fn GetTaskByIDWithHistory(&self, ctx: Context, taskID: i64) -> Result<proto::Task, Error> {
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where id = %? union select {} from mysql.tidb_global_task_history t where id = %?",
                TaskColumns, TaskColumns
            ),
            vec![taskID.into(), taskID.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(Row2Task(rs[0].clone()))
    }

    /// 活跃+history 按 ID 取任务基行。
    // GetTaskBaseByIDWithHistory gets the task by the task ID from both tidb_global_task and tidb_global_task_history.
    pub fn GetTaskBaseByIDWithHistory(
        &self,
        ctx: Context,
        taskID: i64,
    ) -> Result<proto::TaskBase, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where id = %? union select {} from mysql.tidb_global_task_history t where id = %?",
                basicTaskColumns, basicTaskColumns
            ),
            vec![taskID.into(), taskID.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(row2TaskBasic(rs[0].clone()))
    }

    /// 按 task_key 取完整任务。
    // GetTaskByKey gets the task by the task key.
    pub fn GetTaskByKey(&self, ctx: Context, key: String) -> Result<proto::Task, Error> {
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where task_key = %?",
                TaskColumns
            ),
            vec![key.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(Row2Task(rs[0].clone()))
    }

    /// 活跃+history 按 task_key 取任务。
    // GetTaskByKeyWithHistory gets the task from history table by the task key.
    pub fn GetTaskByKeyWithHistory(&self, ctx: Context, key: String) -> Result<proto::Task, Error> {
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where task_key = %?union select {} from mysql.tidb_global_task_history t where task_key = %?",
                TaskColumns, TaskColumns
            ),
            vec![key.clone().into(), key.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(Row2Task(rs[0].clone()))
    }

    /// 活跃+history 按 task_key 取任务基行。
    // GetTaskBaseByKeyWithHistory gets the task base from history table by the task key.
    pub fn GetTaskBaseByKeyWithHistory(
        &self,
        ctx: Context,
        key: String,
    ) -> Result<proto::TaskBase, Error> {
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_global_task t where task_key = %?union select {} from mysql.tidb_global_task_history t where task_key = %?",
                basicTaskColumns, basicTaskColumns
            ),
            vec![key.clone().into(), key.into()],
        )?;
        if rs.is_empty() {
            return Err(ErrTaskNotFound.into());
        }
        Ok(row2TaskBasic(rs[0].clone()))
    }

    /// 按执行节点、任务、step、状态集合查询子任务。
    // GetSubtasksByExecIDAndStepAndStates gets all subtasks by given states on one node.
    pub fn GetSubtasksByExecIDAndStepAndStates(
        &self,
        ctx: Context,
        execID: String,
        taskID: i64,
        step: proto::Step,
        states: Vec<proto::SubtaskState>,
    ) -> Result<Vec<proto::Subtask>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let mut args = vec![execID.into(), TaskIDToKey(taskID).into(), step.into()];
        args.extend(states.iter().cloned().map(Value::from));
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_background_subtask where exec_id = %? and task_key = %? and step = %? and state in ({}%?)",
                SubtaskColumns,
                "%?,".repeat(states.len().saturating_sub(1))
            ),
            args,
        )?;
        Ok(rs.into_iter().map(Row2SubTask).collect())
    }

    /// 取满足状态集合的第一条子任务（limit 1）。
    // GetFirstSubtaskInStates gets the first subtask by given states.
    pub fn GetFirstSubtaskInStates(
        &self,
        ctx: Context,
        tidbID: String,
        taskID: i64,
        step: proto::Step,
        states: Vec<proto::SubtaskState>,
    ) -> Result<Option<proto::Subtask>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let mut args = vec![tidbID.into(), TaskIDToKey(taskID).into(), step.into()];
        args.extend(states.iter().cloned().map(Value::from));
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_background_subtask where exec_id = %? and task_key = %? and step = %? and state in ({}%?) limit 1",
                SubtaskColumns,
                "%?,".repeat(states.len().saturating_sub(1))
            ),
            args,
        )?;
        Ok(rs.into_iter().next().map(Row2SubTask))
    }

    /// 取任务下 pending/running 活跃子任务基行。
    // GetActiveSubtasks implements TaskManager.GetActiveSubtasks.
    pub fn GetActiveSubtasks(
        &self,
        ctx: Context,
        taskID: i64,
    ) -> Result<Vec<proto::SubtaskBase>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_background_subtask where task_key = %? and state in (%?, %?)",
                basicSubtaskColumns
            ),
            vec![TaskIDToKey(taskID).into(), proto::SubtaskStatePending.into(), proto::SubtaskStateRunning.into()],
        )?;
        Ok(rs.into_iter().map(row2BasicSubTask).collect())
    }

    /// 按 step+state 取全部子任务。
    // GetAllSubtasksByStepAndState gets the subtask by step and state.
    pub fn GetAllSubtasksByStepAndState(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
        state: proto::SubtaskState,
    ) -> Result<Option<Vec<proto::Subtask>>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            format!(
                "select {} from mysql.tidb_background_subtask where task_key = %? and state = %? and step = %?",
                SubtaskColumns
            ),
            vec![TaskIDToKey(taskID).into(), state.into(), step.into()],
        )?;
        if rs.is_empty() {
            return Ok(None);
        }
        Ok(Some(rs.into_iter().map(Row2SubTask).collect()))
    }

    /// 按 step 读取各子任务 summary（运行中任务，不读 history）。
    // GetAllSubtaskSummaryByStep gets the subtask summaries by step.
    // Since it's only used for running jobs, we don't need to read from history table.
    pub fn GetAllSubtaskSummaryByStep(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
    ) -> Result<Option<Vec<execute::SubtaskSummary>>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select summary from mysql.tidb_background_subtask where task_key = %? and step = %?",
            vec![TaskIDToKey(taskID).into(), step.into()],
        )?;
        if rs.is_empty() {
            return Ok(None);
        }
        let mut summaries = Vec::with_capacity(rs.len());
        for row in rs {
            let summary =
                json::Unmarshal::<execute::SubtaskSummary>(hack::Slice(row.GetJSON(0).String()))
                    .map_err(errors::Trace)?;
            summaries.push(summary);
        }
        Ok(Some(summaries))
    }

    /// 汇总活跃+history 中该 step 的 summary.row_count。
    // GetSubtaskRowCount gets the subtask row count.
    pub fn GetSubtaskRowCount(
        &self,
        ctx: Context,
        taskID: i64,
        step: proto::Step,
    ) -> Result<i64, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select cast(sum(json_extract(summary, '$.row_count')) as signed) as row_count from (select summary from mysql.tidb_background_subtask where task_key = %? and step = %? union all select summary from mysql.tidb_background_subtask_history where task_key = %? and step = %?) as combined",
            vec![TaskIDToKey(taskID).into(), step.into(), TaskIDToKey(taskID).into(), step.into()],
        )?;
        if rs.is_empty() {
            return Ok(0);
        }
        Ok(rs[0].GetInt64(0))
    }
}
