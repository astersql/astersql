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

// `distribute` 模块的单元测试。
//
// 校验 [`DistributionTable`] 保留分区物理 id 映射，以及调度器常量名
// 与 Go 侧 `balance-range-scheduler` 一致。

use crate::distribute::{
    CancelDistributionJobExec, DistributeTableExec, DistributionBackend, DistributionKeyRange,
    DistributionTable, SchedulerJob, schedulerName,
};
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

#[derive(Default)]
struct MockDistributeBackend {
    scheduler_responses: VecDeque<Result<Vec<SchedulerJob>, String>>,
    created: Vec<(String, BTreeMap<String, String>)>,
    waits: Vec<Duration>,
    cancelled: Vec<u64>,
    cancel_error: Option<String>,
}

impl DistributionBackend for MockDistributeBackend {
    type Context = ();
    type Error = String;

    fn key_range(&self, first_physical_id: i64, last_physical_id: i64) -> DistributionKeyRange {
        DistributionKeyRange {
            start_key: format!("table-{first_physical_id}"),
            end_key: format!("table-{}", last_physical_id + 1),
        }
    }

    fn missing_partition(&self, name: &str) -> Self::Error {
        format!("unknown partition {name}")
    }

    fn create_scheduler_config(
        &mut self,
        _ctx: &mut Self::Context,
        scheduler: &str,
        input: BTreeMap<String, String>,
    ) -> Result<(), Self::Error> {
        self.created.push((scheduler.to_owned(), input));
        Ok(())
    }

    fn scheduler_jobs(
        &mut self,
        _ctx: &mut Self::Context,
        _scheduler: &str,
    ) -> Result<Vec<SchedulerJob>, Self::Error> {
        self.scheduler_responses
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn wait_or_cancel(
        &mut self,
        _ctx: &mut Self::Context,
        duration: Duration,
    ) -> Result<(), Self::Error> {
        self.waits.push(duration);
        Ok(())
    }

    fn cancel_scheduler_job(
        &mut self,
        _ctx: &mut Self::Context,
        scheduler: &str,
        job_id: u64,
    ) -> Result<(), Self::Error> {
        assert_eq!(scheduler, schedulerName);
        self.cancelled.push(job_id);
        match &self.cancel_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

fn table() -> DistributionTable {
    DistributionTable {
        database_name: "test".into(),
        table_name: "orders".into(),
        table_id: 9,
        partitions: BTreeMap::from([("p0".into(), 10), ("p1".into(), 11), ("p2".into(), 13)]),
    }
}

fn executor(partition_names: Vec<&str>) -> DistributeTableExec<MockDistributeBackend> {
    DistributeTableExec {
        backend: MockDistributeBackend::default(),
        table: table(),
        partition_names: partition_names.into_iter().map(str::to_owned).collect(),
        rule: "leader-scatter".into(),
        engine: "tikv".into(),
        timeout: "30m".into(),
        done: false,
        key_ranges: Vec::new(),
    }
}

/// 分区元数据与调度器名称的基础不变量。
#[test]
fn distribution_table_metadata_preserves_partition_ids_and_scheduler_name() {
    let table = table();
    assert_eq!(table.partitions["p1"], 11);
    assert_eq!(schedulerName, "balance-range-scheduler");
}

#[test]
fn key_ranges_merge_contiguous_ids_and_validate_partition_names() {
    let mut all_partitions = executor(Vec::new());
    all_partitions.Open(()).unwrap();
    assert_eq!(
        all_partitions.key_ranges,
        vec![
            DistributionKeyRange {
                start_key: "table-10".into(),
                end_key: "table-12".into(),
            },
            DistributionKeyRange {
                start_key: "table-13".into(),
                end_key: "table-14".into(),
            },
        ]
    );

    let mut selected = executor(vec!["P2", "p1"]);
    selected.Open(()).unwrap();
    assert_eq!(selected.partition_names, vec!["p1", "P2"]);
    assert_eq!(selected.getAlias(), "test.orders.partition(p1,P2)");
    assert_eq!(selected.key_ranges.len(), 2);

    let missing = executor(vec!["absent"]);
    assert_eq!(
        missing.getKeyRanges().unwrap_err(),
        "unknown partition absent"
    );

    let mut plain = executor(Vec::new());
    plain.table.partitions.clear();
    assert_eq!(
        plain.getKeyRanges().unwrap(),
        vec![DistributionKeyRange {
            start_key: "table-9".into(),
            end_key: "table-10".into(),
        }]
    );
}

#[test]
fn distribute_retries_scheduler_visibility_and_emits_largest_active_job() {
    let mut exec = executor(vec!["p2", "p0"]);
    exec.backend
        .scheduler_responses
        .push_back(Err("temporary decode error".into()));
    exec.backend.scheduler_responses.push_back(Ok(vec![
        SchedulerJob {
            alias: "test.orders.partition(p0,p2)".into(),
            engine: "tikv".into(),
            rule: "leader-scatter".into(),
            status: "finished".into(),
            job_id: 99.0,
        },
        SchedulerJob {
            alias: "another.table.".into(),
            engine: "tikv".into(),
            rule: "leader-scatter".into(),
            status: "pending".into(),
            job_id: 100.0,
        },
    ]));
    exec.backend.scheduler_responses.push_back(Ok(vec![
        SchedulerJob {
            alias: "test.orders.partition(p0,p2)".into(),
            engine: "tikv".into(),
            rule: "leader-scatter".into(),
            status: "pending".into(),
            job_id: 4.0,
        },
        SchedulerJob {
            alias: "test.orders.partition(p0,p2)".into(),
            engine: "tikv".into(),
            rule: "leader-scatter".into(),
            status: "running".into(),
            job_id: 5.0,
        },
    ]));

    exec.Open(()).unwrap();
    let mut output = astersql_util_chunk::NewChunkWithCapacity(
        vec![*astersql_expression::types::NewFieldType(
            astersql_parser_mysql::r#type::TypeLonglong,
        )],
        1,
    );
    exec.Next(&mut (), output.as_mut()).unwrap();

    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetUint64(0), 5);
    assert_eq!(exec.backend.waits, vec![Duration::from_millis(500); 2]);
    assert_eq!(exec.backend.created.len(), 1);
    let (scheduler, input) = &exec.backend.created[0];
    assert_eq!(scheduler, schedulerName);
    assert_eq!(input["alias"], "test.orders.partition(p0,p2)");
    assert_eq!(input["engine"], "tikv");
    assert_eq!(input["rule"], "leader-scatter");
    assert_eq!(input["timeout"], "30m");
    assert_eq!(input["start-key"], "table-10,table-13");
    assert_eq!(input["end-key"], "table-11,table-14");

    exec.Next(&mut (), output.as_mut()).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert_eq!(exec.backend.created.len(), 1);
}

#[test]
fn cancel_distribution_job_propagates_errors_and_runs_once_after_success() {
    let mut failed = CancelDistributionJobExec {
        backend: MockDistributeBackend {
            cancel_error: Some("job not found".into()),
            ..Default::default()
        },
        job_id: 1,
        done: false,
    };
    assert_eq!(failed.Next(&mut ()).unwrap_err(), "job not found");
    assert_eq!(failed.backend.cancelled, vec![1]);

    let mut succeeded = CancelDistributionJobExec {
        backend: MockDistributeBackend::default(),
        job_id: 7,
        done: false,
    };
    succeeded.Next(&mut ()).unwrap();
    succeeded.Next(&mut ()).unwrap();
    assert_eq!(succeeded.backend.cancelled, vec![7]);
}
