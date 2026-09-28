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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// DDL（Data Definition Language，数据定义语言）核心模块。
//
// DDL 指 CREATE/ALTER/DROP 等修改库表结构（schema，即元数据）的语句。
// 本模块提供 DDL 子系统的核心抽象：
// - [`Job`]：一次 DDL 变更任务及其状态机（见 [`JobState`]）；
// - [`JobWrapper`]：包装 Job 并附带结果通知通道，供提交方同步等待执行结果；
// - [`UnsyncedJobTracker`]：跟踪尚未完成 schema 同步的任务
//   （分布式场景下所有节点需就同一版本的 schema 达成一致）；
// - [`Ddl`]：DDL 子系统本体，负责启动/停止、任务提交、完成归档，
//   以及暂停/恢复/取消等管理命令的处理。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;

use crate::options::{OptionFn, Options, apply_options};

/// 当前 DDL Job 结构的版本号，写入每个新建/提交的 Job，
/// 便于未来对 Job 序列化格式做兼容升级。
pub const CURRENT_VERSION: i64 = 1;
/// DDL 子系统的启动模式。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StartMode {
    /// 普通启动（默认）。
    #[default]
    Normal,
    /// 集群首次初始化（bootstrap）时启动，需要创建系统表等初始元数据。
    Bootstrap,
    /// 集群版本升级过程中启动，可能需要执行升级相关的元数据变更。
    Upgrade,
}
/// 创建对象（如表）时，若同名对象已存在的处理策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnExist {
    /// 报错（默认），对应普通的 `CREATE TABLE`。
    #[default]
    Error,
    /// 忽略本次创建，对应 `CREATE TABLE IF NOT EXISTS`。
    Ignore,
    /// 替换已有对象，对应 `CREATE OR REPLACE` 语义。
    Replace,
}
/// 建表操作的配置项。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CreateTableConfig {
    /// 同名表已存在时的处理策略。
    pub on_exist: OnExist,
    /// 表 ID 是否已由调用方预先分配（如批量建表时统一分配 ID）。
    pub id_allocated: bool,
}
/// 构造 [`CreateTableConfig`]；`on_exist` 传 `None` 时使用默认策略（报错）。
pub fn create_table_config(on_exist: Option<OnExist>, id_allocated: bool) -> CreateTableConfig {
    CreateTableConfig {
        on_exist: on_exist.unwrap_or_default(),
        id_allocated,
    }
}

/// DDL Job 的状态机。
///
/// 典型流转：`None`（初始）→ `Running`（执行中）→ `Done`（变更完成）
/// → `Synced`（所有节点已同步到新 schema）。
/// 期间可被管理命令切换为 `Paused`（已暂停）或 `Cancelling` → `Cancelled`（取消流程）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    /// 初始状态，尚未提交执行。
    None,
    /// 正在执行。
    Running,
    /// 已被暂停，可通过 resume 命令恢复。
    Paused,
    /// 正在取消（需要回滚已做的部分变更）。
    Cancelling,
    /// 已取消。
    Cancelled,
    /// 变更本身已完成，但尚未确认全部节点同步。
    Done,
    /// 已完成且所有节点均已同步到最新 schema，Job 进入历史记录。
    Synced,
}
/// 管理命令（暂停/恢复/取消）的发起者。
///
/// 用于区分权限：系统暂停的 Job 不允许普通用户恢复。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminCommandOperator {
    /// 用户通过 SQL（如 `ADMIN PAUSE DDL JOBS`）发起。
    User,
    /// 系统内部发起（如升级流程自动暂停 DDL）。
    System,
}
/// 与恢复表扫描相关的 DDL 动作类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionType {
    /// 非 DROP/TRUNCATE 动作，不参与恢复表候选扫描。
    #[default]
    Other,
    /// DROP TABLE。
    DropTable,
    /// TRUNCATE TABLE。
    TruncateTable,
}
/// 一次 DDL 变更任务的元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    /// 全局唯一的 Job ID。
    pub id: i64,
    /// 触发本次变更的原始 SQL 语句文本。
    pub query: String,
    /// 当前状态，见 [`JobState`]。
    pub state: JobState,
    /// Job 结构版本号，见 [`CURRENT_VERSION`]。
    pub version: i64,
    /// 事务开始时间戳（TSO，全局单调递增的逻辑时间戳），
    /// 也用于 GC（垃圾回收）安全点判断，见 [`recover_snapshot_ts`]。
    pub start_ts: u64,
    /// DDL 真正开始执行的时间戳；非零时恢复逻辑优先使用它。
    pub real_start_ts: u64,
    /// DDL 动作类型。
    pub action_type: ActionType,
    /// 目标表的 ID。
    pub table_id: i64,
    /// 目标库（schema/database）的 ID。
    pub schema_id: i64,
    /// 若处于暂停状态，记录是谁暂停的；否则为 `None`。
    pub paused_by: Option<AdminCommandOperator>,
}

