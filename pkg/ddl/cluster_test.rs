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

// FLASHBACK CLUSTER（集群闪回）DDL 作业的单元测试。
//
// FLASHBACK CLUSTER 是将整个集群的数据回退到某个历史时间点的 DDL 操作。
// 执行期间需要：暂停 GC（垃圾回收，负责清理 MVCC 历史版本，若不暂停会导致
// 目标时间点的历史数据被清除）、暂停自动统计信息收集（auto analyze）、
// 将集群置为只读（super_read_only）、关闭 TTL 任务（按存活时间自动删除数据），
// 并把 PD（Placement Driver，负责 Region 调度的集群管理组件）的
// merge-schedule-limit 调为 0 以停止 Region 合并调度。
// 本文件用简化的状态机模型验证上述状态流转、变量保存/恢复以及取消语义。

use crate::cluster::{
    ClusterError, ClusterSettings, FlashbackAction, FlashbackJob, FlashbackState, KeyRange,
    merge_continuous_key_ranges,
};

fn key_range(start: u8, end: u8) -> KeyRange {
    KeyRange {
        start: vec![start],
        end: vec![end],
    }
}

fn cluster_settings() -> ClusterSettings {
    ClusterSettings {
        gc_enabled: true,
        super_read_only: false,
        ttl_job_enabled: true,
        auto_analyze_enabled: true,
        pd_schedulers: vec!["merge-schedule-limit".to_owned()],
    }
}

#[test]
fn production_merge_matches_go_exclusion_contract() {
    assert_eq!(
        vec![key_range(1, 6)],
        merge_continuous_key_ranges(vec![key_range(1, 2), key_range(5, 6)], &[]).unwrap()
    );
    assert_eq!(
        vec![key_range(1, 2), key_range(5, 6)],
        merge_continuous_key_ranges(vec![key_range(1, 2), key_range(5, 6)], &[key_range(3, 4)],)
            .unwrap()
    );
    assert_eq!(
        Err(ClusterError::InvalidRange),
        merge_continuous_key_ranges(vec![key_range(2, 1)], &[])
    );
}

#[test]
fn production_rejects_cancel_after_flashback_writes_begin() {
    let original = cluster_settings();
    let mut settings = original.clone();
    let mut job = FlashbackJob::new(10, vec![key_range(1, 2)]);
    job.apply_action(FlashbackAction::Prepare, &mut settings)
        .unwrap();
    job.apply_action(FlashbackAction::Prepare, &mut settings)
        .unwrap();
    job.apply_action(FlashbackAction::Flashback, &mut settings)
        .unwrap();

    assert_eq!(Err(ClusterError::InvalidState), job.cancel(&mut settings));

    assert_eq!(FlashbackState::FlashingBack, job.state);
    assert_ne!(original, settings);
}

#[test]
fn production_finish_and_early_cancel_restore_go_settings() {
    let original = cluster_settings();

    let mut cancelled_settings = original.clone();
    let mut cancelled = FlashbackJob::new(10, vec![key_range(1, 2)]);
    cancelled
        .apply_action(FlashbackAction::Prepare, &mut cancelled_settings)
        .unwrap();
    assert_eq!(Ok(()), cancelled.cancel(&mut cancelled_settings));
    assert_eq!(FlashbackState::Cancelled, cancelled.state);
    assert_eq!(original, cancelled_settings);

    let mut finished_settings = original.clone();
    let mut finished = FlashbackJob::new(10, vec![key_range(1, 2)]);
    for action in [
        FlashbackAction::Prepare,
        FlashbackAction::Prepare,
        FlashbackAction::Flashback,
        FlashbackAction::Flashback,
        FlashbackAction::Finish,
    ] {
        finished
            .apply_action(action, &mut finished_settings)
            .unwrap();
    }
    assert_eq!(FlashbackState::Done, finished.state);
    assert_eq!(original.gc_enabled, finished_settings.gc_enabled);
    assert_eq!(original.super_read_only, finished_settings.super_read_only);
    assert_eq!(
        original.auto_analyze_enabled,
        finished_settings.auto_analyze_enabled
    );
    assert!(!finished_settings.ttl_job_enabled);
    assert_eq!(original.pd_schedulers, finished_settings.pd_schedulers);
}

