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

//! 中文说明开始（自动生成）
//! 中文总览：`main_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `main_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 71 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `GCS_ENDPOINT` 是当前文件里的常量。
//! 阅读 `GCS_ENDPOINT` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `GCS_ENDPOINT` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Step` 是当前文件里的分支类型。
//! 阅读 `Step` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Step` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `TaskState` 是当前文件里的分支类型。
//! 阅读 `TaskState` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `TaskState` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Summary` 是当前文件里的状态类型。
//! 阅读 `Summary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Summary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `add` 是当前文件里的辅助函数。
//! 阅读 `add` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `add` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Subtask` 是当前文件里的状态类型。
//! 阅读 `Subtask` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Subtask` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Metering` 是当前文件里的状态类型。
//! 阅读 `Metering` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Metering` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Task` 是当前文件里的状态类型。
//! 阅读 `Task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `step_summary` 是当前文件里的辅助函数。
//! 阅读 `step_summary` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `step_summary` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `FakeGcs` 是当前文件里的状态类型。
//! 阅读 `FakeGcs` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `FakeGcs` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `create_object` 是当前文件里的辅助函数。
//! 阅读 `create_object` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `create_object` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `get_object` 是当前文件里的辅助函数。
//! 阅读 `get_object` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `get_object` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `list_prefix` 是当前文件里的辅助函数。
//! 阅读 `list_prefix` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `list_prefix` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `remove_prefix` 是当前文件里的辅助函数。
//! 阅读 `remove_prefix` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `remove_prefix` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `requests` 是当前文件里的辅助函数。
//! 阅读 `requests` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `requests` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `Table` 是当前文件里的状态类型。
//! 阅读 `Table` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Table` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `MockGcsSuite` 是当前文件里的状态类型。
//! 阅读 `MockGcsSuite` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `MockGcsSuite` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `setup` 是当前文件里的辅助函数。
//! 阅读 `setup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `setup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `tear_down` 是当前文件里的辅助函数。
//! 阅读 `tear_down` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `tear_down` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `prepare_and_use_db` 是当前文件里的辅助函数。
//! 阅读 `prepare_and_use_db` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `prepare_and_use_db` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `create_table` 是当前文件里的辅助函数。
//! 阅读 `create_table` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `create_table` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `table` 是当前文件里的辅助函数。
//! 阅读 `table` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `table` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `table_mut` 是当前文件里的辅助函数。
//! 阅读 `table_mut` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `table_mut` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `cleanup_sys_tables` 是当前文件里的辅助函数。
//! 阅读 `cleanup_sys_tables` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `cleanup_sys_tables` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `create_task` 是当前文件里的辅助函数。
//! 阅读 `create_task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `create_task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `task` 是当前文件里的辅助函数。
//! 阅读 `task` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `task` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `task_mut` 是当前文件里的辅助函数。
//! 阅读 `task_mut` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `task_mut` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `complete_cleanup` 是当前文件里的辅助函数。
//! 阅读 `complete_cleanup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `complete_cleanup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `rows` 是当前文件里的辅助函数。
//! 阅读 `rows` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `rows` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `parse_csv_line` 是当前文件里的辅助函数。
//! 阅读 `parse_csv_line` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `parse_csv_line` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `normalize_cell` 是当前文件里的辅助函数。
//! 阅读 `normalize_cell` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `normalize_cell` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `mvi_values` 是当前文件里的辅助函数。
//! 阅读 `mvi_values` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `mvi_values` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `UniqueGroup` 是当前文件里的分支类型。
//! 阅读 `UniqueGroup` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `UniqueGroup` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `rows_conflict` 是当前文件里的辅助函数。
//! 阅读 `rows_conflict` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `rows_conflict` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `capture_conflicts` 是当前文件里的辅助函数。
//! 阅读 `capture_conflicts` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `capture_conflicts` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `sorted_strings` 是当前文件里的辅助函数。
//! 阅读 `sorted_strings` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `sorted_strings` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `serial_guard` 是当前文件里的辅助函数。
//! 阅读 `serial_guard` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `serial_guard` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_import_into_suite_lifecycle_and_helpers` 是当前文件里的辅助函数。
//! 阅读 `test_import_into_suite_lifecycle_and_helpers` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_into_suite_lifecycle_and_helpers` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 符号 `test_main_preserves_real_tikv_config_and_execution_contract` 是当前文件里的辅助函数。
//! 阅读 `test_main_preserves_real_tikv_config_and_execution_contract` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_main_preserves_real_tikv_config_and_execution_contract` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Executable in-process RealTiKV/import harness used by this package's tests.
//! The object-store boundary is the same fake boundary as the Go tests. SQL,
//! import jobs, task history, subtasks and cleanup are modeled as real mutable
//! state, so failure, retry and cleanup assertions observe side effects.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, OnceLock};

use astersql_tests_realtikvtest::{
    RunTestMain, UpdateTiDBConfig, WithRealTiKV,
    stubs::{TestMain, config, reset_test_globals},
};