impl Job {
    /// 创建一个初始状态（`None`）的新 Job，版本号取 [`CURRENT_VERSION`]。
    pub fn new(id: i64, schema_id: i64, table_id: i64, query: impl Into<String>) -> Self {
        Self {
            id,
            query: query.into(),
            state: JobState::None,
            version: CURRENT_VERSION,
            start_ts: 0,
            real_start_ts: 0,
            action_type: ActionType::Other,
            table_id,
            schema_id,
            paused_by: None,
        }
    }
}

/// Job 的包装器：在 [`Job`] 之上附加一条一次性结果通道，
/// 使提交 DDL 的会话可以阻塞等待后台执行完成的结果。
pub struct JobWrapper {
    /// 被包装的 DDL 任务。
    pub job: Job,
    /// Job ID 是否已由调用方预先分配。
    pub id_allocated: bool,
    // 发送端用 Option 包裹：notify_result 时 take 出来发送，保证只通知一次。
    result_sender: Option<mpsc::Sender<Result<(), String>>>,
    result_receiver: mpsc::Receiver<Result<(), String>>,
}
impl JobWrapper {
    /// 包装一个 Job，并创建内部的结果通知通道。
    pub fn new(job: Job, id_allocated: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            job,
            id_allocated,
            result_sender: Some(sender),
            result_receiver: receiver,
        }
    }
    /// 由执行方调用：把执行结果发给等待方。重复调用只有第一次生效。
    pub fn notify_result(&mut self, result: Result<(), String>) {
        if let Some(sender) = self.result_sender.take() {
            let _ = sender.send(result);
        }
    }
    /// 由提交方调用：阻塞等待执行结果；若通道已关闭（执行方异常退出）则返回错误。
    pub fn wait_result(&self) -> Result<(), String> {
        self.result_receiver
            .recv()
            .unwrap_or_else(|_| Err("DDL result channel closed".into()))
    }
}

/// 未同步 Job 的跟踪器。
///
/// 分布式数据库中，一次 DDL 完成后需要等待所有节点加载新版本 schema
/// （即“schema 同步”）。本结构记录哪些 Job 尚未同步完成，
/// 以及哪些 Job 可能已经执行过一次（用于故障恢复后避免重复执行副作用）。
#[derive(Default)]
pub struct UnsyncedJobTracker {
    /// 已完成变更但尚未确认全部节点同步的 Job ID 集合。
    unsynced: BTreeSet<i64>,
    /// 可能已经执行过一次的 Job ID 集合（重启恢复时用于幂等判断）。
    already_run_once: BTreeSet<i64>,
}
impl UnsyncedJobTracker {
    /// 标记某 Job 为未同步。
    pub fn add_unsynced(&mut self, id: i64) {
        self.unsynced.insert(id);
    }
    /// 查询某 Job 是否处于未同步状态。
    pub fn is_unsynced(&self, id: i64) -> bool {
        self.unsynced.contains(&id)
    }
    /// 同步完成后移除未同步标记。
    pub fn remove_unsynced(&mut self, id: i64) {
        self.unsynced.remove(&id);
    }
    /// 查询某 Job 是否可能已执行过一次。
    pub fn maybe_already_run_once(&self, id: i64) -> bool {
        self.already_run_once.contains(&id)
    }
    /// 标记某 Job 已执行过一次。
    pub fn set_already_run_once(&mut self, id: i64) {
        self.already_run_once.insert(id);
    }
}