/// DDL 作业推进过程中的模式（Schema）状态。
///
/// 对应 TiDB 在线 DDL 的多阶段状态机：作业按
/// DeleteOnly -> WriteOnly -> WriteReorganization -> Public 逐步推进，
/// 保证各节点在不同阶段看到的 schema 版本互相兼容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemaState {
    /// 仅删除阶段：新 schema 只对删除操作可见。
    DeleteOnly,
    /// 仅写入阶段：新 schema 对写入可见，尚未开始数据重组。
    WriteOnly,
    /// 写重组阶段：真正执行闪回回退数据的阶段，不允许取消。
    WriteReorganization,
    /// 公开阶段：作业完成，schema 对所有操作可见。
    Public,
}

/// 闪回过程中需要临时调整的全局系统变量集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlobalVariables {
    /// GC（MVCC 历史版本垃圾回收）是否开启。
    gc_enabled: bool,
    /// 自动统计信息收集（auto analyze）是否开启。
    auto_analyze_enabled: bool,
    /// 集群是否处于超级只读模式（禁止一切写入）。
    super_read_only: bool,
    /// TTL（按存活时间自动清理过期行）后台任务是否开启。
    ttl_jobs_enabled: bool,
}

/// FLASHBACK CLUSTER 作业的简化模型。
///
/// 记录作业当前状态、PD 的 merge-schedule-limit（Region 合并调度上限）
/// 以及全局变量的当前值和进入作业前保存的原始值，用于完成或取消时恢复。
struct FlashbackCluster {
    /// 作业当前所处的 schema 状态。
    state: SchemaState,
    /// PD 当前的 merge-schedule-limit，闪回期间会被置 0。
    pd_merge_schedule_limit: u64,
    /// 进入作业前保存的 merge-schedule-limit，用于事后恢复。
    saved_pd_merge_schedule_limit: u64,
    /// 全局变量的当前值。
    variables: GlobalVariables,
    /// 进入作业前保存的全局变量原始值。
    saved_variables: GlobalVariables,
}

impl FlashbackCluster {
    /// 以给定的全局变量与 PD 调度上限创建一个新作业，初始状态为 DeleteOnly，
    /// 同时把入参保存为"原始值"以便结束后恢复。
    fn new(variables: GlobalVariables, merge_schedule_limit: u64) -> Self {
        Self {
            state: SchemaState::DeleteOnly,
            pd_merge_schedule_limit: merge_schedule_limit,
            saved_pd_merge_schedule_limit: merge_schedule_limit,
            variables,
            saved_variables: variables,
        }
    }

    /// 将作业状态机向前推进一步。
    fn advance(&mut self) {
        self.state = match self.state {
            SchemaState::DeleteOnly => SchemaState::WriteOnly,
            // 进入写重组阶段前，关闭 GC/auto analyze/TTL、开启只读、
            // 停止 PD 的 Region 合并调度，保证闪回期间数据与调度稳定。
            SchemaState::WriteOnly => {
                self.variables.gc_enabled = false;
                self.variables.auto_analyze_enabled = false;
                self.variables.super_read_only = true;
                self.variables.ttl_jobs_enabled = false;
                self.pd_merge_schedule_limit = 0;
                SchemaState::WriteReorganization
            }
            SchemaState::WriteReorganization => SchemaState::Public,
            // Public 为终态，继续推进保持不变。
            SchemaState::Public => SchemaState::Public,
        };
    }

    /// 尝试提交新的 DDL 作业：闪回进行期间（非 Public 状态）禁止任何新 DDL。
    fn add_ddl(&self) -> Result<(), &'static str> {
        if self.state != SchemaState::Public {
            return Err("Can't add ddl job, have flashback cluster job");
        }
        Ok(())
    }

    /// 取消闪回作业：写重组阶段已开始改写数据，无法取消；
    /// 其他阶段取消后恢复所有被修改的变量与调度上限。
    fn cancel(&mut self) -> Result<(), &'static str> {
        if self.state == SchemaState::WriteReorganization {
            return Err("flashback cluster job cannot be cancelled in write reorganization");
        }
        self.restore_after_cancel();
        Ok(())
    }

    /// 闪回成功完成：恢复 PD 调度上限与大部分全局变量。
    fn finish(&mut self) {
        self.state = SchemaState::Public;
        self.pd_merge_schedule_limit = self.saved_pd_merge_schedule_limit;
        self.variables.gc_enabled = self.saved_variables.gc_enabled;
        self.variables.super_read_only = self.saved_variables.super_read_only;
        // TiDB deliberately keeps TTL disabled after a successful flashback.
        // TiDB 有意在闪回成功后保持 TTL 关闭，避免刚回退的数据被 TTL 立刻清理。
        self.variables.ttl_jobs_enabled = false;
    }

    /// 取消后完全恢复：全局变量与 PD 调度上限均回到进入作业前的原始值。
    fn restore_after_cancel(&mut self) {
        self.state = SchemaState::Public;
        self.pd_merge_schedule_limit = self.saved_pd_merge_schedule_limit;
        self.variables = self.saved_variables;
    }
}

