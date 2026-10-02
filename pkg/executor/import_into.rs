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

// `IMPORT INTO` 语句执行器：校验列赋值表达式、提交导入任务并等待完成。
//
// 支持从文件路径或 SELECT 管线导入；可走单机或分布式任务，
// 以及取消已有导入作业。运行时依赖通过 `ImportIntoRuntime` 注入。

#![allow(non_snake_case)]

/// 列赋值表达式抽象：标量函数名、所需可选属性位与子节点。
pub trait ImportExpression {
    fn scalar_function_name(&self) -> Option<&str>;
    fn required_optional_properties(&self) -> u64;
    fn children(&self) -> &[Self]
    where
        Self: Sized;
}

/// 递归检查表达式所需可选属性是否被编码上下文覆盖。
pub fn checkExprWithProvidedProps<E: ImportExpression>(
    index: usize,
    expression: &E,
    provided_properties: u64,
) -> Result<(), UnsupportedImportFunction> {
    if let Some(function_name) = expression.scalar_function_name() {
        if expression.required_optional_properties() | provided_properties != provided_properties {
            return Err(UnsupportedImportFunction {
                function_name: function_name.to_owned(),
                assignment_index: index,
            });
        }
        for child in expression.children() {
            checkExprWithProvidedProps(index, child, provided_properties)?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列赋值中出现编码上下文不支持的函数时抛出。
pub struct UnsupportedImportFunction {
    pub function_name: String,
    pub assignment_index: usize,
}

impl std::fmt::Display for UnsupportedImportFunction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "FUNCTION {} is not supported in IMPORT INTO column assignment, index {}",
            self.function_name, self.assignment_index
        )
    }
}

impl std::error::Error for UnsupportedImportFunction {}

/// `IMPORT INTO` 执行所需的会话/计划/任务/权限等运行时钩子。
pub trait ImportIntoRuntime {
    type Context;
    type Request;
    type Plan;
    type Table;
    type SelectExecutor;
    type ImportPlan;
    type Assignment;
    type Expression: ImportExpression;
    type Controller;
    type Task;
    type JobInfo;
    type Error: From<UnsupportedImportFunction>;

