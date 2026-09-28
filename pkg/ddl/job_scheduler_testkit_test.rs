// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use crate::ddl::{Job, JobState};
use crate::ddl_running_jobs::InvolvingSchemaInfo;
use crate::job_scheduler::{JobScheduler, JobType, ScheduledJob};
use crate::job_worker::{JobWorker, WorkerType};
use crate::schema_version::SchemaAction;

fn scheduler() -> JobScheduler {
    JobScheduler::new(
        JobWorker::new(WorkerType::General),
        JobWorker::new(WorkerType::AddIndex),
    )
}

fn job(id: i64, query: &str, involves: Vec<InvolvingSchemaInfo>) -> ScheduledJob {
    ScheduledJob {
        job: Job::new(id, 1, id, query),
        involves,
        job_type: JobType::General,
        action: SchemaAction::Other,
    }
}

/// Go `check` 的直接等价物：同一冲突组中的任意两个 job 不能交叉出现。
fn assert_not_interleaved(record: &[i64], ids: &[i64]) {
    fn first_is_before(record: &[i64], left: i64, right: i64) -> bool {
        record
            .iter()
            .find_map(|id| {
                (*id == left)
                    .then_some(true)
                    .or((*id == right).then_some(false))
            })
            .unwrap_or_else(|| panic!("neither {left} nor {right} occurs in {record:?}"))
    }

    fn assert_all_before(record: &[i64], first: i64, second: i64) {
        let mut met_second = false;
        for id in record {
            if *id == second {
                met_second = true;
            }
            assert!(
                !(met_second && *id == first),
                "job {first} crosses job {second} in {record:?}"
            );
        }
    }

    for (position, left) in ids.iter().enumerate() {
        for right in &ids[position + 1..] {
            if first_is_before(record, *left, *right) {
                assert_all_before(record, *left, *right);
            } else {
                assert_all_before(record, *right, *left);
            }
        }
    }
}

#[test]
fn ddl_scheduling_preserves_go_conflict_groups() {
    // Go TestDDLScheduling 归一化后的交付序列：独立 job 可交错，冲突组必须连续。
    let record = [
        0, 3, 7, 8, 0, 3, 7, 8, 0, 3, 7, 8, 1, 1, 1, 2, 2, 2, 4, 4, 4, 5, 5, 5, 6, 6, 6, 9, 9, 9,
    ];
    for ids in [
        &[0, 1, 2][..],
        &[0, 4],
        &[1, 4],
        &[2, 4],
        &[4, 5],
        &[4, 6],
        &[4, 9],
        &[5, 6],
        &[5, 9],
        &[6, 9],
        &[8, 9],
    ] {
        assert_not_interleaved(&record, ids);
    }

    let mut scheduler = scheduler();
    let definitions = [
        (
            0,
            "alter table e2 add index idx(id)",
            vec![InvolvingSchemaInfo::schema("test", "e2")],
        ),
        (
            1,
            "alter table e2 add index idx1(id)",
            vec![InvolvingSchemaInfo::schema("test", "e2")],
        ),
        (
            2,
            "alter table e2 add index idx2(id)",
            vec![InvolvingSchemaInfo::schema("test", "e2")],
        ),
        (
            3,
            "create table e5 (id int)",
            vec![InvolvingSchemaInfo::schema("test", "e5")],
        ),
        (
            4,
            "alter table e exchange partition p1 with table e2",
            vec![
                InvolvingSchemaInfo::schema("test", "e"),
                InvolvingSchemaInfo::schema("test", "e2"),
            ],
        ),
        (
            5,
            "alter table e add index idx(id)",
            vec![InvolvingSchemaInfo::schema("test", "e")],
        ),
        (
            6,
            "alter table e add partition p3",
            vec![InvolvingSchemaInfo::schema("test", "e")],
        ),
        (
            7,
            "create table e4 (id int)",
            vec![InvolvingSchemaInfo::schema("test", "e4")],
        ),
        (
            8,
            "alter table e3 add index idx1(id)",
            vec![InvolvingSchemaInfo::schema("test", "e3")],
        ),
        (
            9,
            "alter table e exchange partition p1 with table e3",
            vec![
                InvolvingSchemaInfo::schema("test", "e"),
                InvolvingSchemaInfo::schema("test", "e3"),
            ],
        ),
    ];
    for (id, query, involves) in definitions {
        scheduler.enqueue(job(id, query, involves));
    }

    assert_eq!(0, scheduler.schedule().unwrap());
    scheduler.on_become_owner();
    assert_eq!(1, scheduler.reload_schema_count);
    assert_eq!(4, scheduler.schedule().unwrap());
    while scheduler.schedule().unwrap() != 0 {}
    assert!(!scheduler.worker_pool_exhausted());
}

#[test]
fn upgrading_related_job_states_keep_go_transitions() {
    let cases = [
        (JobState::Done, 1),
        (JobState::Cancelling, 1),
        (JobState::Running, 2),
    ];

    for (state, rounds_to_finish) in cases {
        let mut scheduler = scheduler();
        scheduler.on_become_owner();
        let mut scheduled = job(
            1,
            "alter table e2 add index idx(id)",
            vec![InvolvingSchemaInfo::schema("test", "e2")],
        );
        scheduled.job.state = state;
        scheduler.enqueue(scheduled);

        for _ in 0..rounds_to_finish {
            assert_eq!(1, scheduler.schedule().unwrap());
        }
        assert_eq!(0, scheduler.schedule().unwrap());
    }
}

#[test]
fn general_ddl_with_query_is_not_mdl_blocked() {
    let mut scheduler = scheduler();
    scheduler.on_become_owner();
    scheduler.enqueue(job(
        1,
        "alter table t add column b int",
        vec![InvolvingSchemaInfo::schema("test", "t")],
    ));
    // Go 的 CREATE VIEW 查询不会填充 MDL 相关的 involving tables。
    scheduler.enqueue(job(2, "create view v as select * from t", Vec::new()));

    assert_eq!(2, scheduler.schedule().unwrap());
}
