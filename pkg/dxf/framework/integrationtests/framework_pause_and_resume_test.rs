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

use astersql_dxf_framework_handle as handle;
use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_scheduler as scheduler;
use astersql_dxf_framework_storage as storage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

static HANDLE_RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct TaskRecord {
    id: i64,
    state: proto::TaskState,
}

#[derive(Default)]
struct PauseResumeRuntime {
    tasks: Mutex<HashMap<String, TaskRecord>>,
    live_subtasks: Mutex<HashMap<(i64, proto::Step, proto::SubtaskState), i64>>,
    history_subtasks: Mutex<HashMap<i64, i64>>,
    subtask_errors: Mutex<HashMap<i64, Vec<String>>>,
}

impl PauseResumeRuntime {
    fn insert_task(&self, id: i64, key: &str, state: proto::TaskState) {
        self.tasks
            .lock()
            .unwrap()
            .insert(key.into(), TaskRecord { id, state });
    }

    fn task(&self, key: &str) -> proto::Task {
        let record = self.tasks.lock().unwrap()[key];
        proto::Task {
            TaskBase: proto::TaskBase {
                ID: record.id,
                Key: key.into(),
                Type: proto::TaskTypeExample,
                State: record.state,
                Step: proto::StepOne,
                Priority: proto::NormalPriority,
                RequiredSlots: 1,
                TargetScope: String::new(),
                CreateTime: SystemTime::UNIX_EPOCH,
                MaxNodeCount: 0,
                ExtraParams: proto::ExtraParams::default(),
                Keyspace: String::new(),
            },
            SchedulerID: String::new(),
            StartTime: SystemTime::UNIX_EPOCH,
            StateUpdateTime: SystemTime::UNIX_EPOCH,
            Meta: Vec::new(),
            Error: None,
            ModifyParam: proto::ModifyParam {
                PrevState: proto::TaskStatePending,
                Modifications: Vec::new(),
            },
        }
    }

    fn state(&self, key: &str) -> proto::TaskState {
        self.tasks.lock().unwrap()[key].state
    }

    fn finish_resume(&self, key: &str) {
        let id = {
            let mut tasks = self.tasks.lock().unwrap();
            let task = tasks.get_mut(key).unwrap();
            assert_eq!(task.state, proto::TaskStateResuming);
            task.state = proto::TaskStateRunning;
            task.state = proto::TaskStateSucceed;
            task.id
        };
        let mut subtasks = self.live_subtasks.lock().unwrap();
        subtasks.insert((id, proto::StepOne, proto::SubtaskStateSucceed), 3);
        subtasks.insert((id, proto::StepTwo, proto::SubtaskStateSucceed), 1);
    }

    fn check_subtasks_state(&self, task_id: i64, state: proto::SubtaskState, expected: i64) {
        let history = self
            .history_subtasks
            .lock()
            .unwrap()
            .get(&task_id)
            .copied()
            .unwrap_or_default();
        let actual = if history != 0 {
            history
        } else {
            let live = self.live_subtasks.lock().unwrap();
            live.get(&(task_id, proto::StepOne, state))
                .copied()
                .unwrap_or_default()
                + live
                    .get(&(task_id, proto::StepTwo, state))
                    .copied()
                    .unwrap_or_default()
        };
        assert_eq!(actual, expected);
    }
}

