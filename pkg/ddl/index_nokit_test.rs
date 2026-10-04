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

// 添加索引（ADD INDEX）DDL 任务的独立单元测试（不依赖 testkit 测试框架）。
//
// 本文件用简化的内存模型模拟两类逻辑并进行验证：
// 1. 当分布式 reorg（索引回填）任务因存储节点（TiKV/TiFlash）磁盘写满
//    而失败时，DDL job 应被系统自动暂停（auto-pause）的判定与执行逻辑；
// 2. 运行期动态修改 reorg 任务参数（并发度、批大小、写入限速）的轮询
//    循环，其缓存只有在修改成功后才更新的语义。
//
// 术语说明：DDL（数据定义语言）指建表、加索引等模式变更操作；
// reorg（reorganization）指加索引时后台扫描全表并回填索引数据的过程。

/// reorg 子任务的运行状态，对应分布式任务框架中任务的生命周期阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskState {
    /// 正在运行。
    Running,
    /// 已暂停（可能由用户或系统触发）。
    Paused,
    /// 正在取消。
    Cancelling,
}

/// reorg（索引回填）任务的可动态调整参数集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReorgConfig {
    /// 回填工作线程并发度。
    concurrency: u64,
    /// 每批次处理的行数。
    batch_size: u64,
    /// 写入存储层的最大速率限制（限流，避免影响在线业务）。
    max_write_speed: u64,
}

/// DDL job 的简化模型，只保留与磁盘写满自动暂停相关的字段。
#[derive(Debug)]
struct Job {
    /// 用户是否以“KV 磁盘已恢复”为理由手动恢复过该 job；
    /// 为 true 时不应再次因同一原因自动暂停。
    resume_reason_kv_disk_full: bool,
    /// job 是否由系统（而非用户）自动暂停。
    paused_by_system: bool,
    /// 暂停原因描述，供用户排查。
    pause_reason: Option<String>,
}

/// 分布式 reorg 子任务的简化模型。
#[derive(Debug)]
struct Task {
    /// 任务 ID。
    id: u64,
    /// 当前任务状态。
    state: TaskState,
    /// 任务失败时的错误信息。
    error: Option<String>,
    /// 任务当前生效的 reorg 参数。
    config: ReorgConfig,
}

/// 判断一个已存在的任务是否应因“存储磁盘写满”而触发 job 自动暂停。
///
/// 条件：任务处于 Paused 状态、错误信息包含 "disk full"，
/// 且用户尚未以磁盘恢复为由手动恢复过该 job（避免反复暂停）。
fn should_auto_pause_existing_kv_disk_full_task(job: &Job, task: &Task) -> bool {
    task.state == TaskState::Paused
        && task
            .error
            .as_deref()
            .is_some_and(|error| error.contains("disk full"))
        && !job.resume_reason_kv_disk_full
}

/// 因存储磁盘写满而自动暂停 ADD INDEX job，返回记录的暂停消息。
///
/// 根据错误文本判断写满的是 TiFlash（列存副本）还是 TiKV（行存引擎），
/// 并在 job 上标记“由系统暂停”及暂停原因，同时清除用户的恢复标记。
fn auto_pause_add_index_job_on_kv_disk_full(job: &mut Job, task_id: u64, error: &str) -> String {
    // 依据错误信息区分磁盘写满发生在哪种存储引擎。
    let lower_error = error.to_ascii_lowercase();
    let storage = if lower_error.contains("tiflash") {
        "TiFlash"
    } else if lower_error.contains("tikv") {
        "TiKV"
    } else {
        "storage node"
    };
    let message = format!("DDL job auto-paused because {storage} disk full on task {task_id}");
    job.resume_reason_kv_disk_full = false;
    job.paused_by_system = true;
    job.pause_reason = Some(message.clone());
    message
}

/// 参数修改轮询循环中每一次轮询可能观察到的事件。
///
/// 模拟真实实现中“读取 job 最新参数 -> 查询任务状态 -> 下发修改”的
/// 各个环节及其可能的错误结果。
#[derive(Clone, Debug)]
enum Poll {
    /// 循环收到结束信号，正常退出。
    Done,
    /// 读取 job 信息出错（可重试，继续下一轮）。
    JobError,
    /// job 已不存在，循环退出。
    JobNotFound,
    /// 读取 CPU/资源信息出错（可重试）。
    CpuError,
    /// 成功读到节点 CPU 数量；实际 required slots 取并发限制与它的较小值。
    Cpu(u64),
    /// 成功读到 job 上的目标 reorg 参数。
    Job(ReorgConfig),
    /// 任务已不存在，循环退出。
    TaskNotFound,
    /// 查询任务状态出错（可重试）。
    TaskError,
    /// 成功读到任务当前状态。
    Task(TaskState),
    /// 下发参数修改失败。
    ModifyError,
    /// 下发参数修改成功。
    ModifyOk,
}