pub const GCS_ENDPOINT: &str = "http://127.0.0.1:4443/storage/v1/";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Step {
    EncodeAndSort,
    MergeSort,
    CollectConflicts,
    ConflictResolution,
    WriteAndIngest,
    PostProcess,
    Import,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Running,
    AwaitingResolution,
    Reverting,
    Reverted,
    Succeed,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    pub rows: u64,
    pub processed: u64,
    pub gets: u64,
    pub puts: u64,
}

impl Summary {
    pub fn add(self, rhs: Self) -> Self {
        Self {
            rows: self.rows + rhs.rows,
            processed: self.processed + rhs.processed,
            gets: self.gets + rhs.gets,
            puts: self.puts + rhs.puts,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subtask {
    pub step: Step,
    pub state: TaskState,
    pub summary: Summary,
    pub external_path: Option<String>,
    pub conflict_count: u64,
    pub recorded_conflict_count: u64,
    pub kv_group: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metering {
    pub task_id: i64,
    pub request_gets: u64,
    pub request_puts: u64,
    pub object_read: u64,
    pub object_write: u64,
    pub cluster_read: u64,
    pub cluster_write: u64,
    pub row_count: i64,
    pub data_kv_bytes: i64,
    pub index_kv_bytes: i64,
    pub required_slots: i32,
    pub max_node_count: i32,
    pub duration_seconds: i64,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: i64,
    pub job_id: i64,
    pub state: TaskState,
    pub required_slots: i32,
    pub max_node_count: i32,
    pub cloud_uri: String,
    pub source_rows: usize,
    pub result_rows: usize,
    pub conflict_files: Vec<String>,
    pub external_meta_cleaned: bool,
    pub subtasks: Vec<Subtask>,
    pub metering: Option<Metering>,
}

impl Task {
    pub fn step_summary(&self, step: Step) -> Summary {
        self.subtasks
            .iter()
            .filter(|subtask| subtask.step == step)
            .fold(Summary::default(), |acc, subtask| acc.add(subtask.summary))
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeGcs {
    objects: BTreeMap<(String, String), Vec<u8>>,
    pub stopped: bool,
    get_requests: u64,
    put_requests: u64,
}

impl FakeGcs {
    pub fn create_object(&mut self, bucket: &str, name: &str, content: impl Into<Vec<u8>>) {
        assert!(!self.stopped, "fake GCS must be running");
        self.put_requests += 1;
        self.objects
            .insert((bucket.to_owned(), name.to_owned()), content.into());
    }

    pub fn get_object(&mut self, bucket: &str, name: &str) -> Result<Vec<u8>, String> {
        assert!(!self.stopped, "fake GCS must be running");
        self.get_requests += 1;
        self.objects
            .get(&(bucket.to_owned(), name.to_owned()))
            .cloned()
            .ok_or_else(|| format!("object not found: gs://{bucket}/{name}"))
    }

    pub fn list_prefix(&self, bucket: &str, prefix: &str) -> Vec<String> {
        self.objects
            .keys()
            .filter(|(b, name)| b == bucket && name.starts_with(prefix))
            .map(|(_, name)| name.clone())
            .collect()
    }

    pub fn remove_prefix(&mut self, bucket: &str, prefix: &str) {
        self.objects
            .retain(|(b, name), _| b != bucket || !name.starts_with(prefix));
    }

    pub fn requests(&self) -> (u64, u64) {
        (self.get_requests, self.put_requests)
    }
}

#[derive(Clone, Debug)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    pub import_mode: bool,
}

#[derive(Clone, Debug)]
pub struct MockGcsSuite {
    pub server: FakeGcs,
    pub active_db: String,
    pub tables: HashMap<String, Table>,
    pub tasks: Vec<Task>,
    pub system_rows: [usize; 3],
    next_id: i64,
}

impl MockGcsSuite {
    pub fn setup() -> Self {
        Self {
            server: FakeGcs::default(),
            active_db: String::new(),
            tables: HashMap::new(),
            tasks: Vec::new(),
            system_rows: [0; 3],
            next_id: 1,
        }
    }

    pub fn tear_down(&mut self) {
        self.server.stopped = true;
    }

    pub fn prepare_and_use_db(&mut self, db: &str) {
        self.tables
            .retain(|name, _| !name.starts_with(&format!("{db}.")));
        self.active_db = db.to_owned();
    }

    pub fn create_table(&mut self, name: &str) {
        self.tables.insert(
            format!("{}.{}", self.active_db, name),
            Table {
                rows: Vec::new(),
                import_mode: false,
            },
        );
    }

    pub fn table(&self, name: &str) -> &Table {
        self.tables
            .get(&format!("{}.{}", self.active_db, name))
            .expect("table must exist")
    }

    pub fn table_mut(&mut self, name: &str) -> &mut Table {
        self.tables
            .get_mut(&format!("{}.{}", self.active_db, name))
            .expect("table must exist")
    }

    pub fn cleanup_sys_tables(&mut self) {
        self.system_rows = [0; 3];
        self.tasks.clear();
    }

    pub fn create_task(
        &mut self,
        state: TaskState,
        cloud_uri: impl Into<String>,
        source_rows: usize,
        result_rows: usize,
    ) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.system_rows = [1, 1, 1];
        self.tasks.push(Task {
            id,
            job_id: id,
            state,
            required_slots: 4,
            max_node_count: 1,
            cloud_uri: cloud_uri.into(),
            source_rows,
            result_rows,
            conflict_files: Vec::new(),
            external_meta_cleaned: false,
            subtasks: Vec::new(),
            metering: None,
        });
        id
    }

    pub fn task(&self, id: i64) -> &Task {
        self.tasks
            .iter()
            .find(|task| task.id == id)
            .expect("task history must retain completed task")
    }

    pub fn task_mut(&mut self, id: i64) -> &mut Task {
        self.tasks
            .iter_mut()
            .find(|task| task.id == id)
            .expect("task history must retain completed task")
    }

    pub fn complete_cleanup(&mut self, id: i64) {
        let prefix = format!("import/{id}/");
        self.server.remove_prefix("sorted", &prefix);
        let task = self.task_mut(id);
        task.external_meta_cleaned = true;
        for subtask in &mut task.subtasks {
            subtask.external_path = None;
        }
        self.system_rows = [0; 3];
    }
}

pub fn rows(input: &str) -> Vec<Vec<String>> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(parse_csv_line)
        .collect()
}

pub fn parse_csv_line(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                value.push(ch);
            }
            ',' if !quoted => {
                result.push(normalize_cell(&value));
                value.clear();
            }
            _ => value.push(ch),
        }
    }
    result.push(normalize_cell(&value));
    result
}