impl handle::Runtime for PauseResumeRuntime {
    fn get_cpu_count_of_node(&self, _: &handle::Context) -> handle::Result<i32> {
        Ok(16)
    }
    fn get_task_by_key_with_history(
        &self,
        _: &handle::Context,
        key: &str,
    ) -> handle::Result<Option<proto::Task>> {
        let exists = self.tasks.lock().unwrap().contains_key(key);
        Ok(exists.then(|| self.task(key)))
    }
    fn create_task(
        &self,
        _: &handle::Context,
        _: &str,
        _: proto::TaskType,
        _: &str,
        _: i32,
        _: &str,
        _: i32,
        _: proto::ExtraParams,
        _: Vec<u8>,
    ) -> handle::Result<i64> {
        unreachable!()
    }
    fn get_task_by_id(&self, _: &handle::Context, _: i64) -> handle::Result<proto::Task> {
        unreachable!()
    }
    fn get_task_by_id_with_history(
        &self,
        _: &handle::Context,
        _: i64,
    ) -> handle::Result<proto::Task> {
        unreachable!()
    }
    fn get_task_base_by_id_with_history(
        &self,
        _: &handle::Context,
        _: i64,
    ) -> handle::Result<proto::TaskBase> {
        unreachable!()
    }
    fn get_task_by_key(
        &self,
        _: &handle::Context,
        key: &str,
    ) -> handle::Result<Option<proto::Task>> {
        let exists = self.tasks.lock().unwrap().contains_key(key);
        Ok(exists.then(|| self.task(key)))
    }
    fn cancel_task(&self, _: &handle::Context, _: i64) -> handle::Result<()> {
        unreachable!()
    }
    fn pause_task(&self, _: &handle::Context, key: &str) -> handle::Result<bool> {
        let mut tasks = self.tasks.lock().unwrap();
        let Some(task) = tasks.get_mut(key) else {
            return Ok(false);
        };
        if matches!(
            task.state,
            proto::TaskStatePending | proto::TaskStateRunning
        ) {
            task.state = proto::TaskStatePausing;
            task.state = proto::TaskStatePaused;
            return Ok(true);
        }
        Ok(false)
    }
    fn resume_task(&self, _: &handle::Context, key: &str) -> handle::Result<bool> {
        let mut tasks = self.tasks.lock().unwrap();
        let Some(task) = tasks.get_mut(key) else {
            return Ok(false);
        };
        if task.state == proto::TaskStatePaused {
            task.state = proto::TaskStateResuming;
            return Ok(true);
        }
        Ok(false)
    }
    fn get_task_bases_in_states(
        &self,
        _: &handle::Context,
        _: &[proto::TaskState],
    ) -> handle::Result<Vec<proto::TaskBase>> {
        unreachable!()
    }
    fn get_all_nodes(&self, _: &handle::Context) -> handle::Result<Vec<proto::ManagedNode>> {
        unreachable!()
    }
    fn get_busy_nodes(
        &self,
        _: &handle::Context,
    ) -> handle::Result<Vec<scheduler::schstatus::Node>> {
        unreachable!()
    }
    fn owner_exec_id(&self, _: &handle::Context) -> handle::Result<String> {
        unreachable!()
    }
    fn get_active_task_summary(
        &self,
        _: &handle::Context,
    ) -> handle::Result<storage::ActiveTaskSummary> {
        unreachable!()
    }
    fn list_history_tasks(
        &self,
        _: &handle::Context,
        _: i32,
        _: i64,
        _: &str,
    ) -> handle::Result<storage::HistoryTaskPage> {
        unreachable!()
    }
    fn local_cpu_count(&self) -> i32 {
        16
    }
    fn update_pause_scale_in_flag(
        &self,
        _: &handle::Context,
        _: &scheduler::schstatus::TTLFlag,
    ) -> handle::Result<()> {
        unreachable!()
    }
    fn get_pause_scale_in_flag(
        &self,
        _: &handle::Context,
    ) -> handle::Result<Option<scheduler::schstatus::TTLFlag>> {
        unreachable!()
    }
    fn get_schedule_tune_factors(
        &self,
        _: &handle::Context,
        _: &str,
    ) -> handle::Result<Option<scheduler::schstatus::TTLTuneFactors>> {
        unreachable!()
    }
    fn is_next_gen(&self) -> bool {
        false
    }
    fn service_scope(&self) -> String {
        String::new()
    }
    fn cloud_storage_uri(&self) -> String {
        String::new()
    }
    fn sem_enabled(&self) -> bool {
        false
    }
    fn cluster_id(&self, _: &handle::Context) -> Option<u64> {
        None
    }
    fn new_object_store(
        &self,
        _: &handle::Context,
        _: &str,
        _: Option<Arc<handle::AccessStats>>,
    ) -> handle::Result<Arc<dyn handle::ObjectStorage>> {
        unreachable!()
    }
    fn write_meter_data(
        &self,
        _: &handle::Context,
        _: i64,
        _: &str,
        _: &handle::MeterItem,
    ) -> handle::Result<()> {
        unreachable!()
    }
}

struct RuntimeGuard(Option<Arc<dyn handle::Runtime>>);
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        handle::ClearRuntime();
        if let Some(previous) = self.0.take() {
            handle::InstallRuntime(previous);
        }
    }
}

#[test]
fn test_framework_pause_and_resume() {
    let _lock = HANDLE_RUNTIME_TEST_LOCK.lock().unwrap();
    let runtime = Arc::new(PauseResumeRuntime::default());
    let _guard = RuntimeGuard(handle::InstallRuntime(runtime.clone()));
    let ctx = handle::Context::background();

    for (id, key, initial_state) in [
        (1, "key1", proto::TaskStateRunning),
        (2, "key2", proto::TaskStatePending),
    ] {
        runtime.insert_task(id, key, initial_state);
        handle::PauseTask(&ctx, key).unwrap();
        assert_eq!(runtime.state(key), proto::TaskStatePaused);
        handle::ResumeTask(&ctx, key).unwrap();
        assert_eq!(runtime.state(key), proto::TaskStateResuming);
        runtime.finish_resume(key);
        assert_eq!(runtime.state(key), proto::TaskStateSucceed);
        runtime.check_subtasks_state(id, proto::SubtaskStateSucceed, 4);
        assert!(runtime.subtask_errors.lock().unwrap().get(&id).is_none());
    }

    // CheckSubtasksState uses history after GC has moved every subtask out of the live table.
    runtime.history_subtasks.lock().unwrap().insert(1, 4);
    runtime.live_subtasks.lock().unwrap().clear();
    runtime.check_subtasks_state(1, proto::SubtaskStateSucceed, 4);
}