/// 模拟运行期修改 reorg 任务参数的轮询循环。
///
/// 依次消费 `events`：当读到与缓存不同的目标参数时记为待应用（pending），
/// 只有在任务处于 Running 状态且修改下发成功时才更新缓存 `cached`
/// 并记录到 `applied`；各类可重试错误跳过本轮，终止事件退出循环。
/// 返回 (最终缓存参数, 成功应用的参数列表)。
fn modify_task_param_loop(
    events: &[Poll],
    mut cached: ReorgConfig,
) -> (ReorgConfig, Vec<ReorgConfig>) {
    let mut applied = Vec::new();
    let mut pending = None;
    let mut iterator = events.iter();
    while let Some(event) = iterator.next() {
        match event {
            // 终止类事件：结束信号或 job/任务已不存在，退出循环。
            Poll::Done | Poll::JobNotFound | Poll::TaskNotFound => break,
            // 可重试错误：跳过本轮，继续轮询。
            Poll::JobError | Poll::CpuError | Poll::TaskError => continue,
            Poll::Cpu(_) => panic!("orphan CPU result"),
            Poll::Job(config) => {
                // Go 每次读到 job 后都会先调用 adjustConcurrency；CPU 查询失败时
                // 本轮不能继续，成功时 required slots 为 worker limit 与 CPU 数的较小值。
                let Some(cpu_event) = iterator.next() else {
                    break;
                };
                let cpu_count = match cpu_event {
                    Poll::Cpu(count) => *count,
                    Poll::CpuError => continue,
                    _ => panic!("CPU result expected after job"),
                };
                let mut target = *config;
                target.concurrency = target.concurrency.min(cpu_count);
                if target != cached {
                    pending = Some(target);
                } else {
                    pending = None;
                }
            }
            Poll::Task(TaskState::Running) => {
                // 只有存在待应用参数时才尝试下发修改。
                let Some(target) = pending else { continue };
                // 下一个事件即为本次修改的结果。
                match iterator.next() {
                    Some(Poll::ModifyOk) => {
                        // 修改成功后才更新缓存，保证缓存反映实际生效值。
                        cached = target;
                        applied.push(target);
                        pending = None;
                    }
                    // 修改失败保留 pending，等待下一轮重试。
                    Some(Poll::ModifyError) | None => {}
                    Some(_) => panic!("task modification result expected"),
                }
            }
            // 非运行态的任务不接受参数修改，本轮跳过。
            Poll::Task(TaskState::Paused | TaskState::Cancelling) => {}
            // 修改结果事件必须紧跟在 Running 状态之后出现。
            Poll::ModifyError | Poll::ModifyOk => panic!("orphan task modification result"),
        }
    }
    (cached, applied)
}

/// 测试用的初始 reorg 参数。
const CURRENT: ReorgConfig = ReorgConfig {
    concurrency: 1,
    batch_size: 2,
    max_write_speed: 3,
};
/// 测试用的修改后目标 reorg 参数。
const MODIFIED: ReorgConfig = ReorgConfig {
    concurrency: 4,
    batch_size: 5,
    max_write_speed: 6,
};