    /// 重置并扩展结果集请求缓冲。
    fn grow_and_reset_request(&self, request: &mut Self::Request);
    /// 由逻辑计划与目标表构建导入计划。
    fn create_import_plan(
        &mut self,
        context: &mut Self::Context,
        plan: &Self::Plan,
        table: &Self::Table,
    ) -> Result<Self::ImportPlan, Self::Error>;
    /// 取出列赋值列表。
    fn column_assignments(&self, plan: &Self::Plan) -> Vec<Self::Assignment>;
    /// 编码上下文当前可提供的可选属性位掩码。
    fn encoding_optional_properties(
        &mut self,
        import_plan: &Self::ImportPlan,
    ) -> Result<u64, Self::Error>;
    /// 将单条赋值编译为可校验的表达式树。
    fn build_assignment_expression(
        &mut self,
        import_plan: &Self::ImportPlan,
        assignment: &Self::Assignment,
    ) -> Result<Self::Expression, Self::Error>;
    /// 创建导入控制器（路径、配置、作业状态等）。
    fn create_controller(
        &mut self,
        import_plan: Self::ImportPlan,
        table: &Self::Table,
        plan: &Self::Plan,
    ) -> Result<Self::Controller, Self::Error>;
    /// 是否延迟到后台再初始化数据文件。
    fn should_use_async_prepare(&self, controller: &Self::Controller) -> bool;
    /// 解析并打开待导入数据文件。
    fn initialize_data_files(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
    ) -> Result<(), Self::Error>;
    /// 是否启用新一代导入内核（需额外计算资源参数）。
    fn next_generation_kernel(&self) -> bool;
    /// 按集群与文件规模估算导入资源参数。
    fn calculate_resource_parameters(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
    ) -> Result<(), Self::Error>;
    /// Must create and close a fresh session so processlist and stale-read
    /// state from the user's session cannot leak into pre-checks.
    fn check_requirements_in_new_session(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
        before_file_initialization: bool,
    ) -> Result<(), Self::Error>;
    /// 写入导入所需的 TiKV 侧配置。
    /// TiKV：分布式 KV 存储引擎，Region 为数据分片。
    fn initialize_tikv_configs(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
    ) -> Result<(), Self::Error>;
    /// 作业是否以 detached（提交后不等待）方式运行。
    fn controller_is_detached(&self, controller: &Self::Controller) -> bool;
    /// 导入数据源路径。
    fn controller_path(&self, controller: &Self::Controller) -> &str;
    /// 路径是否为本地文件系统。
    fn path_is_local(&self, path: &str) -> Result<bool, Self::Error>;
    /// 是否允许提交分布式导入任务。
    fn distributed_tasks_enabled(&self) -> bool;
    /// 本地路径下预先切分数据 chunk。
    /// chunk：一段连续待编码的数据切片。
    fn populate_chunks(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
    ) -> Result<(), Self::Error>;
    /// 提交单机导入任务。
    fn submit_standalone_task(
        &mut self,
        context: &mut Self::Context,
        controller: &Self::Controller,
        statement: &str,
        populated_chunks: bool,
    ) -> Result<(i64, Self::Task), Self::Error>;
    /// 提交分布式导入任务。
    fn submit_distributed_task(
        &mut self,
        context: &mut Self::Context,
        controller: &Self::Controller,
        statement: &str,
    ) -> Result<(i64, Self::Task), Self::Error>;
    /// 阻塞直到任务完成或暂停。
    fn wait_task_done_or_paused(
        &mut self,
        context: &mut Self::Context,
        task: &Self::Task,
    ) -> Result<(), Self::Error>;
    /// 错误是否因会话上下文取消引起。
    fn error_is_context_cancelled(&self, error: &Self::Error) -> bool;
    /// 后台取消导入作业并等待结束。
    fn cancel_and_wait_import_job_background(&mut self, job_id: i64) -> Result<(), Self::Error>;
    /// 用系统会话读取作业信息。
    fn get_job_with_system_session(
        &mut self,
        context: &mut Self::Context,
        job_id: i64,
    ) -> Result<Self::JobInfo, Self::Error>;
    /// 将作业信息写入结果请求。
    fn fill_one_job_info(&self, request: &mut Self::Request, job: &Self::JobInfo);
    /// Runs the select producer and table importer concurrently, uses fresh
    /// chunks (not the session pool), doubles chunk capacity up to max, closes
    /// the channel on producer exit, flushes stats best-effort, and records the
    /// affected rows/message exactly as the Go pipeline.
    fn import_from_select_pipeline(
        &mut self,
        context: &mut Self::Context,
        controller: &mut Self::Controller,
        select_executor: &mut Self::SelectExecutor,
    ) -> Result<(), Self::Error>;
    /// 关闭导入控制器并释放资源。
    fn close_controller(&mut self, controller: &mut Self::Controller);
    /// 关闭基类执行器。
    fn close_base_executor(&mut self) -> Result<(), Self::Error>;

    /// 当前用户是否具备 SUPER 权限。
    fn has_super_privilege(&self) -> bool;
    /// 按权限读取待操作的作业。
    fn get_job_for_action(
        &mut self,
        context: &mut Self::Context,
        job_id: i64,
        has_super_privilege: bool,
    ) -> Result<Self::JobInfo, Self::Error>;
    /// 作业当前状态是否允许取消。
    fn job_can_cancel(&self, job: &Self::JobInfo) -> bool;
    /// 构造“不可取消”错误。
    fn invalid_cancel_operation(&self) -> Self::Error;
    /// 取消导入作业并等待结束。
    fn cancel_and_wait_import_job(
        &mut self,
        context: &mut Self::Context,
        job_id: i64,
    ) -> Result<(), Self::Error>;
}

/// 用编码上下文提供的属性位校验全部列赋值表达式。
pub fn ValidateImportIntoColAssignmentsWithEncodeCtx<R: ImportIntoRuntime>(
    runtime: &mut R,
    import_plan: &R::ImportPlan,
    assignments: &[R::Assignment],
) -> Result<(), R::Error> {
    let provided = runtime.encoding_optional_properties(import_plan)?;
    for (index, assignment) in assignments.iter().enumerate() {
        let expression = runtime.build_assignment_expression(import_plan, assignment)?;
        checkExprWithProvidedProps(index, &expression, provided)?;
    }
    Ok(())
}

