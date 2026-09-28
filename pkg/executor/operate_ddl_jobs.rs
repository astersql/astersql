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

// DDL 作业管理执行器：取消 / 暂停 / 恢复作业，以及运行时改写重组参数。
//
// DDL（Data Definition Language，数据定义语言）作业由后台调度执行。
// 本模块将 ADMIN CANCEL/PAUSE/RESUME DDL JOBS 与 ALTER DDL JOB 映射为
// Open/Next 风格执行器：Open 阶段在系统会话中真正下发命令，Next 阶段
// 按批把每个 job_id 的成功或错误信息写入结果 Chunk。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;

/// 取消 / 暂停 / 恢复 DDL 作业所需的后端能力抽象。
///
/// 具体实现负责创建系统会话、调用 DDL owner、格式化错误等；
/// 执行器本身只编排生命周期与结果输出。
pub trait DDLJobCommandBackend {
    /// 会话侧上下文（如 sessionctx）。
    type Context;
    /// 系统会话句柄。
    type Session;
    /// 后端错误类型。
    type Error;

    /// 取得用于管理命令的全局系统会话。
    fn system_session(&mut self) -> Result<Self::Session, Self::Error>;
    /// 对一组 job_id 执行取消 / 暂停 / 恢复，返回逐作业错误与整体结果。
    fn execute(
        &mut self,
        ctx: &mut Self::Context,
        session: &mut Self::Session,
        job_ids: &[i64],
    ) -> (Vec<Option<Self::Error>>, Result<(), Self::Error>);
    /// 归还系统会话到会话池。
    fn release_system_session(&mut self, session: Self::Session);
    /// 结果 Chunk 的最大行容量。
    fn max_chunk_size(&self) -> usize;
    /// 将后端错误格式化为可读字符串。
    fn format_error(&self, error: &Self::Error) -> String;
}

/// 通用 DDL 作业命令执行器：Open 下发命令，Next 输出 job_id 与结果列。
pub struct CommandDDLJobsExec<B: DDLJobCommandBackend> {
    /// 后端实现。
    pub backend: B,
    /// Next 已写出的行游标。
    pub cursor: usize,
    /// 待操作的 DDL 作业 ID 列表。
    pub job_ids: Vec<i64>,
    /// 与 `job_ids` 对齐的逐作业错误；`None` 表示成功。
    pub errors: Vec<Option<B::Error>>,
}

impl<B: DDLJobCommandBackend> CommandDDLJobsExec<B> {
    /// 打开执行器：在系统会话事务中执行管理命令并缓存逐作业错误。
    pub fn Open(&mut self, ctx: &mut B::Context) -> Result<(), B::Error> {
        // A global system-session transaction is required for admin commands.
        // 管理命令必须走全局系统会话事务，避免污染用户会话。
        let mut session = self.backend.system_session()?;
        let (errors, result) = self.backend.execute(ctx, &mut session, &self.job_ids);
        self.errors = errors;
        self.backend.release_system_session(session);
        result
    }

    /// 拉取下一结果批：两列分别为 job_id 字符串与 "successful"/错误信息。
    pub fn Next<C>(&mut self, _ctx: C, request: &mut Chunk) -> Result<(), B::Error> {
        request.GrowAndReset(self.backend.max_chunk_size());
        if self.cursor >= self.job_ids.len() {
            return Ok(());
        }
        let batch_size = request.Capacity().min(self.job_ids.len() - self.cursor);
        for index in self.cursor..self.cursor + batch_size {
            request.AppendString(0, &self.job_ids[index].to_string());
            // 有错误则输出 "error: ..."，否则输出 "successful"。
            let result = self
                .errors
                .get(index)
                .and_then(Option::as_ref)
                .map(|error| format!("error: {}", self.backend.format_error(error)))
                .unwrap_or_else(|| "successful".to_owned());
            request.AppendString(1, &result);
        }
        self.cursor += batch_size;
        Ok(())
    }
}

/// ADMIN CANCEL DDL JOBS 执行器。
pub struct CancelDDLJobsExec<B: DDLJobCommandBackend>(pub CommandDDLJobsExec<B>);
/// ADMIN PAUSE DDL JOBS 执行器。
pub struct PauseDDLJobsExec<B: DDLJobCommandBackend>(pub CommandDDLJobsExec<B>);
/// ADMIN RESUME DDL JOBS 执行器。
pub struct ResumeDDLJobsExec<B: DDLJobCommandBackend>(pub CommandDDLJobsExec<B>);

/// ALTER DDL JOB 可修改的重组参数名。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlterDDLJobOptionName {
    /// 重组并发线程数（concurrency）。
    Thread,
    /// 每批处理的行数。
    BatchSize,
    /// 最大写吞吐上限。
    MaxWriteSpeed,
}

/// 单条 ALTER DDL JOB 选项：名称 + 可选整型取值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlterDDLJobOption {
    /// 选项名。
    pub name: AlterDDLJobOptionName,
    /// 选项值；`None` 表示跳过该项。
    pub value: Option<i64>,
}

/// 改写 DDL 作业配置时的最大事务重试次数。
pub const alterDDLJobMaxRetryCnt: usize = 3;

/// ALTER DDL JOB 所需的后端能力：事务、读改写作业元数据、校验可改性。
pub trait AlterDDLJobBackend {
    /// 会话侧上下文。
    type Context;
    /// 系统会话句柄。
    type Session;
    /// DDL 作业元数据类型。
    type Job;
    /// 后端错误类型。
    type Error;