/// 验证磁盘写满自动暂停的判定条件与暂停执行后的 job 状态。
#[test]
fn test_should_auto_pause_existing_kv_disk_full_task() {
    let mut job = Job {
        resume_reason_kv_disk_full: false,
        paused_by_system: false,
        pause_reason: None,
    };
    let mut task = Task {
        id: 123,
        state: TaskState::Paused,
        error: Some("store 1 disk full".to_owned()),
        config: CURRENT,
    };
    assert!(should_auto_pause_existing_kv_disk_full_task(&job, &task));
    // 用户已以磁盘恢复为由手动恢复过，不应再次自动暂停。
    job.resume_reason_kv_disk_full = true;
    assert!(!should_auto_pause_existing_kv_disk_full_task(&job, &task));
    job.resume_reason_kv_disk_full = false;
    // 任务仍在运行（未暂停），不满足自动暂停条件。
    task.state = TaskState::Running;
    assert!(!should_auto_pause_existing_kv_disk_full_task(&job, &task));
    task.state = TaskState::Paused;
    // 错误信息不含 "disk full" 关键字时不触发。
    task.error = Some("not disk full capacity error".replace("disk full", "capacity"));
    assert!(!should_auto_pause_existing_kv_disk_full_task(&job, &task));

    // 执行自动暂停：错误提及 TiFlash，消息应标记 TiFlash 而非 TiKV，
    // 并重置用户恢复标记、置位系统暂停标记。
    job.resume_reason_kv_disk_full = true;
    let error = auto_pause_add_index_job_on_kv_disk_full(
        &mut job,
        task.id,
        "remaining storage capacity of TiFlash is less than 10%",
    );
    assert!(error.contains("TiFlash disk full"));
    assert!(!error.contains("TiKV"));
    assert!(job.paused_by_system);
    assert!(!job.resume_reason_kv_disk_full);
    assert!(
        job.pause_reason
            .as_deref()
            .unwrap()
            .contains("TiFlash disk full")
    );
    assert_eq!(CURRENT, task.config);
}

/// 验证轮询循环的各类退出与重试路径：均不应产生任何参数修改。
#[test]
fn test_modify_task_param_loop_exit_and_retry_paths() {
    for events in [
        vec![Poll::Done],
        vec![Poll::JobError, Poll::JobNotFound],
        vec![Poll::Job(CURRENT), Poll::CpuError, Poll::JobNotFound],
        vec![Poll::Job(CURRENT), Poll::Cpu(123), Poll::JobNotFound],
        vec![Poll::Job(MODIFIED), Poll::Cpu(123), Poll::TaskNotFound],
        vec![
            Poll::Job(MODIFIED),
            Poll::Cpu(123),
            Poll::TaskError,
            Poll::Job(MODIFIED),
            Poll::Cpu(123),
            Poll::Task(TaskState::Cancelling),
            Poll::TaskNotFound,
        ],
    ] {
        let (cached, applied) = modify_task_param_loop(&events, CURRENT);
        assert_eq!(CURRENT, cached);
        assert!(applied.is_empty());
    }
}

/// 验证首次修改失败后缓存不变，重试成功后缓存才更新为目标值。
#[test]
fn test_modify_task_param_loop_updates_cache_only_after_success() {
    let events = [
        Poll::Job(MODIFIED),
        Poll::Cpu(123),
        Poll::Task(TaskState::Running),
        Poll::ModifyError,
        Poll::Job(MODIFIED),
        Poll::Cpu(123),
        Poll::Task(TaskState::Running),
        Poll::ModifyOk,
        Poll::Job(MODIFIED),
        Poll::Cpu(123),
        Poll::JobNotFound,
    ];
    let (cached, applied) = modify_task_param_loop(&events, CURRENT);
    assert_eq!(MODIFIED, cached);
    assert_eq!(vec![MODIFIED], applied);
}

/// 验证连续两次不同的参数变更能被依次应用，且后一次覆盖前一次的 pending。
#[test]
fn test_modify_task_param_loop_applies_two_distinct_changes() {
    let modified_twice = ReorgConfig {
        concurrency: 7,
        batch_size: 8,
        max_write_speed: 9,
    };
    let events = [
        Poll::Job(MODIFIED),
        Poll::Cpu(123),
        Poll::Task(TaskState::Running),
        Poll::ModifyOk,
        Poll::Job(MODIFIED),
        Poll::Cpu(123),
        Poll::Job(modified_twice),
        Poll::Cpu(123),
        Poll::Task(TaskState::Running),
        Poll::ModifyOk,
        Poll::JobNotFound,
    ];
    let (cached, applied) = modify_task_param_loop(&events, CURRENT);
    assert_eq!(modified_twice, cached);
    assert_eq!(vec![MODIFIED, modified_twice], applied);
}