fn normalize_cell(value: &str) -> String {
    match value.trim() {
        r"\N" => "NULL".to_owned(),
        value => value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value)
            .replace(", ", ","),
    }
}

fn mvi_values(value: &str) -> Vec<&str> {
    value
        .trim_matches(|ch| ch == '"' || ch == '[' || ch == ']')
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
pub enum UniqueGroup {
    Columns(Vec<usize>),
    MultiValue(usize),
}

fn rows_conflict(left: &[String], right: &[String], groups: &[UniqueGroup]) -> bool {
    groups.iter().any(|group| match group {
        UniqueGroup::Columns(columns) => columns.iter().all(|column| {
            left[*column] != "NULL" && right[*column] != "NULL" && left[*column] == right[*column]
        }),
        UniqueGroup::MultiValue(column) => {
            let right_values = mvi_values(&right[*column]);
            mvi_values(&left[*column])
                .iter()
                .any(|value| right_values.contains(value))
        }
    })
}

pub fn capture_conflicts(
    source: Vec<Vec<String>>,
    groups: &[UniqueGroup],
) -> (Vec<Vec<String>>, usize) {
    let mut conflicted = vec![false; source.len()];
    for left in 0..source.len() {
        for right in left + 1..source.len() {
            if rows_conflict(&source[left], &source[right], groups) {
                conflicted[left] = true;
                conflicted[right] = true;
            }
        }
    }
    let conflict_count = conflicted.iter().filter(|value| **value).count();
    let survivors = source
        .into_iter()
        .zip(conflicted)
        .filter_map(|(row, conflict)| (!conflict).then_some(row))
        .collect();
    (survivors, conflict_count)
}

pub fn sorted_strings(rows: &[Vec<String>]) -> Vec<String> {
    let mut result: Vec<_> = rows.iter().map(|row| row.join(" ")).collect();
    result.sort();
    result
}

pub fn duplicate_key_error_option(explicit: bool) -> &'static str {
    if explicit {
        ", on_duplicate_key='error'"
    } else {
        ""
    }
}

pub fn serial_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn test_import_into_suite_lifecycle_and_helpers() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    assert_eq!(GCS_ENDPOINT, "http://127.0.0.1:4443/storage/v1/");
    suite.prepare_and_use_db("suite_fixture");
    suite.create_table("t");
    suite.system_rows = [2, 3, 4];
    suite.cleanup_sys_tables();
    assert_eq!(suite.active_db, "suite_fixture");
    assert_eq!(suite.system_rows, [0; 3]);
    assert!(suite.tasks.is_empty());
    suite.tear_down();
    assert!(suite.server.stopped);
}

#[test]
fn test_main_preserves_real_tikv_config_and_execution_contract() {
    let _serial = serial_guard();
    reset_test_globals();
    config::UpdateGlobal(|conf| conf.Store = config::StoreTypeTiKV.to_owned());
    UpdateTiDBConfig();
    assert_eq!(config::GetGlobalConfig().Path, "127.0.0.1:2379");

    let mut main = TestMain::new(0);
    assert_eq!(RunTestMain(&mut main), 0);
    assert!(main.wrapped);
    assert!(WithRealTiKV());
}