/// 构造测试用的全局变量组合（auto analyze 固定为开启）。
fn variables(gc: bool, super_read_only: bool, ttl: bool) -> GlobalVariables {
    GlobalVariables {
        gc_enabled: gc,
        auto_analyze_enabled: true,
        super_read_only,
        ttl_jobs_enabled: ttl,
    }
}

/// 验证进入写重组阶段时 PD 的 merge-schedule-limit 被置 0，
/// 取消后恢复为原值且状态回到 Public。
#[test]
fn test_flashback_close_and_reset_pd_schedule() {
    let mut flashback = FlashbackCluster::new(variables(true, false, true), 1);
    flashback.advance();
    flashback.advance();
    assert_eq!(SchemaState::WriteReorganization, flashback.state);
    assert_eq!(0, flashback.pd_merge_schedule_limit);

    flashback.restore_after_cancel();
    assert_eq!(1, flashback.pd_merge_schedule_limit);
    assert_eq!(SchemaState::Public, flashback.state);
}

/// 验证闪回进行期间禁止提交新的 DDL 作业，完成后恢复允许。
#[test]
fn test_add_ddl_during_flashback() {
    let mut flashback = FlashbackCluster::new(variables(true, false, true), 1);
    flashback.advance();
    assert_eq!(SchemaState::WriteOnly, flashback.state);
    assert_eq!(
        Err("Can't add ddl job, have flashback cluster job"),
        flashback.add_ddl()
    );
    flashback.finish();
    assert_eq!(Ok(()), flashback.add_ddl());
}

/// 验证闪回期间全局变量被强制切换到安全组合
/// （GC/auto analyze/TTL 关闭、只读开启），完成后按规则恢复：
/// gc_enabled 与 super_read_only 恢复原值，TTL 保持关闭。
#[test]
fn test_global_variables_on_flashback() {
    // 覆盖两种不同的初始变量组合，确认恢复逻辑与初始值无关。
    for original in [variables(true, false, true), variables(false, true, false)] {
        let mut flashback = FlashbackCluster::new(original, 1);
        flashback.advance();
        flashback.advance();
        assert_eq!(
            GlobalVariables {
                gc_enabled: false,
                auto_analyze_enabled: false,
                super_read_only: true,
                ttl_jobs_enabled: false,
            },
            flashback.variables
        );

        flashback.finish();
        assert_eq!(original.gc_enabled, flashback.variables.gc_enabled);
        assert_eq!(
            original.super_read_only,
            flashback.variables.super_read_only
        );
        assert!(!flashback.variables.ttl_jobs_enabled);
    }
}

/// 验证取消语义：早期阶段可取消并完整恢复变量；
/// 进入写重组阶段后取消被拒绝，只能等待完成。
#[test]
fn test_cancel_flashback_cluster() {
    let original = variables(true, false, true);
    // 早期阶段（DeleteOnly）取消：成功且变量完整恢复。
    let mut early = FlashbackCluster::new(original, 1);
    assert_eq!(Ok(()), early.cancel());
    assert_eq!(original, early.variables);

    // 晚期阶段（WriteReorganization）取消：被拒绝，随后正常完成。
    let mut late = FlashbackCluster::new(original, 1);
    late.advance();
    late.advance();
    assert_eq!(
        Err("flashback cluster job cannot be cancelled in write reorganization"),
        late.cancel()
    );
    late.finish();
    assert!(!late.variables.ttl_jobs_enabled);
}