/// Go's adjustConcurrency applies the node CPU ceiling before comparing and
/// submitting required slots.
#[test]
fn test_modify_task_param_loop_caps_concurrency_by_cpu_count() {
    let target = ReorgConfig {
        concurrency: 64,
        batch_size: CURRENT.batch_size,
        max_write_speed: CURRENT.max_write_speed,
    };
    let events = [
        Poll::Job(target),
        Poll::Cpu(8),
        Poll::Task(TaskState::Running),
        Poll::ModifyOk,
        Poll::JobNotFound,
    ];
    let (cached, applied) = modify_task_param_loop(&events, CURRENT);
    assert_eq!(8, cached.concurrency);
    assert_eq!(vec![cached], applied);
}

#[test]
fn test_auto_pause_store_type_matches_go_fallback_and_case_folding() {
    let mut job = Job {
        resume_reason_kv_disk_full: false,
        paused_by_system: false,
        pause_reason: None,
    };
    assert!(
        auto_pause_add_index_job_on_kv_disk_full(&mut job, 1, "unknown disk full")
            .contains("storage node disk full")
    );
    assert!(
        auto_pause_add_index_job_on_kv_disk_full(&mut job, 1, "TIKV disk full")
            .contains("TiKV disk full")
    );
}

/// Row-size estimation must preserve the Go distinction between unknown
/// statistics (zero) and a positive Region fallback.
#[test]
fn test_estimate_table_row_size_preserves_unknown_statistics() {
    use crate::index::estimate_table_row_size;

    assert_eq!(estimate_table_row_size(0, 0, 0), 0);
    assert_eq!(estimate_table_row_size(0, 0, 19), 19);
    assert_eq!(estimate_table_row_size(171, 9, 0), 19);
    assert_eq!(estimate_table_row_size(-1, 1, 99), 1);
}

/// Bounded pre-split interpolation must retain the complete table/index
/// prefix so downstream routing can still decode both identifiers.
#[test]
fn test_bounded_index_presplit_keys_retain_decodable_prefix() {
    use crate::index_cop::Datum;
    use crate::index_presplit::get_split_keys_from_bound;

    let table_id = 42_i64;
    let index_id = 7_i64;
    let keys = get_split_keys_from_bound(
        table_id,
        index_id,
        &[Datum::Int(0)],
        &[Datum::Int(100_000)],
        3,
    )
    .expect("bounded index pre-split keys");
    assert_eq!(keys.len(), 2);
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));

    for key in keys {
        assert!(key.len() >= 18, "split key lost its routing prefix");
        assert_eq!(key[0], b't');
        assert_eq!(
            i64::from_be_bytes(key[1..9].try_into().expect("table id bytes")),
            table_id
        );
        assert_eq!(key[9], b'i');
        assert_eq!(
            i64::from_be_bytes(key[10..18].try_into().expect("index id bytes")),
            index_id
        );
    }
}

#[test]
fn cloud_storage_uri_is_reloaded_after_owner_failover() {
    use crate::index::resolve_cloud_storage_uri_after_owner_failover;

    let mut cached = String::new();
    let uri =
        resolve_cloud_storage_uri_after_owner_failover(900_001, true, false, &mut cached, || {
            "s3://bucket/dxf/".to_owned()
        })
        .expect("the new owner should reload the configured URI");
    assert_eq!(uri, "s3://bucket/dxf/");
    assert_eq!(cached, uri);

    let cached_uri =
        resolve_cloud_storage_uri_after_owner_failover(900_001, true, false, &mut cached, || {
            "s3://changed/dxf/".to_owned()
        })
        .expect("an existing owner cache wins over changed configuration");
    assert_eq!(cached_uri, "s3://bucket/dxf/");
}

#[test]
fn cloud_storage_uri_recovery_preserves_go_error_and_bypass_paths() {
    use crate::index::resolve_cloud_storage_uri_after_owner_failover;

    let mut cached = String::new();
    let error = resolve_cloud_storage_uri_after_owner_failover(
        900_001,
        true,
        false,
        &mut cached,
        String::new,
    )
    .expect_err("a cloud job cannot silently resume in local mode");
    assert_eq!(
        error,
        "cloud storage URI is empty for add-index job 900001 with cloud storage enabled"
    );

    assert_eq!(
        resolve_cloud_storage_uri_after_owner_failover(
            900_001,
            false,
            false,
            &mut cached,
            || panic!("local sort must not load cloud configuration"),
        )
        .unwrap(),
        ""
    );
    assert_eq!(
        resolve_cloud_storage_uri_after_owner_failover(
            900_001,
            true,
            true,
            &mut cached,
            || panic!("merge-temp-index must not load cloud configuration"),
        )
        .unwrap(),
        ""
    );
}