/// DDL 子系统本体：管理生命周期、任务队列与管理命令。
pub struct Ddl {
    /// 本 DDL 实例（节点）的唯一标识。
    pub id: String,
    /// 通过 Option 函数模式装配的配置项（如底层存储 store）。
    pub options: Options,
    /// 是否已启动（start 成功后为 true）。
    pub started: bool,
    /// 是否允许接收新的 DDL 任务（可被动态开关）。
    pub enabled: bool,
    /// 是否开启 TiFlash（列式存储副本引擎）副本状态轮询。
    pub tiflash_poll_enabled: bool,
    /// 是否启用元数据锁（MDL，防止 DDL 与正在使用旧 schema 的事务冲突）。
    pub metadata_lock_enabled: bool,
    /// 启动模式，见 [`StartMode`]。
    pub start_mode: StartMode,
    /// 进行中的 Job，按 ID 排序存储。
    pub jobs: BTreeMap<i64, Job>,
    /// 已完成（Synced）的历史 Job 记录。
    pub history: Vec<Job>,
    /// 未同步 Job 跟踪器。
    pub tracker: UnsyncedJobTracker,
}
impl Ddl {
    /// 创建一个未启动的 DDL 实例，并应用传入的配置选项。
    pub fn new(id: impl Into<String>, options: impl IntoIterator<Item = OptionFn>) -> Self {
        Self {
            id: id.into(),
            options: apply_options(options),
            started: false,
            enabled: true,
            tiflash_poll_enabled: true,
            metadata_lock_enabled: true,
            start_mode: StartMode::Normal,
            jobs: BTreeMap::new(),
            history: Vec::new(),
            tracker: UnsyncedJobTracker::default(),
        }
    }
    /// 以指定模式启动 DDL 子系统。
    ///
    /// 幂等：已启动时直接返回 Ok；启动前要求已配置底层存储（store）。
    pub fn start(&mut self, mode: StartMode) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        if self.options.store.is_none() {
            return Err("DDL store is not configured".into());
        }
        self.start_mode = mode;
        self.started = true;
        Ok(())
    }
    /// 停止 DDL 子系统（不清空已有任务，仅停止接收与执行）。
    pub fn stop(&mut self) {
        self.started = false;
    }
    /// 允许接收新的 DDL 任务。
    pub fn enable_ddl(&mut self) {
        self.enabled = true;
    }
    /// 禁止接收新的 DDL 任务（如集群升级期间临时关闭）。
    pub fn disable_ddl(&mut self) {
        self.enabled = false;
    }
    /// 切换元数据锁开关，返回值表示状态是否发生了变化。
    pub fn switch_metadata_lock(&mut self, enabled: bool) -> bool {
        let changed = self.metadata_lock_enabled != enabled;
        self.metadata_lock_enabled = enabled;
        changed
    }
    /// 提交一个新的 DDL 任务。
    ///
    /// 要求子系统已启动且未被禁用，且 Job ID 不能重复。
    /// 提交时统一刷新版本号、置为 Running，并登记到未同步跟踪器。
    pub fn submit_job(&mut self, mut job: Job) -> Result<(), String> {
        if !self.started || !self.enabled {
            return Err("DDL is not running".into());
        }
        if self.jobs.contains_key(&job.id) {
            return Err(format!("DDL job {} already exists", job.id));
        }
        job.version = CURRENT_VERSION;
        job.state = JobState::Running;
        self.tracker.add_unsynced(job.id);
        self.jobs.insert(job.id, job);
        Ok(())
    }
    /// 完成指定 Job：从运行队列移除、清除未同步标记、
    /// 置为 Synced 并归档到历史记录。
    pub fn finish_job(&mut self, id: i64) -> Result<(), String> {
        let mut job = self
            .jobs
            .remove(&id)
            .ok_or_else(|| format!("DDL job {id} not found"))?;
        job.state = JobState::Synced;
        self.tracker.remove_unsynced(id);
        self.history.push(job);
        Ok(())
    }
    /// 对一批 Job 逐个执行管理命令，返回与 `ids` 顺序对应的结果列表，
    /// 单个 Job 失败不影响其余 Job 的处理。
    pub fn process_jobs(
        &mut self,
        ids: &[i64],
        command: JobCommand,
        operator: AdminCommandOperator,
    ) -> Vec<Result<(), String>> {
        ids.iter()
            .map(|id| self.process_job(*id, command, operator))
            .collect()
    }
    /// 在一个可提交的事务边界内执行批量管理命令。
    ///
    /// Go 的 `PauseJobs` 会先返回每个 job 的处理结果，再提交包含状态转换的
    /// 元数据事务。提交或并发写冲突失败时，调用方仍能观察逐 job 结果，但
    /// 任何暂存的状态转换都不能泄漏到内存状态中。`commit` 抽象该提交边界，
    /// 既允许生产接线真实存储，也允许测试注入确定性的提交失败。
    pub fn process_jobs_transactionally<F>(
        &mut self,
        ids: &[i64],
        command: JobCommand,
        operator: AdminCommandOperator,
        commit: F,
    ) -> (Vec<Result<(), String>>, Result<(), String>)
    where
        F: FnOnce() -> Result<(), String>,
    {
        let jobs_before_command = self.jobs.clone();
        let job_results = self.process_jobs(ids, command, operator);
        let commit_result = commit();
        if commit_result.is_err() {
            self.jobs = jobs_before_command;
        }
        (job_results, commit_result)
    }
    /// 对单个 Job 执行管理命令，校验状态机转换是否合法：
    /// - Cancel：仅允许 Running/Paused → Cancelling；
    /// - Pause：仅允许 Running → Paused，并记录暂停发起者；
    /// - Resume：仅允许 Paused → Running，且用户不能恢复系统暂停的 Job。
    fn process_job(
        &mut self,
        id: i64,
        command: JobCommand,
        operator: AdminCommandOperator,
    ) -> Result<(), String> {
        let job = self
            .jobs
            .get_mut(&id)
            .ok_or_else(|| format!("DDL job {id} not found"))?;
        match command {
            JobCommand::Cancel if matches!(job.state, JobState::Running | JobState::Paused) => {
                job.state = JobState::Cancelling;
                Ok(())
            }
            JobCommand::Pause if job.state == JobState::Running => {
                job.state = JobState::Paused;
                job.paused_by = Some(operator);
                Ok(())
            }
            JobCommand::Resume if job.state == JobState::Paused => {
                if job.paused_by == Some(AdminCommandOperator::System)
                    && operator == AdminCommandOperator::User
                {
                    return Err("user cannot resume a system-paused DDL job".into());
                }
                job.state = JobState::Running;
                job.paused_by = None;
                Ok(())
            }
            _ => Err(format!(
                "invalid state {:?} for command {:?}",
                job.state, command
            )),
        }
    }
    /// 返回全部 Job 的快照：进行中的任务在前，历史记录在后。
    pub fn all_jobs(&self) -> Vec<Job> {
        self.jobs
            .values()
            .cloned()
            .chain(self.history.iter().cloned())
            .collect()
    }
}
/// 可作用于 DDL Job 的管理命令。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobCommand {
    /// 取消 Job（进入 Cancelling 回滚流程）。
    Cancel,
    /// 暂停 Job。
    Pause,
    /// 恢复被暂停的 Job。
    Resume,
}
/// 获取恢复表数据（如 `RECOVER TABLE`/`FLASHBACK`）时使用的快照时间戳。
/// Go 侧优先采用非零的 `RealStartTS`，旧任务没有该值时回退到 `StartTS`。
pub fn recover_snapshot_ts(job: &Job) -> u64 {
    if job.real_start_ts != 0 {
        job.real_start_ts
    } else {
        job.start_ts
    }
}
/// 在给定的 Job 列表中查找可恢复的 DROP/TRUNCATE TABLE 任务。
///
/// 只考虑 DROP TABLE/TRUNCATE TABLE Job。每个候选先按恢复快照时间戳校验
/// GC 安全点；快照过旧时立即返回错误，否则调用 `visit`，其返回 true 时短路。
pub fn drop_or_truncate_table_info_from_jobs(
    jobs: &[Job],
    gc_safe_point: u64,
    mut visit: impl FnMut(&Job) -> bool,
) -> Result<bool, String> {
    for job in jobs {
        if !matches!(
            job.action_type,
            ActionType::DropTable | ActionType::TruncateTable
        ) {
            continue;
        }
        let snapshot_ts = recover_snapshot_ts(job);
        if gc_safe_point > snapshot_ts {
            return Err(format!(
                "snapshot timestamp {snapshot_ts} is older than GC safe point {gc_safe_point}"
            ));
        }
        if visit(job) {
            return Ok(true);
        }
    }
    Ok(false)
}
