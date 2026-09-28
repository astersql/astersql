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

// `ADMIN SHOW DDL JOB QUERIES` 执行器：按 Job ID 或 offset/limit 展示 DDL SQL。
//
// 在内部 DDL 事务中合并运行中与历史作业（按 Job ID 去重），再按批写入字符串 Chunk。

use std::collections::HashSet;

/// 默认拉取的历史作业条数上限。
pub const DEFAULT_HISTORY_JOB_COUNT: usize = 10;

/// DDL 作业最小接口：标识与原始查询文本。
pub trait DDLJob {
    fn id(&self) -> i64;
    fn query(&self) -> &str;
}

/// Storage/session boundary used by the two SHOW DDL JOB QUERIES executors.
///
/// `with_internal_ddl_transaction` must create a system-session transaction,
/// mark that session as in-transaction, and release it with the internal DDL
/// source type after `operation` returns. This is the Rust ownership boundary
/// for the deferred rollback performed by the Go implementation.
///
/// 存储/会话边界：内部 DDL 事务须在系统会话上开启，标记 in-transaction，
/// 并在 `operation` 返回后按内部 DDL 源类型释放（对应 Go 侧延迟回滚）。
pub trait DDLJobSource {
    type Error;
    type Job: DDLJob;
    type Context;

    fn open_base(&mut self, context: &Self::Context) -> Result<(), Self::Error>;

    /// 在内部 DDL 事务中执行 `operation`，保证资源按所有权边界释放。
    fn with_internal_ddl_transaction<R>(
        &mut self,
        context: &Self::Context,
        operation: impl FnOnce(&mut Self) -> Result<R, Self::Error>,
    ) -> Result<R, Self::Error>;

    fn running_jobs(&mut self, context: &Self::Context) -> Result<Vec<Self::Job>, Self::Error>;

    fn history_jobs(&mut self, limit: usize) -> Result<Vec<Self::Job>, Self::Error>;

    fn max_chunk_size(&self) -> usize;
}

/// 仅含字符串列的结果 Chunk 抽象。
pub trait StringChunk {
    fn grow_and_reset(&mut self, max_chunk_size: usize);
    fn capacity(&self) -> usize;
    fn append_string(&mut self, column: usize, value: &str);
}

/// 将运行中与历史作业按 Job ID 去重后追加到 `target`（运行中优先）。
fn append_distinct_jobs<J: DDLJob>(target: &mut Vec<J>, running: Vec<J>, history: Vec<J>) {
    let mut appended_job_ids = HashSet::new();
    for job in running.into_iter().chain(history) {
        if appended_job_ids.insert(job.id()) {
            target.push(job);
        }
    }
}

/// Executor for `ADMIN SHOW DDL JOB QUERIES`.
/// 按给定 Job ID 列表过滤并输出对应查询文本。
pub struct ShowDDLJobQueriesExec<S>
where
    S: DDLJobSource,
{
    pub BaseExecutor: S,
    pub cursor: usize,
    pub jobs: Vec<S::Job>,
    pub jobIDs: Vec<i64>,
}

impl<S> ShowDDLJobQueriesExec<S>
where
    S: DDLJobSource,
{
    /// 打开执行器：在内部事务中加载运行中与默认数量历史作业。
    pub fn Open(&mut self, context: &S::Context) -> Result<(), S::Error> {
        self.BaseExecutor.open_base(context)?;
        let (running, history) =
            self.BaseExecutor
                .with_internal_ddl_transaction(context, |source| {
                    let running = source.running_jobs(context)?;
                    let history = source.history_jobs(DEFAULT_HISTORY_JOB_COUNT)?;
                    Ok((running, history))
                })?;
        append_distinct_jobs(&mut self.jobs, running, history);
        Ok(())
    }

    /// 按批扫描当前窗口内作业，命中 `jobIDs` 则写出 query。
    pub fn Next<Q: StringChunk>(&mut self, request: &mut Q) -> Result<(), S::Error> {
        request.grow_and_reset(self.BaseExecutor.max_chunk_size());
        if self.cursor >= self.jobs.len() {
            return Ok(());
        }
        if self.jobIDs.len() >= self.jobs.len() {
            return Ok(());
        }

        let current_batch = request.capacity().min(self.jobs.len() - self.cursor);
        for id in &self.jobIDs {
            for job in &self.jobs[self.cursor..self.cursor + current_batch] {
                if *id == job.id() {
                    request.append_string(0, job.query());
                }
            }
        }
        self.cursor += current_batch;
        Ok(())
    }
}

/// Executor for the offset/limit form of `ADMIN SHOW DDL JOB QUERIES`.
/// 带 offset/limit 的形式：输出 Job ID 与 query 两列。
pub struct ShowDDLJobQueriesWithRangeExec<S>
where
    S: DDLJobSource,
{
    pub BaseExecutor: S,
    pub cursor: usize,
    pub jobs: Vec<S::Job>,
    pub offset: u64,
    pub limit: u64,
}

impl<S> ShowDDLJobQueriesWithRangeExec<S>
where
    S: DDLJobSource,
{
    /// 打开执行器：按 offset+limit 计算历史拉取量，并将游标跳到 offset。
    pub fn Open(&mut self, context: &S::Context) -> Result<(), S::Error> {
        self.BaseExecutor.open_base(context)?;
        let history_limit = self.offset.wrapping_add(self.limit) as usize;
        let (running, history) =
            self.BaseExecutor
                .with_internal_ddl_transaction(context, |source| {
                    let running = source.running_jobs(context)?;
                    let history = source.history_jobs(history_limit)?;
                    Ok((running, history))
                })?;
        append_distinct_jobs(&mut self.jobs, running, history);

        let offset = self.offset as usize;
        if self.cursor < offset {
            self.cursor = offset;
        }
        Ok(())
    }

    /// 在 [offset, offset+limit) 窗口内按批写出 id 与 query。
    pub fn Next<Q: StringChunk>(&mut self, request: &mut Q) -> Result<(), S::Error> {
        request.grow_and_reset(self.BaseExecutor.max_chunk_size());
        if self.cursor >= self.jobs.len() {
            return Ok(());
        }
        if self.offset as usize > self.jobs.len() {
            return Ok(());
        }

        let current_batch = request.capacity().min(self.jobs.len() - self.cursor);
        let end = self.offset.wrapping_add(self.limit) as usize;
        for (row_offset, job) in self.jobs[self.cursor..self.cursor + current_batch]
            .iter()
            .enumerate()
        {
            if self.cursor + row_offset >= end {
                break;
            }
            let id = job.id().to_string();
            request.append_string(0, &id);
            request.append_string(1, job.query());
        }
        self.cursor += current_batch;
        Ok(())
    }
}
