// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 任务取消（cancel）功能的单元测试。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/DROP/ALTER 等修改
// 库表结构的语句。这类操作在数据库内核中以后台"任务（Job）"的形式异步执行，
// 用户可以通过管理命令（如 ADMIN CANCEL DDL JOBS）取消尚未完成的 DDL 任务。
// 本文件验证多种 DDL 任务的取消流程、暂停后再取消、以及任务执行前取消等场景。

use crate::ddl::{AdminCommandOperator, Ddl, Job, JobCommand, JobState};
use crate::job_worker::{JobContext, JobWorker, WorkerType};
use crate::schema_version::SchemaAction;

/// 构造一个测试用的 DDL 任务（Job）。
///
/// - `id`：任务唯一标识，同时被复用为 `start_ts`（启动时间戳）与 `table_id`；
/// - `query`：触发该任务的原始 SQL 语句文本；
/// - 初始状态为 `JobState::None`，表示任务尚未开始执行。
fn job(id: i64, query: &str) -> Job {
    Job {
        id,
        query: query.into(),
        state: JobState::None,
        version: 0,
        start_ts: id as u64,
        real_start_ts: 0,
        action_type: crate::ddl::ActionType::Other,
        table_id: id,
        schema_id: 1,
        paused_by: None,
    }
}

/// 创建一个已处于"运行中"状态的 DDL 管理器实例。
///
/// `started = true` 模拟 DDL 后台工作循环已启动，
/// 使得后续提交的任务可以被取消/暂停等管理命令处理。
fn running_ddl() -> Ddl {
    let mut ddl = Ddl::new("cancel-test", Vec::new());
    ddl.started = true;
    ddl
}

/// 测试批量取消多种类型的 DDL 任务。
///
/// 覆盖建库/删库、建表/删表、加列/删列、加索引/删索引、
/// 截断表（truncate）与重命名表等常见 DDL；
/// 同时验证取消不存在的任务（id=999）会返回错误。
#[test]
fn test_cancel_various_jobs() {
    let mut ddl = running_ddl();
    // 各类典型 DDL 语句，每条对应一个待提交的任务。
    let queries = [
        "create schema s",
        "drop schema s",
        "create table t(a int)",
        "drop table t",
        "add column b int",
        "drop column b",
        "add index i(a)",
        "drop index i",
        "truncate table t",
        "rename table t to t1",
    ];
    // 逐条提交任务，任务 id 从 1 开始递增。
    for (offset, query) in queries.iter().enumerate() {
        ddl.submit_job(job(offset as i64 + 1, query)).unwrap();
    }
    let ids: Vec<_> = (1..=queries.len() as i64).collect();
    // 以普通用户身份对所有任务下发取消（Cancel）命令，预期全部成功。
    let results = ddl.process_jobs(&ids, JobCommand::Cancel, AdminCommandOperator::User);
    assert!(results.iter().all(Result::is_ok));
    // 取消成功后任务进入 Cancelling（取消中）状态，等待后台真正回滚。
    assert!(
        ids.iter()
            .all(|id| ddl.jobs[id].state == JobState::Cancelling)
    );
    // 单个不存在的 ID 不应中断同批其余 job，返回值顺序与请求一致。
    ddl.submit_job(job(11, "create table mixed_a(a int)"))
        .unwrap();
    ddl.submit_job(job(12, "create table mixed_b(a int)"))
        .unwrap();
    let mixed = ddl.process_jobs(
        &[11, 999, 12],
        JobCommand::Cancel,
        AdminCommandOperator::User,
    );
    assert!(mixed[0].is_ok());
    assert!(mixed[1].is_err()); // 999 不存在。
    assert!(mixed[2].is_ok());
    assert_eq!(JobState::Cancelling, ddl.jobs[&11].state);
    assert_eq!(JobState::Cancelling, ddl.jobs[&12].state);
}

/// 测试"添加唯一索引"任务的暂停后取消流程。
///
/// 验证状态转换链：None -> Paused（暂停）-> Cancelling（取消中），
/// 并确认对已处于取消中的任务重复下发取消命令会失败。
#[test]
fn test_cancel_for_add_unique_index() {
    let mut ddl = running_ddl();
    ddl.submit_job(job(1, "alter table t add unique index uk(a)"))
        .unwrap();
    // 先暂停任务，状态应变为 Paused。
    assert!(ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::User)[0].is_ok());
    assert_eq!(JobState::Paused, ddl.jobs[&1].state);
    // 暂停中的任务仍可被取消，状态转为 Cancelling。
    assert!(ddl.process_jobs(&[1], JobCommand::Cancel, AdminCommandOperator::User)[0].is_ok());
    assert_eq!(JobState::Cancelling, ddl.jobs[&1].state);
    // 已在取消中的任务不允许再次取消。
    assert!(ddl.process_jobs(&[1], JobCommand::Cancel, AdminCommandOperator::User)[0].is_err());
}

/// 测试在任务尚未开始执行时（状态仍为 None）就将其取消。
///
/// 以系统（System）身份下发取消命令，验证任务状态进入 Cancelling，
/// 并且跟踪器（tracker）中该任务被标记为未同步（unsynced），
/// 即其元数据变更尚未同步到集群中的其他节点。
#[test]
fn test_cancel_job_before_run() {
    let mut ddl = running_ddl();
    ddl.submit_job(job(1, "alter table t add index idx(a)"))
        .unwrap();
    let result = ddl.process_jobs(&[1], JobCommand::Cancel, AdminCommandOperator::System);
    assert!(result[0].is_ok());
    assert_eq!(JobState::Cancelling, ddl.jobs[&1].state);
    // 取消动作会使任务在同步跟踪器中处于未同步状态。
    assert!(ddl.tracker.is_unsynced(1));

    // worker 收到 Cancelling job 后直接归档为 Cancelled，不应产生 SchemaDiff。
    // 这对应 Go 在 beforeTransitOneJobStep 取消 truncate 时，表数据保持不变。
    let mut cancelled = ddl.jobs.remove(&1).unwrap();
    let mut worker = JobWorker::new(WorkerType::General);
    let mut context = JobContext::default();
    let diff = worker
        .transit_one_job_step(&mut context, &mut cancelled, SchemaAction::TruncateTable)
        .unwrap();
    assert!(diff.is_none());
    assert_eq!(JobState::Cancelled, cancelled.state);
    assert_eq!(1, worker.history.len());
    assert_eq!(JobState::Cancelled, worker.history[0].state);
}
