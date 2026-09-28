// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 表数据分布（distribute table）执行器。
//
// 对应 `DISTRIBUTE TABLE` / 取消分布任务相关语句：把表（或指定分区）的
// key range（键范围）提交给 PD（Placement Driver）侧的
// `balance-range-scheduler`，由调度器按 rule/engine 在存储引擎间再平衡。
//
// 主要内容：
// - [`DistributeTableExec`]：一次性提交调度配置并回写 job id；
// - [`CancelDistributionJobExec`]：按 job id 取消未完成的调度任务；
// - [`DistributionBackend`]：与 PD/调度微服务交互的抽象边界。

#![allow(non_snake_case)]

use std::collections::BTreeMap;
use std::time::Duration;

use astersql_util_chunk::Chunk;

/// PD 侧用于范围再平衡的调度器名称，与 Go 常量保持一致。
pub const schedulerName: &str = "balance-range-scheduler";

/// 待分布表的元数据：库名、表名、逻辑表 id，以及分区名到物理表 id 的映射。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionTable {
    pub database_name: String,
    pub table_name: String,
    pub table_id: i64,
    /// 分区名 → 物理表 id（Region 编码时使用物理 id）。
    pub partitions: BTreeMap<String, i64>,
}

/// 编码后的键区间，提交给调度器时作为 start-key / end-key。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionKeyRange {
    pub start_key: String,
    pub end_key: String,
}

/// 从调度微服务查询到的一条调度作业快照。
#[derive(Clone, Debug, PartialEq)]
pub struct SchedulerJob {
    pub alias: String,
    pub engine: String,
    pub rule: String,
    pub status: String,
    pub job_id: f64,
}

/// 分布执行器对外部系统的依赖：编码 key range、创建/查询/取消调度配置。
pub trait DistributionBackend {
    type Context;
    type Error;

    /// 将连续物理表 id 区间编码为调度器用的键范围。
    fn key_range(&self, first_physical_id: i64, last_physical_id: i64) -> DistributionKeyRange;
    /// 指定分区名不存在时返回错误。
    fn missing_partition(&self, name: &str) -> Self::Error;
    /// 向 PD 创建/更新 scheduler 配置（触发分布作业）。
    fn create_scheduler_config(
        &mut self,
        ctx: &mut Self::Context,
        scheduler: &str,
        input: BTreeMap<String, String>,
    ) -> Result<(), Self::Error>;
    /// 列出指定调度器当前作业。
    fn scheduler_jobs(
        &mut self,
        ctx: &mut Self::Context,
        scheduler: &str,
    ) -> Result<Vec<SchedulerJob>, Self::Error>;
    /// 等待一段时间，或在会话取消时提前返回。
    fn wait_or_cancel(
        &mut self,
        ctx: &mut Self::Context,
        duration: Duration,
    ) -> Result<(), Self::Error>;
    /// 按 job id 取消调度作业。
    fn cancel_scheduler_job(
        &mut self,
        ctx: &mut Self::Context,
        scheduler: &str,
        job_id: u64,
    ) -> Result<(), Self::Error>;
}

/// `DISTRIBUTE TABLE` 执行器：Open 时算好 key ranges，Next 时提交并回写 job id。
pub struct DistributeTableExec<B: DistributionBackend> {
    pub backend: B,
    pub table: DistributionTable,
    /// 用户指定要分布的分区名；空表示整表（含全部分区）。
    pub partition_names: Vec<String>,
    pub rule: String,
    pub engine: String,
    pub timeout: String,
    /// 本执行器只产出一行，done 防止重复提交。
    pub done: bool,
    pub key_ranges: Vec<DistributionKeyRange>,
}

impl<B: DistributionBackend> DistributeTableExec<B> {
    /// 打开执行器：计算 key ranges，并对分区名做大小写不敏感排序（稳定 alias）。
    pub fn Open<C>(&mut self, _ctx: C) -> Result<(), B::Error> {
        self.key_ranges = self.getKeyRanges()?;
        self.partition_names.sort_by_key(|name| name.to_lowercase());
        Ok(())
    }

    /// 提交分布配置；最多重试 3 次查询 job id（调度微服务可能滞后于 PD 配置更新）。
    pub fn Next(&mut self, ctx: &mut B::Context, chunk: &mut Chunk) -> Result<(), B::Error> {
        chunk.Reset();
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.distributeTable(ctx)?;

        // 配置刚写入 PD 时，微服务侧列表可能尚未可见，短暂等待后重试。
        let mut job_id = -1.0;
        for attempt in 0..3 {
            let (found, id) = self.getSchedulerJob(ctx);
            if found {
                job_id = id;
                break;
            }
            if attempt < 2 {
                self.backend
                    .wait_or_cancel(ctx, Duration::from_millis(500))?;
            }
        }
        if job_id != -1.0 {
            chunk.AppendUint64(0, job_id as u64);
        }
        Ok(())
    }