/// `IMPORT INTO` 主执行器状态。
pub struct ImportIntoExec<R: ImportIntoRuntime> {
    pub runtime: R,
    pub select_executor: Option<R::SelectExecutor>,
    pub controller: Option<R::Controller>,
    pub statement: String,
    pub plan: R::Plan,
    pub table: R::Table,
    pub data_filled: bool,
}

/// 构造尚未执行的导入执行器。
pub fn newImportIntoExec<R: ImportIntoRuntime>(
    runtime: R,
    select_executor: Option<R::SelectExecutor>,
    statement: String,
    plan: R::Plan,
    table: R::Table,
) -> ImportIntoExec<R> {
    ImportIntoExec {
        runtime,
        select_executor,
        controller: None,
        statement,
        plan,
        table,
        data_filled: false,
    }
}

impl<R: ImportIntoRuntime> ImportIntoExec<R> {
    /// 执行导入：建计划、校验赋值、提交任务，必要时等待并填充作业信息。
    /// 若存在 SELECT 子执行器则走 `importFromSelect`。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.runtime.grow_and_reset_request(request);
        if self.data_filled {
            return Ok(());
        }
        let import_plan = self
            .runtime
            .create_import_plan(context, &self.plan, &self.table)?;
        let assignments = self.runtime.column_assignments(&self.plan);
        ValidateImportIntoColAssignmentsWithEncodeCtx(
            &mut self.runtime,
            &import_plan,
            &assignments,
        )?;
        self.controller = Some(self.runtime.create_controller(
            import_plan,
            &self.table,
            &self.plan,
        )?);

        // SELECT 源：走并发管线，跳过文件任务提交。
        if self.select_executor.is_some() {
            return self.importFromSelect(context);
        }
        let controller = self.controller.as_mut().expect("initialized above");
        let async_prepare = self.runtime.should_use_async_prepare(controller);
        if !async_prepare {
            self.runtime.initialize_data_files(context, controller)?;
            if self.runtime.next_generation_kernel() {
                self.runtime
                    .calculate_resource_parameters(context, controller)?;
            }
        }
        self.runtime
            .check_requirements_in_new_session(context, controller, async_prepare)?;
        self.runtime.initialize_tikv_configs(context, controller)?;
        let (job_id, task) = self.submitTask(context)?;
        if !self
            .runtime
            .controller_is_detached(self.controller.as_ref().expect("initialized"))
        {
            self.waitTask(context, job_id, &task)?;
        }
        self.fillJobInfo(context, job_id, request)
    }

    /// 标记结果已填充，并通过系统会话写出作业信息。
    pub fn fillJobInfo(
        &mut self,
        context: &mut R::Context,
        job_id: i64,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.data_filled = true;
        let job = self.runtime.get_job_with_system_session(context, job_id)?;
        self.runtime.fill_one_job_info(request, &job);
        Ok(())
    }

    /// 按路径本地性与分布式开关选择提交方式。
    pub fn submitTask(&mut self, context: &mut R::Context) -> Result<(i64, R::Task), R::Error> {
        let controller = self.controller.as_mut().expect("controller initialized");
        let local = self
            .runtime
            .path_is_local(self.runtime.controller_path(controller))?;
        // 本地文件先切 chunk 再提交单机任务。
        if local {
            self.runtime.populate_chunks(context, controller)?;
            return self
                .runtime
                .submit_standalone_task(context, controller, &self.statement, true);
        }
        if self.runtime.distributed_tasks_enabled() {
            self.runtime
                .submit_distributed_task(context, controller, &self.statement)
        } else {
            self.runtime
                .submit_standalone_task(context, controller, &self.statement, false)
        }
    }

    /// 等待任务；若因上下文取消则后台取消作业。
    pub fn waitTask(
        &mut self,
        context: &mut R::Context,
        job_id: i64,
        task: &R::Task,
    ) -> Result<(), R::Error> {
        match self.runtime.wait_task_done_or_paused(context, task) {
            Err(error) if self.runtime.error_is_context_cancelled(&error) => {
                self.runtime.cancel_and_wait_import_job_background(job_id)
            }
            result => result,
        }
    }

    /// 从 SELECT 管线并发导入，不再走文件任务提交路径。
    pub fn importFromSelect(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.data_filled = true;
        self.runtime.import_from_select_pipeline(
            context,
            self.controller.as_mut().expect("controller initialized"),
            self.select_executor
                .as_mut()
                .expect("select executor checked"),
        )
    }

    /// 关闭控制器与基类执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        if let Some(controller) = &mut self.controller {
            self.runtime.close_controller(controller);
        }
        self.runtime.close_base_executor()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 对已有导入作业的管理动作。
