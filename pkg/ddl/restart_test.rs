// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// DDL Worker 重启恢复相关单元测试。
//
// 模拟创建（None → Public）与删除
//（Public → WriteOnly → DeleteOnly → None）在 Worker 中途重启时的续跑行为，
// 验证建库/删库、建表/删表可恢复完成，
// 且 schema version 在重启过程中单调递增。

use std::collections::{HashMap, HashSet};

/// Schema 对象公开状态，对齐 TiDB DDL 的 SchemaState 子集。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectState {
    None,
    DeleteOnly,
    WriteOnly,
    Public,
}

/// 简化的 DDL 作业类型：建/删库与建/删表。
#[derive(Clone, Debug)]
enum JobKind {
    CreateSchema(String),
    DropSchema(String),
    CreateTable { schema: String, table: String },
    DropTable { schema: String, table: String },
}

/// 模拟中的 DDL Job：含 ID、类型与当前 schema 状态。
#[derive(Clone, Debug)]
struct Job {
    id: u64,
    kind: JobKind,
    state: ObjectState,
}

/// 模拟 Domain：持有 worker 代数、schema version、库表集合与作业历史。
#[derive(Default)]
struct Domain {
    worker_generation: u64,
    schema_version: u64,
    schemas: HashSet<String>,
    tables: HashMap<String, HashSet<String>>,
    history: HashMap<u64, Result<(), String>>,
}

impl Domain {
    /// 模拟重启 DDL Worker，递增代数。
    fn restart_workers(&mut self) {
        self.worker_generation += 1;
    }

    /// 推进作业一个 schema 状态；创建到达 Public、删除到达 None 时完成。
    fn advance_job(&mut self, job: &mut Job) -> bool {
        let dropping = matches!(job.kind, JobKind::DropSchema(_) | JobKind::DropTable { .. });
        job.state = match (dropping, job.state) {
            (false, ObjectState::None) => ObjectState::Public,
            (true, ObjectState::Public) => ObjectState::WriteOnly,
            (true, ObjectState::WriteOnly) => ObjectState::DeleteOnly,
            (true, ObjectState::DeleteOnly) => ObjectState::None,
            (false, state) => panic!("create job cannot advance from {state:?}"),
            (true, state) => panic!("drop job cannot advance from {state:?}"),
        };
        self.schema_version += 1;
        let completed = if dropping {
            job.state == ObjectState::None
        } else {
            job.state == ObjectState::Public
        };
        if !completed {
            return false;
        }
        // 仅在终态落地元信息变更，与 Go history job 的可见时机一致。
        match &job.kind {
            JobKind::CreateSchema(schema) => {
                self.schemas.insert(schema.clone());
            }
            JobKind::DropSchema(schema) => {
                self.schemas.remove(schema);
                self.tables.remove(schema);
            }
            JobKind::CreateTable { schema, table } => {
                self.tables
                    .entry(schema.clone())
                    .or_default()
                    .insert(table.clone());
            }
            JobKind::DropTable { schema, table } => {
                if let Some(tables) = self.tables.get_mut(schema) {
                    tables.remove(table);
                }
            }
        }
        self.history.insert(job.id, Ok(()));
        true
    }

    /// 在每步推进后重启 Worker，直到作业完成（模拟中断续跑）。
    fn run_interrupted_job(&mut self, job: &mut Job) -> Result<(), String> {
        loop {
            if self.advance_job(job) {
                return self
                    .history
                    .get(&job.id)
                    .cloned()
                    .unwrap_or_else(|| Err("completed job missing from history".to_owned()));
            }
            self.restart_workers();
        }
    }
}

/// 验证建库/删库在多次 Worker 重启后仍能完成。
#[test]
fn test_schema_resume() {
    let mut domain = Domain::default();
    let mut create = Job {
        id: 1,
        kind: JobKind::CreateSchema("test_restart".to_owned()),
        state: ObjectState::None,
    };
    domain.run_interrupted_job(&mut create).unwrap();
    assert!(domain.schemas.contains("test_restart"));

    let mut drop_job = Job {
        id: 2,
        kind: JobKind::DropSchema("test_restart".to_owned()),
        state: ObjectState::Public,
    };
    domain.run_interrupted_job(&mut drop_job).unwrap();
    assert!(!domain.schemas.contains("test_restart"));
    assert_eq!(2, domain.worker_generation);
}

/// 验证 schema version 在状态推进与 Worker 重启过程中始终单调不减。
#[test]
fn test_schema_version_is_monotonic_across_restarts() {
    let mut domain = Domain::default();
    domain.schemas.insert("test_restart".to_owned());
    let mut job = Job {
        id: 3,
        kind: JobKind::DropSchema("test_restart".to_owned()),
        state: ObjectState::Public,
    };
    let mut previous_version = domain.schema_version;
    while !domain.advance_job(&mut job) {
        assert!(domain.schema_version >= previous_version);
        previous_version = domain.schema_version;
        domain.restart_workers();
        assert!(domain.schema_version >= previous_version);
    }
    assert!(domain.schema_version >= previous_version);
    assert_eq!(2, domain.worker_generation);
    assert_eq!(Some(&Ok(())), domain.history.get(&job.id));
}

/// 验证建表/删表在中断续跑后结果正确，且历史记录成功。
#[test]
fn test_table_resume() {
    let mut domain = Domain::default();
    domain.schemas.insert("test_table".to_owned());
    let mut create = Job {
        id: 4,
        kind: JobKind::CreateTable {
            schema: "test_table".to_owned(),
            table: "t1".to_owned(),
        },
        state: ObjectState::None,
    };
    domain.run_interrupted_job(&mut create).unwrap();
    assert!(domain.tables["test_table"].contains("t1"));

    let mut drop_job = Job {
        id: 5,
        kind: JobKind::DropTable {
            schema: "test_table".to_owned(),
            table: "t1".to_owned(),
        },
        state: ObjectState::Public,
    };
    domain.run_interrupted_job(&mut drop_job).unwrap();
    assert!(!domain.tables["test_table"].contains("t1"));
    assert_eq!(Some(&Ok(())), domain.history.get(&drop_job.id));
}

#[test]
fn job_state_transitions_match_go_ddl_direction() {
    let mut domain = Domain::default();

    let mut create = Job {
        id: 6,
        kind: JobKind::CreateSchema("create_state".to_owned()),
        state: ObjectState::None,
    };
    assert!(domain.advance_job(&mut create));
    assert_eq!(ObjectState::Public, create.state);

    domain.schemas.insert("drop_state".to_owned());
    let mut drop_job = Job {
        id: 7,
        kind: JobKind::DropSchema("drop_state".to_owned()),
        state: ObjectState::Public,
    };
    assert!(!domain.advance_job(&mut drop_job));
    assert_eq!(ObjectState::WriteOnly, drop_job.state);
    assert!(!domain.advance_job(&mut drop_job));
    assert_eq!(ObjectState::DeleteOnly, drop_job.state);
    assert!(domain.advance_job(&mut drop_job));
    assert_eq!(ObjectState::None, drop_job.state);
    assert!(!domain.schemas.contains("drop_state"));
    assert_eq!(Some(&Ok(())), domain.history.get(&drop_job.id));
}