    /// 按 alias/engine/rule 匹配未完成作业，取最大 job_id。
    pub fn getSchedulerJob(&mut self, ctx: &mut B::Context) -> (bool, f64) {
        // Scheduler microservices may lag PD's config update. Fetch/decoding
        // failures are intentionally retried by `Next`, matching Go.
        // 拉取/解码失败时返回未找到，由 Next 负责重试，与 Go 行为一致。
        let Ok(jobs) = self.backend.scheduler_jobs(ctx, schedulerName) else {
            return (false, -1.0);
        };
        let alias = self.getAlias();
        let job_id = jobs
            .into_iter()
            .filter(|job| {
                job.alias == alias
                    && job.engine == self.engine
                    && job.rule == self.rule
                    && job.status != "finished"
            })
            .map(|job| job.job_id)
            .fold(-1.0_f64, f64::max);
        (job_id > -1.0, job_id)
    }

    /// 组装 alias/engine/rule/timeout 与起止键，写入 balance-range-scheduler 配置。
    pub fn distributeTable(&mut self, ctx: &mut B::Context) -> Result<(), B::Error> {
        let mut input = BTreeMap::new();
        input.insert("alias".to_owned(), self.getAlias());
        input.insert("engine".to_owned(), self.engine.clone());
        input.insert("rule".to_owned(), self.rule.clone());
        if !self.timeout.is_empty() {
            input.insert("timeout".to_owned(), self.timeout.clone());
        }
        // 多段 key range 以逗号拼接，与 Go 侧调度器入参格式一致。
        input.insert(
            "start-key".to_owned(),
            self.key_ranges
                .iter()
                .map(|range| range.start_key.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        input.insert(
            "end-key".to_owned(),
            self.key_ranges
                .iter()
                .map(|range| range.end_key.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        self.backend
            .create_scheduler_config(ctx, schedulerName, input)
    }

    /// 作业别名：`库.表.` 或 `库.表.partition(p0,p1,...)`。
    pub fn getAlias(&self) -> String {
        let partition = if self.partition_names.is_empty() {
            String::new()
        } else {
            format!("partition({})", self.partition_names.join(","))
        };
        format!(
            "{}.{}.{}",
            self.table.database_name, self.table.table_name, partition
        )
    }

    /// 根据分区选择物理表 id，合并连续 id 为更少的 key range。
    pub fn getKeyRanges(&self) -> Result<Vec<DistributionKeyRange>, B::Error> {
        // 无分区 → 逻辑表 id；有分区但未指定名 → 全部分区；否则按名解析。
        let mut physical_ids = if self.table.partitions.is_empty() {
            vec![self.table.table_id]
        } else if self.partition_names.is_empty() {
            self.table.partitions.values().copied().collect()
        } else {
            let mut ids = Vec::with_capacity(self.partition_names.len());
            for name in &self.partition_names {
                let lower = name.to_lowercase();
                let Some((_, id)) = self
                    .table
                    .partitions
                    .iter()
                    .find(|(partition, _)| partition.to_lowercase() == lower)
                else {
                    return Err(self.backend.missing_partition(name));
                };
                ids.push(*id);
            }
            ids
        };
        physical_ids.sort_unstable();

        // 将连续物理 id 合并为一段 range，减少调度器入参段数。
        let mut ranges = Vec::new();
        let mut index = 0;
        while index < physical_ids.len() {
            let first = physical_ids[index];
            let mut last = first;
            while index + 1 < physical_ids.len() && physical_ids[index + 1] == last + 1 {
                index += 1;
                last = physical_ids[index];
            }
            ranges.push(self.backend.key_range(first, last));
            index += 1;
        }
        Ok(ranges)
    }
}

/// 取消指定分布作业的执行器，Next 只执行一次取消调用。
pub struct CancelDistributionJobExec<B: DistributionBackend> {
    pub backend: B,
    pub job_id: u64,
    pub done: bool,
}

impl<B: DistributionBackend> CancelDistributionJobExec<B> {
    /// 向 balance-range-scheduler 发送取消请求。
    pub fn Next(&mut self, ctx: &mut B::Context) -> Result<(), B::Error> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.backend
            .cancel_scheduler_job(ctx, schedulerName, self.job_id)
    }
}
