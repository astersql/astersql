// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use crate::executor::{
    ColumnInfo, DdlAction, DdlJob, Executor, Ident, JobBackend, JobState, ObjectState, OnExist,
    SessionContext, SubmitResult, TableInfo,
};

const ERROR_BEFORE_DECODE_ARGS: &str = "error before decoding DDL job arguments";

#[derive(Debug, Default, PartialEq, Eq)]
struct FailpointProbe {
    add_column_states: Vec<ObjectState>,
    decode_attempts: usize,
    injected_errors: usize,
}

/// Test-only backend isolating the two failpoint boundaries used by the Go test.
#[derive(Default)]
struct DecodeFailBackend {
    next_job_id: i64,
    current: BTreeMap<i64, DdlJob>,
    history: BTreeMap<i64, DdlJob>,
    probe: Rc<RefCell<FailpointProbe>>,
}

impl DecodeFailBackend {
    fn finish(&mut self, job: &DdlJob) {
        let mut finished = job.clone();
        finished.state = JobState::Synced;
        finished.schema_state = ObjectState::Public;
        finished.schema_version = finished.id as u64;
        self.current.remove(&finished.id);
        self.history.insert(finished.id, finished);
    }

    fn before_decode(&self) -> Result<(), &'static str> {
        let mut probe = self.probe.borrow_mut();
        probe.decode_attempts += 1;
        if probe.injected_errors == 0 {
            probe.injected_errors += 1;
            return Err(ERROR_BEFORE_DECODE_ARGS);
        }
        Ok(())
    }
}

impl JobBackend for DecodeFailBackend {
    fn submit(&mut self, job: &mut DdlJob) -> Result<SubmitResult, String> {
        self.next_job_id += 1;
        job.id = self.next_job_id;
        job.state = JobState::Running;
        self.current.insert(job.id, job.clone());

        if job.action == DdlAction::AddColumn {
            job.schema_state = ObjectState::WriteOnly;
            self.probe
                .borrow_mut()
                .add_column_states
                .push(ObjectState::WriteOnly);
            job.schema_state = ObjectState::WriteReorganization;
            self.probe
                .borrow_mut()
                .add_column_states
                .push(ObjectState::WriteReorganization);
            let _injected = self.before_decode().expect_err("first decode must fail");

            // The worker retries the same job step after the injected error. It must resume from
            // WriteReorganization instead of replaying the already completed WriteOnly state.
            self.probe
                .borrow_mut()
                .add_column_states
                .push(ObjectState::WriteReorganization);
            self.before_decode()
                .expect("retry must decode DDL job arguments");
            self.probe
                .borrow_mut()
                .add_column_states
                .push(ObjectState::Public);
        }

        self.finish(job);
        Ok(SubmitResult {
            job_id: job.id,
            merged: false,
        })
    }

    fn history_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String> {
        Ok(self.history.get(&job_id).cloned())
    }

    fn current_job(&mut self, job_id: i64) -> Result<Option<DdlJob>, String> {
        Ok(self.current.get(&job_id).cloned())
    }

    fn cancel(&mut self, job_id: i64) -> Result<(), crate::executor::CancelJobError> {
        if let Some(mut job) = self.current.remove(&job_id) {
            job.state = JobState::Cancelled;
            self.history.insert(job_id, job);
        }
        Ok(())
    }
}

fn table(name: &str) -> TableInfo {
    TableInfo {
        id: 0,
        schema_id: 0,
        name: name.into(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        columns: vec![ColumnInfo::integer("c1"), ColumnInfo::integer("c2")],
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        partitions: Vec::new(),
        auto_increment: 0,
        auto_random_bits: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        temporary: false,
        view: false,
        sequence: false,
        cached: false,
        tiflash_replica_count: 0,
        tiflash_available_ids: BTreeSet::new(),
        placement_policy: None,
        affinity: None,
        ttl_column: None,
        table_lock: None,
    }
}

/// Go `TestFailBeforeDecodeArgs`: the retry resumes at WriteReorganization, does not repeat
/// WriteOnly, publishes `c3`, and records a successful history job.
#[test]
fn test_fail_before_decode_args() {
    let backend = DecodeFailBackend::default();
    let probe = Rc::clone(&backend.probe);
    let mut ddl = Executor::new(backend, Duration::ZERO);
    let mut session = SessionContext::default();

    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let table_id = ddl
        .create_table(&mut session, "test", table("t1"), OnExist::Error)
        .unwrap();
    ddl.add_column(
        &mut session,
        &Ident::new("test", "t1"),
        ColumnInfo::integer("c3"),
        None,
        false,
    )
    .unwrap();

    let probe = probe.borrow();
    assert_eq!(
        vec![
            ObjectState::WriteOnly,
            ObjectState::WriteReorganization,
            ObjectState::WriteReorganization,
            ObjectState::Public,
        ],
        probe.add_column_states
    );
    assert_eq!(2, probe.decode_attempts);
    assert_eq!(1, probe.injected_errors);

    let schema = ddl.schemas.get("test").unwrap();
    let table = schema.tables.get("t1").unwrap();
    assert_eq!(table_id, table.id);
    assert!(table.columns.iter().any(|column| column.name == "c3"));
    let add_column_job = ddl
        .backend()
        .history
        .values()
        .find(|job| job.action == DdlAction::AddColumn)
        .unwrap();
    assert_eq!(JobState::Synced, add_column_job.state);
    assert_eq!(ObjectState::Public, add_column_job.schema_state);
    assert_eq!(None, add_column_job.error);
}
