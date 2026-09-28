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

// DDL 历史任务存储模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE TABLE、ALTER TABLE
// 等修改库表结构的语句。数据库内核会把每条 DDL 语句封装成一个 DDL Job（任务），
// 任务执行完成后（无论成功、回滚还是取消）都会被归档到"历史任务"存储中，
// 供 `ADMIN SHOW DDL JOBS` 等命令查询历史执行记录。
//
// 本模块提供 [`HistoryStore`]，一个基于内存的历史 DDL 任务存储实现：
// 内部按任务 ID 降序（最新的任务在前）维护已完成的 Job 列表，
// 支持按 ID 查询、取最近 N 条、分批遍历以及带起始 ID 与条数限制的扫描。

use crate::ddl::Job;

/// Go `DefNumGetDDLHistoryJobs`: default cap for an unbounded history scan.
pub const DEFAULT_SCAN_LIMIT: usize = 2048;

/// 历史 DDL 任务存储。
///
/// 保存所有已完成的 DDL Job，内部始终按任务 ID 降序排列
/// （ID 越大表示任务越新，因此列表头部是最近完成的任务）。
#[derive(Clone, Debug, Default)]
pub struct HistoryStore {
    /// 已完成的 DDL 任务列表，按 `Job.id` 降序排列。
    jobs: Vec<Job>,
}
impl HistoryStore {
    /// 将一个已完成的 DDL 任务归档到历史存储。
    ///
    /// 若已存在相同 ID 的任务则原地覆盖（幂等写入，避免重复归档产生重复记录），
    /// 否则追加为新记录；写入后重新按 ID 降序排序，保证最新任务始终排在最前。
    ///
    /// `_update_raw_args` 是从 Go 版本迁移保留的参数（原用于控制是否同时
    /// 更新任务的原始参数编码），当前实现未使用。
    pub fn add_history_job(&mut self, job: Job, _update_raw_args: bool) {
        // 先查找是否已有同 ID 的任务：有则覆盖，无则追加。
        if let Some(position) = self.jobs.iter().position(|item| item.id == job.id) {
            self.jobs[position] = job;
        } else {
            self.jobs.push(job);
        }
        // 使用 Reverse 包装 ID 实现降序排序，使最新（ID 最大）的任务位于列表头部。
        self.jobs.sort_by_key(|job| std::cmp::Reverse(job.id));
    }
    /// 按任务 ID 查询历史任务，返回其克隆副本；不存在时返回 `None`。
    pub fn get_by_id(&self, id: i64) -> Option<Job> {
        self.jobs.iter().find(|job| job.id == id).cloned()
    }
    /// 返回最近完成的至多 `maximum` 条历史任务。
    ///
    /// 由于内部列表按 ID 降序排列，直接取前 `maximum` 个即为最新的任务。
    pub fn last_n(&self, maximum: usize) -> Vec<Job> {
        self.jobs.iter().take(maximum).cloned().collect()
    }
    /// 按批次遍历全部历史任务（从最新到最旧）。
    ///
    /// 每次以 `batch_size` 条为一批调用回调 `finish`；回调返回 `true`
    /// 表示提前终止遍历。`batch_size` 至少按 1 处理，避免除零/空批次。
    pub fn iter_batches(&self, batch_size: usize, mut finish: impl FnMut(&[Job]) -> bool) {
        for batch in self.jobs.chunks(batch_size.max(1)) {
            if finish(batch) {
                break;
            }
        }
    }
    /// 返回全部历史任务的克隆副本（按 ID 升序）。
    ///
    /// Go `GetAllHistoryDDLJobs` 与最近任务扫描的顺序不同，会在收集完成后
    /// 显式按任务 ID 升序排序。
    pub fn all(&self) -> Vec<Job> {
        self.jobs.iter().rev().cloned().collect()
    }
    /// 从指定任务 ID 开始向旧方向扫描历史任务，最多返回 `limit` 条。
    ///
    /// - `start_job_id` 为 0 表示从最新任务开始扫描；此时 `limit` 为 0
    ///   使用 Go `DefNumGetDDLHistoryJobs` 对应的默认上限；
    /// - `start_job_id` 非 0 时只返回 ID 小于等于它的任务（即从该任务起往更旧的方向），
    ///   此时必须同时指定非 0 的 `limit`，否则返回错误。
    pub fn scan(&self, start_job_id: i64, limit: usize) -> Result<Vec<Job>, String> {
        // 参数校验：指定起点 ID 时必须配合 limit 使用。
        if start_job_id != 0 && limit == 0 {
            return Err("when 'start_job_id' is specified, it must work with a 'limit'".into());
        }
        let limit = if limit == 0 {
            DEFAULT_SCAN_LIMIT
        } else {
            limit
        };
        // 列表已按 ID 降序，过滤出起点及更旧的任务后截取前 limit 条即可。
        Ok(self
            .jobs
            .iter()
            .filter(|job| start_job_id == 0 || job.id <= start_job_id)
            .take(limit)
            .cloned()
            .collect())
    }
}
