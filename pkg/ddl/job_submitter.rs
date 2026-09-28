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

// DDL Job 提交器：接收 JobSpec、合并同 schema 的 CREATE TABLE 批量任务并持久化，
// 同时统计通知次数供调度侧感知新任务到达。

use crate::ddl::Job;
use std::collections::BTreeMap;

/// 待提交的 DDL 任务规格：含是否已分配 ID、外键标记，以及合并后的子 job 列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSpec {
    /// 主 Job 元数据。
    pub job: Job,
    /// 是否已预先分配全局 ID；已分配则不参与 CREATE TABLE 批量合并。
    pub id_allocated: bool,
    /// 是否含外键；含外键时不参与批量合并。
    pub has_foreign_keys: bool,
    /// 合并进本 Spec 的多个 CREATE TABLE Job（批量创建时填充）。
    pub merged_jobs: Vec<Job>,
}

impl JobSpec {
    /// 构造仅含主 Job 与 id_allocated 标志的规格（默认无外键、无合并子项）。
    pub fn new(job: Job, id_allocated: bool) -> Self {
        Self {
            job,
            id_allocated,
            has_foreign_keys: false,
            merged_jobs: Vec::new(),
        }
    }
}
/// 将 JobSpec 写入持久化映射并放入 pending 队列，供调度器拉取。
#[derive(Default)]
pub struct JobSubmitter {
    pending: Vec<JobSpec>,
    persisted: BTreeMap<i64, Job>,
    notifications: usize,
}
impl JobSubmitter {
    /// 提交一组 Job：先尝试合并 CREATE TABLE，再去重写入；有结果时递增通知计数。
    ///
    /// 返回与输入对应的每个 Job ID 的 `Ok`/`Err`（重复 ID 报错）。
    pub fn submit(&mut self, jobs: Vec<JobSpec>) -> Vec<Result<i64, String>> {
        let merged = merge_create_table_jobs(jobs);
        let mut results = Vec::new();
        for spec in merged {
            if self.persisted.contains_key(&spec.job.id) {
                results.push(Err(format!("DDL job {} already exists", spec.job.id)));
                continue;
            }
            self.persisted.insert(spec.job.id, spec.job.clone());
            self.pending.push(spec.clone());
            results.push(Ok(spec.job.id));
        }
        if results.iter().any(Result::is_ok) {
            self.notifications += 1;
        }
        results
    }
    /// 取出并清空当前 pending 队列。
    pub fn take_pending(&mut self) -> Vec<JobSpec> {
        std::mem::take(&mut self.pending)
    }
    /// 累计通知次数（每次非空 submit 结果递增一次）。
    pub fn notification_count(&self) -> usize {
        self.notifications
    }
}
/// 将同 schema、未分配 ID、无外键且状态为 None 的 CREATE TABLE 按批合并（每批最多 8 个）。
///
/// 合并后主 Job 的 query 为多条语句拼接；不可合并的 Spec 原样透传，最后按 job.id 排序。
pub fn merge_create_table_jobs(jobs: Vec<JobSpec>) -> Vec<JobSpec> {
    const MAX_BATCH_SIZE: usize = 8;

    let mut grouped: BTreeMap<i64, Vec<JobSpec>> = BTreeMap::new();
    let mut passthrough = Vec::new();
    for spec in jobs {
        // 仅无状态 CREATE TABLE 且未分配 ID、无外键时进入按 schema_id 分组。
        if spec
            .job
            .query
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("create table")
            && !spec.id_allocated
            && !spec.has_foreign_keys
        {
            grouped.entry(spec.job.schema_id).or_default().push(spec);
        } else {
            passthrough.push(spec);
        }
    }
    for (_, mut group) in grouped {
        group.sort_by_key(|spec| spec.job.id);
        // 将组内任务尽量均匀拆成若干不超过 MAX_BATCH_SIZE 的批次。
        let batch_count = group.len().div_ceil(MAX_BATCH_SIZE);
        let base_batch_size = group.len() / batch_count;
        let larger_batches = group.len() % batch_count;
        let mut group = group.into_iter();
        for batch_index in 0..batch_count {
            let batch_size = base_batch_size + usize::from(batch_index < larger_batches);
            let mut batch = group.by_ref().take(batch_size).collect::<Vec<_>>();
            if batch.len() == 1 {
                passthrough.push(batch.remove(0));
                continue;
            }
            // 以第一个 Spec 为宿主，其余写入 merged_jobs，并重写拼接后的 query。
            let mut merged = batch.remove(0);
            merged.merged_jobs = std::iter::once(merged.job.clone())
                .chain(batch.iter().map(|spec| spec.job.clone()))
                .collect();
            merged.job.query = build_query_string_from_jobs(
                &std::iter::once(&merged)
                    .chain(batch.iter())
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            passthrough.push(merged);
        }
    }
    passthrough.sort_by_key(|spec| spec.job.id);
    passthrough
}
/// 将多个 Job 的 query 去尾部分号后用 `"; "` 拼接，并保证末尾有分号。
pub fn build_query_string_from_jobs(jobs: &[JobSpec]) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    jobs.iter()
        .map(|spec| {
            let query = spec.job.query.trim();
            if query.ends_with(';') {
                query.to_owned()
            } else {
                format!("{query};")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