    /// 取得系统会话。
    fn system_session(&mut self) -> Result<Self::Session, Self::Error>;
    /// 归还系统会话。
    fn release_system_session(&mut self, session: Self::Session);
    /// 开启事务。
    fn begin(
        &mut self,
        ctx: &mut Self::Context,
        session: &mut Self::Session,
    ) -> Result<(), Self::Error>;
    /// 按 job_id 读取作业。
    fn get_job(
        &mut self,
        ctx: &mut Self::Context,
        session: &mut Self::Session,
        job_id: i64,
    ) -> Result<Self::Job, Self::Error>;
    /// 作业当前状态是否允许改写重组元数据。
    fn is_alterable(&self, job: &Self::Job) -> bool;
    /// 返回作业操作名（用于不支持错误消息）。
    fn operation_name(&self, job: &Self::Job) -> String;
    /// 是否为下一代加索引作业（暂不支持 ALTER）。
    fn is_next_generation_add_index(&self, job: &Self::Job) -> bool;
    /// 构造“操作不支持 ALTER”错误。
    fn unsupported_operation(&self, operation: &str) -> Self::Error;
    /// 构造“下一代加索引不支持 ALTER”错误。
    fn unsupported_next_generation_add_index(&self) -> Self::Error;
    /// 写入并发度。
    fn set_concurrency(&self, job: &mut Self::Job, value: i64);
    /// 写入批大小。
    fn set_batch_size(&self, job: &mut Self::Job, value: i64);
    /// 写入最大写速度。
    fn set_max_write_speed(&self, job: &mut Self::Job, value: i64);
    /// 标记管理操作的终端用户信息。
    fn set_admin_operator_end_user(&self, job: &mut Self::Job);
    /// 将修改后的作业写回存储。
    fn update_job(
        &mut self,
        ctx: &mut Self::Context,
        session: &mut Self::Session,
        job: &Self::Job,
    ) -> Result<(), Self::Error>;
    /// 测试注入：提交前强制失败（返回 Some 则回滚）。
    fn inject_commit_failure(&self) -> Option<Self::Error>;
    /// 提交事务。
    fn commit(
        &mut self,
        ctx: &mut Self::Context,
        session: &mut Self::Session,
    ) -> Result<(), Self::Error>;
    /// 回滚事务。
    fn rollback(&mut self, session: &mut Self::Session);
}

/// ALTER DDL JOB 执行器：在系统会话中带重试地改写重组元数据。
pub struct AlterDDLJobExec<B: AlterDDLJobBackend> {
    /// 后端实现。
    pub backend: B,
    /// 目标作业 ID。
    pub job_id: i64,
    /// 待应用的选项列表。
    pub alter_options: Vec<AlterDDLJobOption>,
}

impl<B: AlterDDLJobBackend> AlterDDLJobExec<B> {
    /// 打开执行器并完成配置改写。
    pub fn Open(&mut self, ctx: &mut B::Context) -> Result<(), B::Error> {
        let mut session = self.backend.system_session()?;
        let result = self.processAlterDDLJobConfig(ctx, &mut session);
        self.backend.release_system_session(session);
        result
    }

    /// 带有限次重试的事务流程：begin → 读作业 → 校验 → 改元数据 → 更新 → commit。
    pub fn processAlterDDLJobConfig(
        &mut self,
        ctx: &mut B::Context,
        session: &mut B::Session,
    ) -> Result<(), B::Error> {
        let mut last_error = None;
        // 并发写作业表可能冲突，最多重试 alterDDLJobMaxRetryCnt 次。
        for _ in 0..alterDDLJobMaxRetryCnt {
            if let Err(error) = self.backend.begin(ctx, session) {
                last_error = Some(error);
                continue;
            }
            let mut job = match self.backend.get_job(ctx, session, self.job_id) {
                Ok(job) => job,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            // 作业类型或状态不允许改写时直接失败，不重试。
            if !self.backend.is_alterable(&job) {
                return Err(self
                    .backend
                    .unsupported_operation(&self.backend.operation_name(&job)));
            }
            if self.backend.is_next_generation_add_index(&job) {
                return Err(self.backend.unsupported_next_generation_add_index());
            }
            self.updateReorgMeta(&mut job);
            if let Err(error) = self.backend.update_job(ctx, session, &job) {
                last_error = Some(error);
                continue;
            }
            // 注入点：模拟提交失败以便测试回滚路径。
            if let Some(error) = self.backend.inject_commit_failure() {
                self.backend.rollback(session);
                return Err(error);
            }
            if let Err(error) = self.backend.commit(ctx, session) {
                self.backend.rollback(session);
                last_error = Some(error);
                continue;
            }
            return Ok(());
        }
        Err(last_error.expect("three alter-DDL retries must retain their last error"))
    }

    /// 按选项列表更新作业的重组（reorg）元数据，并标记管理操作者。
    pub fn updateReorgMeta(&self, job: &mut B::Job) {
        for option in &self.alter_options {
            let Some(value) = option.value else {
                continue;
            };
            match option.name {
                AlterDDLJobOptionName::Thread => self.backend.set_concurrency(job, value),
                AlterDDLJobOptionName::BatchSize => self.backend.set_batch_size(job, value),
                AlterDDLJobOptionName::MaxWriteSpeed => {
                    self.backend.set_max_write_speed(job, value)
                }
            }
            self.backend.set_admin_operator_end_user(job);
        }
    }
}