pub enum ImportIntoAction {
    Cancel,
}

/// 执行 `IMPORT INTO` 作业管理语句（如 CANCEL）。
pub struct ImportIntoActionExec<R: ImportIntoRuntime> {
    pub runtime: R,
    pub action: ImportIntoAction,
    pub job_id: i64,
}

impl<R: ImportIntoRuntime> ImportIntoActionExec<R> {
    /// 校验权限与作业状态后执行取消。
    pub fn Next<T>(&mut self, context: &mut R::Context, _request: &mut T) -> Result<(), R::Error> {
        self.checkPrivilegeAndStatus(context)?;
        cancelAndWaitImportJob(&mut self.runtime, context, self.job_id)
    }

    /// 确认当前用户可操作且作业处于可取消状态。
    pub fn checkPrivilegeAndStatus(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        let has_super_privilege = self.runtime.has_super_privilege();
        let job = self
            .runtime
            .get_job_for_action(context, self.job_id, has_super_privilege)?;
        if self.runtime.job_can_cancel(&job) {
            Ok(())
        } else {
            Err(self.runtime.invalid_cancel_operation())
        }
    }
}

/// 委托运行时取消并等待导入作业。
pub fn cancelAndWaitImportJob<R: ImportIntoRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    job_id: i64,
) -> Result<(), R::Error> {
    runtime.cancel_and_wait_import_job(context, job_id)
}

/// Cancel using the DXF manager and the job's keyspace manager. The initial
/// probe follows the later Go race fix; a missed task is never waited for.
pub fn cancelAndWaitImportJobInStorage(
    context: &astersql_dxf_framework_handle::Context,
    job_id: i64,
    task_manager: &astersql_dxf_framework_storage::TaskManager,
    job_manager: &astersql_dxf_framework_storage::TaskManager,
) -> Result<(), astersql_dxf_framework_storage::Error> {
    cancelImportJobWithFallbackHook(context, job_id, task_manager, job_manager, || {})
}

pub(crate) fn cancelImportJobWithFallbackHook(
    context: &astersql_dxf_framework_handle::Context,
    job_id: i64,
    task_manager: &astersql_dxf_framework_storage::TaskManager,
    job_manager: &astersql_dxf_framework_storage::TaskManager,
    before_fallback: impl FnOnce(),
) -> Result<(), astersql_dxf_framework_storage::Error> {
    use astersql_dxf_framework_storage as storage;
    let key = astersql_dxf_importinto::TaskKey(job_id);
    match task_manager.GetTaskBaseByKeyWithHistory((), key.clone()) {
        Ok(_) => {
            task_manager.WithNewTxn((), |session| {
                task_manager.CancelTaskByKeySession((), session, key.clone())
            })?;
            astersql_dxf_framework_handle::WaitTaskDoneByKey(context, &key)
                .map_err(|error| storage::Error::new(error.to_string()))
        }
        Err(error) if error == storage::ErrTaskNotFound => {
            before_fallback();
            cancelDanglingImportJob(job_manager, job_id)
        }
        Err(error) => Err(error),
    }
}

/// Atomically cancel only a pending import job in its own keyspace.
pub fn cancelDanglingImportJob(
    manager: &astersql_dxf_framework_storage::TaskManager,
    job_id: i64,
) -> Result<(), astersql_dxf_framework_storage::Error> {
    use astersql_dxf_framework_storage as storage;
    manager.WithNewSession(|session| {
        let mut executor = astersql_dxf_importinto::scheduler::ImportJobStorageSession {
            executor: session.GetSQLExecutor(),
        };
        astersql_executor_importer::CancelPendingJob(&mut executor, job_id)
            .map_err(storage::Error::new)?;
        if session.GetSessionVars().StmtCtx.AffectedRows() == 0 {
            return Err(storage::Error::new(
                "job state changed during cancel, please try again later",
            ));
        }
        Ok(())
    })
}
