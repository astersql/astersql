// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Go-equivalent tests for `randomized_integration_test.go`.
//!
//! utiltest/crr is not a Cargo dependency; the simulation uses an in-memory
//! harness that preserves Go round actions, checkpoint safety checks, and
//! catch-up semantics (same approach as `crr/service/service_test.rs`).
//!
//! 随机 CRR 检查点集成模拟：在内存 PD/上下游上打乱 flush/复制/加减 store/重启。
//! 目标：验证安全点不越过未复制对象，且全局检查点与 catch-up 语义与 Go 一致。
//! 种子可由 TIDB_TEST_SEED 覆盖；默认固定以便本地复现。
//! 每轮动作顺序随机，但断言始终检查水位单调与下游可读性。
//! catch_up_every 轮强制排空 pending 并要求计算器前进，防止长期饥饿。
//! prune/add/remove store 模拟拓扑抖动，覆盖缺失 store 阻塞推进的路径。
//! 重启计算器时可选择携带 SyncedTS，验证持久水位恢复。
//! 本文件只加注释说明意图，不改概率参数或断言逻辑。
//! XorShift64 替代 Go math/rand 固定种子源，避免依赖系统时间。
//! SharedPD/MemStorage/SharedDownstream 三者构成最小可复现 CRR 世界。
//! FlushRecord 把“何时产生的检查点”与对象路径绑定，供安全区间扫描。
//! pending 队列模拟复制延迟；replicate 只 mark 下游存在，不搬字节。
//! global_progress_check 强制抬升 PD，验证计算器能跟随上游前进。
//! catch_up 路径先 prune+replicate_all，再放宽超时做 Compute。
//! 动作名字符串写入 round_log.action_order，失败时可对照顺序。
//! max_buffered_files 限制复制批量，避免单轮吞掉全部缓冲掩盖竞态。
//! restart_carry_synced_ts_chance 现为 100%，聚焦“带状态重启”主路径。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_streamhelper::Store;
use serde_json::json;

use crate::{
    Calculator, CalculatorDeps, CheckpointCalculatorConfig, Context, Error, FileExistenceChecker,
    NewCalculator, NewExistenceSyncChecker, PDMetaReader, PersistentState, UpstreamStorageReader,
    WalkOption,
};

// TestCheckpointCalculatorRandomizedCRRSimulation 对应 Go 随机模拟主测试。
#[test]
/// 主随机模拟：多轮打乱动作后校验水位安全与最终 SyncedTS 前进。
fn test_checkpoint_calculator_randomized_crr_simulation() {
    // 可复现种子；失败时依赖 eprintln 输出 seed。
    let seed = deterministic_seed();
    // 概率与上限刻意偏小，控制单测时长同时覆盖关键分支。
    let cfg = RandomizedCRRSimulationConfig {
        iterations: 300,
        initial_stores: 3,
        max_stores: 12,
        region_count: 12,
        max_non_flush_stores_per_round: 2,
        store_no_flush_chance_percent: 10,
        add_store_chance_percent: 12,
        max_add_stores_per_round: 1,
        remove_store_chance_percent: 10,
        max_remove_stores_per_round: 1,
        scatter_chance_percent: 3,
        max_scatter_regions_per_round: 3,
        replicate_chance_percent: 90,
        max_replicate_batch_per_round: 20,
        max_buffered_files: 32,
        restart_calculator_chance_percent: 10,
        restart_carry_synced_ts_chance_percent: 100,
        prune_empty_store_after_rounds: 3,
        prune_empty_store_chance_percent: 85,
        global_progress_check_every: 13,
        catch_up_every: 299,
        compute_timeout: Duration::from_millis(2),
        catch_up_timeout: Duration::from_millis(100),
        calculator_poll_interval: Duration::from_millis(1),
    };
    // 打印 seed 便于 CI 失败复现。
    eprintln!(
        "randomized crr simulation seed={seed} cfg_iterations={}",
        cfg.iterations
    );

    // 初始 store 从 1 连续编号，对齐 Go harness。
    let stores = store_id_range(1, cfg.initial_stores as u64);
    let mut h = new_sim_harness(&stores, cfg.region_count);
    // 无持久状态冷启动计算器。
    h.calculator = Some(h.new_calculator(None, None));

    // 与 Go 侧确定性 PRNG 同角色（非加密）。
    let mut rng = XorShift64::new(seed as u64);
    let mut sim = RandomizedCRRSimulation::new(&mut h, &mut rng, cfg.clone());
    // safe/validated/global 三条水位分别跟踪不同断言阶段。
    let mut last_safe_checkpoint = sim.require_initial_checkpoint();
    let mut last_validated_checkpoint = last_safe_checkpoint;
    let mut last_global_checkpoint = last_safe_checkpoint;

    // 主循环：每轮随机动作，周期性检查全局进度与 catch-up。
    for round in 1..=cfg.iterations {
        sim.run_round(
            round,
            &mut last_safe_checkpoint,
            &mut last_validated_checkpoint,
        );

        // 周期性强制 flush 全部 store 并抬升 PD 全局检查点。
        if round % cfg.global_progress_check_every == 0 {
            last_global_checkpoint = sim.require_global_checkpoint_progress(last_global_checkpoint);
            eprintln!(
                "randomized crr global progress round={round} checkpoint={last_global_checkpoint} state={}",
                sim.describe_state()
            );
        }

        // 排空 pending 后必须能推进，否则判定计算器饥饿。
        if round % cfg.catch_up_every == 0 {
            sim.prune_empty_stores(true);
            sim.replicate_all_pending();
            let (checkpoint, advanced) = sim.try_compute_checkpoint(cfg.catch_up_timeout);
            assert!(
                advanced,
                "calculator failed to catch up after draining pending files, state={}",
                sim.describe_state()
            );
            assert!(checkpoint >= last_safe_checkpoint);
            sim.require_checkpoint_range_safe(last_validated_checkpoint, checkpoint);
            last_safe_checkpoint = checkpoint;
            last_validated_checkpoint = checkpoint;
            eprintln!(
                "randomized crr catch up round={round} checkpoint={checkpoint} state={}",
                sim.describe_state()
            );
        }
    }

    // 整轮结束后内部 synced_ts 必须离开 0。
    assert!(
        sim.last_state.SyncedTS > 0,
        "synced ts should advance during randomized simulation"
    );
}

#[derive(Clone)]
/// 随机模拟旋钮：迭代次数、拓扑抖动概率、超时与轮询间隔。
struct RandomizedCRRSimulationConfig {
    /// 主循环轮数。
    iterations: usize,
    /// 初始 store 数量。
    initial_stores: usize,
    /// store 数量上限（含 pending）。
    max_stores: usize,
    region_count: usize,
    /// 单 store 本轮不 flush 的概率（百分比）。
    store_no_flush_chance_percent: i32,
    /// 每轮最多跳过 flush 的 store 数。
    max_non_flush_stores_per_round: i32,
    /// 添加 store 的触发概率。
    add_store_chance_percent: i32,
    max_add_stores_per_round: i32,
    /// 移除 store 的触发概率。
    remove_store_chance_percent: i32,
    max_remove_stores_per_round: i32,
    scatter_chance_percent: i32,
    max_scatter_regions_per_round: i32,
    /// 尝试复制的概率。
    replicate_chance_percent: i32,
    /// 单轮最大复制条数。
    max_replicate_batch_per_round: i32,
    /// 复制时考虑的缓冲上限。
    max_buffered_files: i32,
    /// 重启计算器概率。
    restart_calculator_chance_percent: i32,
    /// 重启时携带 SyncedTS 的概率。
    restart_carry_synced_ts_chance_percent: i32,
    /// 空 store 连续多少轮后可被 prune。
    prune_empty_store_after_rounds: i32,
    prune_empty_store_chance_percent: i32,
    /// 每 N 轮做一次全局进度强制抬升。
    global_progress_check_every: usize,
    /// 每 N 轮做一次 catch-up。
    catch_up_every: usize,
    /// 普通计算超时。
    compute_timeout: Duration,
    /// catch-up 计算超时（更宽）。
    catch_up_timeout: Duration,
    /// 注入计算器的 PollInterval。
    calculator_poll_interval: Duration,
}

/// 单轮动作日志，用于失败时描述与水位断言。
struct RandomizedCRRRoundLog {
    /// 本轮实际 flush 的 store。
    flushed_stores: Vec<u64>,
    /// 本轮复制文件数。
    replicated_files: i32,
    /// 是否重启了计算器。
    restarted_calculator: bool,
    added_stores: Vec<u64>,
    removed_stores: Vec<u64>,
    scattered_regions: Vec<u64>,
    /// 动作执行顺序（shuffle 后）。
    action_order: Vec<&'static str>,
    /// try_compute 返回的检查点。
    checkpoint: u64,
    /// 是否成功前进。
    advanced: bool,
}

/// 模拟状态机：持有 harness/RNG/配置与 store 拓扑元数据。
struct RandomizedCRRSimulation<'a> {
    h: &'a mut SimHarness,
    rng: &'a mut XorShift64,
    cfg: RandomizedCRRSimulationConfig,
    /// 下一个可分配 store id。
    next_store_id: u64,
    /// 已分配但尚未并入 PD 的 store。
    pending_store_ids: Vec<u64>,
    /// 已在 PD 中的 ready store。
    ready_store_ids: Vec<u64>,
    /// 最近一次计算器状态快照。
    last_state: PersistentState,
    /// 各 store 连续未 flush 轮数。
    empty_store_rounds: HashMap<u64, i32>,
}

impl<'a> RandomizedCRRSimulation<'a> {
    /// 从当前 PD store 列表推导 next_store_id，并快照计算器状态。
    fn new(
        h: &'a mut SimHarness,
        rng: &'a mut XorShift64,
        cfg: RandomizedCRRSimulationConfig,
    ) -> Self {
        let mut next_store_id = 1;
        for store_id in h.pd.store_ids() {
            if store_id >= next_store_id {
                next_store_id = store_id + 1;
            }
        }
        let ready_store_ids = h.pd.store_ids();
        let last_state = h
            .calculator
            .as_ref()
            .map(|c| c.StateSnapshot())
            .unwrap_or_default();
        Self {
            h,
            rng,
            cfg,
            next_store_id,
            pending_store_ids: Vec::new(),
            ready_store_ids,
            last_state,
            empty_store_rounds: HashMap::new(),
        }
    }

    /// 要求 PD 初始全局检查点 > 0。
    fn require_initial_checkpoint(&self) -> u64 {
        let checkpoint = self.h.pd.global_checkpoint();
        // 初始 PD 检查点必须有效。
        assert!(checkpoint > 0);
        checkpoint
    }

    /// 执行一轮：打乱动作顺序，必要时校验水位前进与安全性。
    fn run_round(
        &mut self,
        round: usize,
        last_safe_checkpoint: &mut u64,
        last_validated_checkpoint: &mut u64,
    ) {
        let mut round_log = RandomizedCRRRoundLog {
            flushed_stores: Vec::new(),
            replicated_files: 0,
            restarted_calculator: false,
            added_stores: Vec::new(),
            removed_stores: Vec::new(),
            scattered_regions: Vec::new(),
            action_order: Vec::new(),
            checkpoint: 0,
            advanced: false,
        };

        // Go creates independent streams before shuffling so scheduler timing
        // cannot perturb the simulation's main random sequence.
        let mut replicate_rng = XorShift64::new(self.rng.next_u64().wrapping_add(1));
        let mut compute_rng = XorShift64::new(self.rng.next_u64().wrapping_add(1));
        // 六类动作与 Go 相同；顺序每轮 shuffle。
        let mut actions: Vec<(&str, Box<dyn FnMut(&mut Self, &mut RandomizedCRRRoundLog)>)> = vec![
            (
                // Replication and calculation intentionally overlap, as in Go.
                "replicate-and-compute",
                Box::new(move |s, log| {
                    let buffered = s.h.pending_count();
                    let count = s.sample_partial_replicate_count(buffered, &mut replicate_rng);
                    let pending = Arc::clone(&s.h.pending);
                    let downstream = s.h.downstream.clone();
                    log.restarted_calculator = s.restart_calculator_if_needed(&mut compute_rng);
                    thread::scope(|scope| {
                        let replication = scope
                            .spawn(move || replicate_pending_files(&pending, &downstream, count));
                        let (checkpoint, advanced) =
                            s.try_compute_checkpoint(s.cfg.compute_timeout);
                        log.checkpoint = checkpoint;
                        log.advanced = advanced;
                        log.replicated_files = replication.join().expect("replication thread");
                    });
                }),
            ),
            (
                // 将新 store 放入 pending，下轮全局进度检查时激活。
                "add-stores",
                Box::new(|s, log| {
                    log.added_stores = s.add_random_stores();
                }),
            ),
            (
                // 随机移除可删 store，模拟下线。
                "remove-stores",
                Box::new(|s, log| {
                    log.removed_stores = s.remove_random_stores();
                }),
            ),
            (
                "scatter-regions",
                Box::new(|s, log| {
                    log.scattered_regions = s.scatter_random_regions();
                }),
            ),
            (
                // 清理长期未 flush 的空 store。
                "prune-empty-stores",
                Box::new(|s, log| {
                    log.removed_stores.extend(s.prune_empty_stores(false));
                }),
            ),
            (
                // 随机挑选部分 store 写入新 meta/log。
                "flush-stores",
                Box::new(|s, log| {
                    log.flushed_stores = s.flush_random_stores();
                }),
            ),
        ];

        // Fisher-Yates，使用仿真 RNG。
        self.shuffle_actions(&mut actions);
        for (name, action) in &mut actions {
            round_log.action_order.push(*name);
            action(self, &mut round_log);
        }

        // 动作后刷新 last_state，供重启携带。
        self.remember_synced_ts();

        // 前进则要求单调且区间内对象均已在下游。
        if round_log.advanced {
            assert!(
                round_log.checkpoint >= *last_safe_checkpoint,
                "calculator checkpoint regressed at round {round}, state={}",
                self.describe_state()
            );
            self.require_checkpoint_range_safe(*last_validated_checkpoint, round_log.checkpoint);
            *last_safe_checkpoint = round_log.checkpoint;
            *last_validated_checkpoint = round_log.checkpoint;
            // 未前进时 validated 不得落后于 safe。
        } else {
            assert!(
                *last_validated_checkpoint >= *last_safe_checkpoint,
                "validated checkpoint fell behind safe checkpoint at round {round}, state={}",
                self.describe_state()
            );
        }
    }

    /// 原地乱序动作列表。
    fn shuffle_actions(
        &mut self,
        actions: &mut [(&str, Box<dyn FnMut(&mut Self, &mut RandomizedCRRRoundLog)>)],
    ) {
        for i in (1..actions.len()).rev() {
            let j = self.rng.int_n(i + 1);
            actions.swap(i, j);
        }
    }

    /// 随机 flush 若干已有 region 的 store；可按概率跳过部分。
    fn flush_random_stores(&mut self) -> Vec<u64> {
        let stores = self.stores_with_regions();
        if stores.is_empty() {
            return Vec::new();
        }
        // 本轮故意不 flush 的 store 数。
        let skip = self.sample_optional_count(
            self.cfg.store_no_flush_chance_percent,
            self.cfg.max_non_flush_stores_per_round,
            i32::MAX,
        );
        // 实际 flush 数 = 候选数 - 跳过数。
        let count = (stores.len() as i32 - skip).max(0) as usize;
        let selected = pick_u64_subset(&stores, count, |n| self.rng.int_n(n));
        for store_id in &selected {
            self.h.flush_store(*store_id);
        }
        selected
    }

    /// 在 max_stores 限制内追加 pending store id。
    fn add_random_stores(&mut self) -> Vec<u64> {
        // 剩余可添加配额。
        let limit = self.cfg.max_stores as i32
            - self.h.pd.store_ids().len() as i32
            - self.pending_store_ids.len() as i32;
        let count = self.sample_optional_count(
            self.cfg.add_store_chance_percent,
            self.cfg.max_add_stores_per_round,
            limit,
        );
        let mut added = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let store_id = self.next_store_id;
            self.pending_store_ids.push(store_id);
            added.push(store_id);
            self.next_store_id += 1;
        }
        added
    }

    /// 将 pending store 并入 PD，并 bootstrap flush 一次。
    fn activate_pending_stores(&mut self) -> bool {
        // 无待激活则直接返回。
        if self.pending_store_ids.is_empty() {
            return false;
        }
        let mut store_ids = self.h.pd.store_ids();
        let pending: Vec<u64> = self.pending_store_ids.drain(..).collect();
        for store_id in pending {
            store_ids.push(store_id);
            self.ready_store_ids.push(store_id);
            self.bootstrap_added_store(store_id);
        }
        store_ids.sort_unstable();
        store_ids.dedup();
        self.h.pd.set_stores(&store_ids);
        self.h.store_ids = store_ids;
        true
    }

    /// 随机移除可删 store，保留至少一个以避免空集群。
    fn remove_random_stores(&mut self) -> Vec<u64> {
        let mut remaining = self.sample_optional_count(
            self.cfg.remove_store_chance_percent,
            self.cfg.max_remove_stores_per_round,
            self.removable_store_ids().len() as i32,
        );
        if remaining == 0 {
            return Vec::new();
        }
        let mut removed = Vec::new();
        while remaining > 0 {
            let candidates = self.removable_store_ids();
            if candidates.is_empty() {
                break;
            }
            let idx = self.rng.int_n(candidates.len());
            let store_id = candidates[idx];
            let remaining_stores: Vec<u64> = self
                .current_ready_store_ids()
                .into_iter()
                .filter(|id| *id != store_id)
                .collect();
            for region_id in self.region_ids_on_store(store_id) {
                if remaining_stores.is_empty() {
                    break;
                }
                let destination = remaining_stores[self.rng.int_n(remaining_stores.len())];
                self.h.transfer_region(region_id, destination);
            }
            let mut store_ids = self.h.pd.store_ids();
            store_ids.retain(|id| *id != store_id);
            self.h.pd.set_stores(&store_ids);
            self.h.store_ids = store_ids;
            self.ready_store_ids.retain(|id| *id != store_id);
            removed.push(store_id);
            remaining -= 1;
        }
        removed
    }

    /// 按连续空轮数/概率修剪从未 flush 的 store；force 忽略概率。
    fn prune_empty_stores(&mut self, force: bool) -> Vec<u64> {
        let store_ids = self.h.pd.store_ids();
        let mut removed = Vec::new();
        let current: HashMap<u64, ()> = store_ids.iter().map(|id| (*id, ())).collect();
        for store_id in &store_ids {
            if !self.region_ids_on_store(*store_id).is_empty() {
                self.empty_store_rounds.remove(store_id);
                continue;
            }
            let rounds = self.empty_store_rounds.entry(*store_id).or_insert(0);
            *rounds += 1;
            // 未达阈值则继续累计。
            if !force && *rounds < self.cfg.prune_empty_store_after_rounds {
                continue;
            }
            // 非强制路径仍受概率门控。
            if !force && !self.roll_percent(self.cfg.prune_empty_store_chance_percent) {
                continue;
            }
            let mut next = self.h.pd.store_ids();
            next.retain(|id| id != store_id);
            // 禁止删光所有 store。
            if next.is_empty() {
                continue;
            }
            self.h.pd.set_stores(&next);
            self.h.store_ids = next;
            self.ready_store_ids.retain(|id| id != store_id);
            self.empty_store_rounds.remove(store_id);
            removed.push(*store_id);
        }
        // 清理已不在集群中的计数项。
        self.empty_store_rounds
            .retain(|id, _| current.contains_key(id));
        removed
    }

    /// Give a newly activated store one donor region, matching the Go harness.
    fn bootstrap_added_store(&mut self, store_id: u64) {
        let donors: Vec<u64> = self
            .current_ready_store_ids()
            .into_iter()
            .filter(|id| *id != store_id)
            .collect();
        if donors.is_empty() {
            return;
        }
        let donor = donors[self.rng.int_n(donors.len())];
        let regions = self.region_ids_on_store(donor);
        if regions.is_empty() {
            return;
        }
        let region_id = regions[self.rng.int_n(regions.len())];
        self.h.transfer_region(region_id, store_id);
    }

    /// Randomly transfer regions between ready stores.
    fn scatter_random_regions(&mut self) -> Vec<u64> {
        let stores = self.current_ready_store_ids();
        if stores.len() <= 1 {
            return Vec::new();
        }
        let regions = self.h.region_ids();
        let count = self.sample_optional_count(
            self.cfg.scatter_chance_percent,
            self.cfg.max_scatter_regions_per_round,
            regions.len() as i32,
        );
        let selected = pick_u64_subset(&regions, count as usize, |n| self.rng.int_n(n));
        let mut scattered = Vec::new();
        for region_id in selected {
            let current = self.h.region_store(region_id).expect("known region");
            let candidates: Vec<u64> = stores.iter().copied().filter(|id| *id != current).collect();
            if candidates.is_empty() {
                continue;
            }
            let destination = candidates[self.rng.int_n(candidates.len())];
            self.h.transfer_region(region_id, destination);
            scattered.push(region_id);
        }
        scattered
    }

    /// 按配置部分复制 pending 对象到下游。
    fn replicate_random_buffered_files(&mut self, rng: &mut XorShift64) -> i32 {
        let buffered = self.h.pending_count();
        let count = self.sample_partial_replicate_count(buffered, rng);
        if count == 0 {
            return 0;
        }
        self.h.replicate_n(count).expect("replicate")
    }

    /// 按概率重建计算器，可选 RestorePersistentState。
    fn restart_calculator_if_needed(&mut self, rng: &mut XorShift64) -> bool {
        self.remember_synced_ts();
        if !roll_percent(rng, self.cfg.restart_calculator_chance_percent) {
            return false;
        }
        // 仅在有水位且掷中概率时携带状态重启。
        let state = if self.last_state.SyncedTS > 0
            && roll_percent(rng, self.cfg.restart_carry_synced_ts_chance_percent)
        {
            Some(self.last_state.clone())
        } else {
            None
        };
        self.h.calculator = Some(self.h.new_calculator(
            Some(CheckpointCalculatorConfig {
                // 与 Go 测试任务名对齐。
                TaskName: "drr_test_task".into(),
                PollInterval: self.cfg.calculator_poll_interval,
                ..Default::default()
            }),
            state,
        ));
        true
    }

    /// 限时 ComputeNextCheckpoint；超时视为未前进，其它错误失败测试。
    fn try_compute_checkpoint(&mut self, timeout: Duration) -> (u64, bool) {
        let ctx = Context::Background();
        let (compute_ctx, _cancel) = Context::WithTimeout(&ctx, timeout);
        let calc = self.h.calculator.as_mut().expect("calculator");
        match calc.ComputeNextCheckpoint(&compute_ctx) {
            Ok(checkpoint) => (checkpoint, true),
            Err(err) => {
                // 仅接受超时；其它错误是真实缺陷。
                assert!(
                    err.to_string().contains("context deadline exceeded"),
                    "unexpected compute error: {err}"
                );
                (0, false)
            }
        }
    }

    /// 循环复制直至 pending 清空。
    fn replicate_all_pending(&mut self) {
        loop {
            let buffered = self.h.pending_count();
            if buffered == 0 {
                return;
            }
            let replicated = self.h.replicate_n(buffered).expect("replicate all");
            assert_eq!(buffered, replicated);
        }
    }

    /// 激活 pending、flush 全部、抬升 PD 全局点并要求严格大于 previous。
    fn require_global_checkpoint_progress(&mut self, previous: u64) -> u64 {
        self.activate_pending_stores();
        let expected = self.flush_all_stores_and_get_checkpoint();
        // 全局进度检查要求严格前进。
        assert!(expected > previous);
        // 把 PD 视图与 flush 结果对齐。
        self.h.pd.set_checkpoint(expected, &self.h.pd.store_ids());
        self.ready_store_ids = self.h.pd.store_ids();
        expected
    }

    /// 断言 (previous, current] 区间内每条 flush 记录的 meta/log 下游可读。
    fn require_checkpoint_range_safe(&self, previous: u64, current: u64) {
        if current <= previous {
            return;
        }
        // 逐条检查区间内 flush 的下游可读性。
        for record in self.h.records_up_to(current) {
            // 已由 previous 覆盖的记录跳过。
            if record.checkpoint_ts <= previous {
                continue;
            }
            assert!(
                self.h.downstream.exists(&record.meta_path),
                "metadata {} should be readable",
                record.meta_path
            );
            for log_path in &record.log_paths {
                assert!(
                    self.h.downstream.exists(log_path),
                    "log {log_path} should be readable"
                );
            }
        }
    }

    /// 失败信息用的紧凑状态描述。
    fn describe_state(&self) -> String {
        format!(
            "stores={:?} buffered={} global={} synced={}",
            self.h.pd.store_ids(),
            self.h.pending_count(),
            self.h.pd.global_checkpoint(),
            self.h
                .calculator
                .as_ref()
                .map(|c| c.SyncedTS())
                .unwrap_or(0)
        )
    }

    /// 返回至少 flush 过一次的 store。
    fn stores_with_regions(&self) -> Vec<u64> {
        self.h
            .pd
            .store_ids()
            .into_iter()
            .filter(|id| !self.region_ids_on_store(*id).is_empty())
            .collect()
    }

    /// flush 全部 ready store，返回新分配的检查点 ts。
    fn flush_all_stores_and_get_checkpoint(&mut self) -> u64 {
        let stores = self.stores_with_regions();
        assert!(!stores.is_empty());
        let mut checkpoint = u64::MAX;
        for store_id in stores {
            let record = self.h.flush_store(store_id);
            if record.checkpoint_ts < checkpoint {
                checkpoint = record.checkpoint_ts;
            }
        }
        assert_ne!(u64::MAX, checkpoint);
        checkpoint
    }

    /// 可移除候选：保留至少一个 ready store。
    fn removable_store_ids(&self) -> Vec<u64> {
        let store_ids = self.h.pd.store_ids();
        let ready = self.current_ready_store_ids();
        store_ids
            .into_iter()
            .filter(|store_id| {
                self.region_ids_on_store(*store_id).is_empty()
                    || ready.iter().any(|id| id != store_id)
            })
            .collect()
    }

    fn current_ready_store_ids(&self) -> Vec<u64> {
        let current = self.h.pd.store_ids();
        self.ready_store_ids
            .iter()
            .copied()
            .filter(|id| current.contains(id))
            .collect()
    }

    fn region_ids_on_store(&self, store_id: u64) -> Vec<u64> {
        self.h.region_ids_on_store(store_id)
    }

    /// 以 percent 概率采样 [0,max] 且不超过 limit。
    fn sample_optional_count(&mut self, chance_percent: i32, max_count: i32, limit: i32) -> i32 {
        if limit <= 0 || max_count <= 0 || !self.roll_percent(chance_percent) {
            return 0;
        }
        1 + self.rng.int_n(limit.min(max_count) as usize) as i32
    }

    /// 决定本轮复制多少 pending；受 batch/缓冲上限约束。
    fn sample_partial_replicate_count(&self, buffered: i32, rng: &mut XorShift64) -> i32 {
        if buffered <= 0 {
            return 0;
        }
        if self.cfg.max_buffered_files > 0 && buffered > self.cfg.max_buffered_files {
            return buffered - self.cfg.max_buffered_files;
        }
        if !roll_percent(rng, self.cfg.replicate_chance_percent) {
            return 0;
        }
        let limit = buffered.min(self.cfg.max_replicate_batch_per_round);
        if limit <= 1 {
            return limit;
        }
        1 + rng.int_n((limit - 1) as usize) as i32
    }

    fn roll_percent(&mut self, chance_percent: i32) -> bool {
        roll_percent(self.rng, chance_percent)
    }

    /// 从计算器快照刷新 last_state。
    fn remember_synced_ts(&mut self) {
        if let Some(calc) = self.h.calculator.as_ref() {
            let state = calc.StateSnapshot();
            if state.SyncedTS > self.last_state.SyncedTS
                || state.LastCheckpoint > self.last_state.LastCheckpoint
            {
                self.last_state = state;
            }
        }
    }
}

/// percent∈[0,100] 的伯努利试验。
fn roll_percent(rng: &mut XorShift64, chance_percent: i32) -> bool {
    if chance_percent <= 0 {
        return false;
    }
    if chance_percent >= 100 {
        return true;
    }
    rng.int_n(100) < chance_percent as usize
}

fn replicate_pending_files(
    pending: &Arc<Mutex<Vec<String>>>,
    downstream: &SharedDownstream,
    count: i32,
) -> i32 {
    let mut pending = pending.lock().unwrap();
    if pending.is_empty() || count <= 0 {
        return 0;
    }
    let take = (count as usize).min(pending.len());
    let batch: Vec<String> = pending.drain(..take).collect();
    drop(pending);
    for path in &batch {
        downstream.mark_exists(path);
    }
    batch.len() as i32
}

/// 从 pool 无放回抽取 count 个元素。
fn pick_u64_subset(items: &[u64], count: usize, mut int_n: impl FnMut(usize) -> usize) -> Vec<u64> {
    if count == 0 || items.is_empty() {
        return Vec::new();
    }
    let mut pool = items.to_vec();
    for i in (1..pool.len()).rev() {
        let j = int_n(i + 1);
        pool.swap(i, j);
    }
    let count = count.min(pool.len());
    pool.truncate(count);
    pool
}

/// 闭区间 store id 列表。
fn store_id_range(start: u64, end_inclusive: u64) -> Vec<u64> {
    (start..=end_inclusive).collect()
}

/// 环境变量覆盖，否则固定默认种子。
fn deterministic_seed() -> i64 {
    std::env::var("TIDB_TEST_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20260728)
}

/// 简单 xorshift64* 风格 PRNG，保证跨平台可复现。
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// seed=0 时替换为黄金比例常数，避免零状态退化。
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E3779B97F4A7C15 } else { seed },
        }
    }

    /// 推进状态并返回下一个 u64。
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// 均匀取 [0,n) ；n=0 返回 0。
    fn int_n(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() as usize) % n
    }
}

#[derive(Clone)]
/// 单次 flush 产生的检查点/路径记录，用于安全区间断言。
struct FlushRecord {
    /// 分配给该 flush 的逻辑检查点。
    checkpoint_ts: u64,
    /// meta 文件名中的 flushTS。
    flush_ts: u64,
    meta_path: String,
    log_paths: Vec<String>,
}

/// 内存仿真世界：PD、上下游、pending 队列、flush 历史与计算器。
struct SimHarness {
    pd: SharedPD,
    upstream: MemStorage,
    downstream: SharedDownstream,
    /// 尚未复制的对象路径队列。
    pending: Arc<Mutex<Vec<String>>>,
    /// 单调时间戳发生器。
    next_ts: Arc<Mutex<u64>>,
    /// 历史 flush 记录。
    records: Arc<Mutex<Vec<FlushRecord>>>,
    /// Current owner store for every simulated region.
    region_stores: Arc<Mutex<HashMap<u64, u64>>>,
    store_ids: Vec<u64>,
    calculator: Option<Calculator>,
}

/// 构造初始检查点=10 的仿真环境。
fn new_sim_harness(stores: &[u64], region_count: usize) -> SimHarness {
    // 与 Go harness 初始检查点一致。
    let initial = 10;
    let region_stores = (1..=region_count as u64)
        .map(|region_id| {
            let store_id = stores[(region_id as usize - 1) % stores.len()];
            (region_id, store_id)
        })
        .collect();
    SimHarness {
        pd: SharedPD::with_checkpoint(initial, stores),
        upstream: MemStorage::new("file:///tmp/crr-checkpoint-randomized"),
        downstream: SharedDownstream::default(),
        pending: Arc::new(Mutex::new(Vec::new())),
        next_ts: Arc::new(Mutex::new(initial)),
        records: Arc::new(Mutex::new(Vec::new())),
        region_stores: Arc::new(Mutex::new(region_stores)),
        store_ids: stores.to_vec(),
        calculator: None,
    }
}

impl SimHarness {
    fn region_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.region_stores.lock().unwrap().keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    fn region_ids_on_store(&self, store_id: u64) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .region_stores
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(region_id, owner)| (*owner == store_id).then_some(*region_id))
            .collect();
        ids.sort_unstable();
        ids
    }

    fn region_store(&self, region_id: u64) -> Option<u64> {
        self.region_stores.lock().unwrap().get(&region_id).copied()
    }

    fn transfer_region(&self, region_id: u64, store_id: u64) {
        let previous = self
            .region_stores
            .lock()
            .unwrap()
            .insert(region_id, store_id);
        assert!(previous.is_some(), "region {region_id} must exist");
    }

    /// 单调分配时间戳，步进 10，避免与 flush/checkpoint 碰撞。
    fn alloc_ts(&self) -> u64 {
        let mut guard = self.next_ts.lock().unwrap();
        // 步进 10：为 checkpoint_ts 与 flush_ts 成对分配留间隔。
        *guard += 10;
        *guard
    }

    /// 写入一对 meta/log，加入 pending 与 records。
    fn flush_store(&mut self, store_id: u64) -> FlushRecord {
        let checkpoint_ts = self.alloc_ts();
        let flush_ts = self.alloc_ts();
        let (meta_path, log_path) = write_meta(&self.upstream, flush_ts, store_id);
        // meta 与 log 都进入 pending，需一并复制。
        self.pending.lock().unwrap().push(meta_path.clone());
        self.pending.lock().unwrap().push(log_path.clone());
        let record = FlushRecord {
            checkpoint_ts,
            flush_ts,
            meta_path,
            log_paths: vec![log_path],
        };
        self.records.lock().unwrap().push(record.clone());
        record
    }

    /// 当前未复制到下游的对象数。
    fn pending_count(&self) -> i32 {
        self.pending.lock().unwrap().len() as i32
    }

    /// 将 pending 前 n 个标记为下游存在。
    fn replicate_n(&mut self, n: i32) -> Result<i32, Error> {
        Ok(replicate_pending_files(&self.pending, &self.downstream, n))
    }

    /// 返回 checkpoint_ts ≤ tso 的 flush 记录。
    fn records_up_to(&self, tso: u64) -> Vec<FlushRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.checkpoint_ts <= tso)
            .cloned()
            .collect()
    }

    /// 组装 Calculator；可选恢复 PersistentState。
    fn new_calculator(
        &self,
        cfg: Option<CheckpointCalculatorConfig>,
        state: Option<PersistentState>,
    ) -> Calculator {
        let mut cfg = cfg.unwrap_or(CheckpointCalculatorConfig {
            TaskName: "drr_test_task".into(),
            PollInterval: Duration::from_millis(1),
            ..Default::default()
        });
        // 空任务名回退默认，避免 PD 查询键为空。
        if cfg.TaskName.is_empty() {
            cfg.TaskName = "drr_test_task".into();
        }
        let mut calc = NewCalculator(
            CalculatorDeps {
                PD: Box::new(self.pd.clone()),
                Upstream: Box::new(self.upstream.clone()),
                Sync: Box::new(NewExistenceSyncChecker(self.downstream.clone())),
            },
            cfg,
            None,
        )
        .expect("calculator");
        if let Some(state) = state {
            calc.RestorePersistentState(state).expect("restore");
        }
        calc
    }
}

#[derive(Clone)]
/// 线程安全 PD 桩：可变全局检查点与 store 列表。
struct SharedPD {
    inner: Arc<SharedPDInner>,
}

struct SharedPDInner {
    checkpoint: Mutex<u64>,
    stores: Mutex<Vec<Store>>,
}

impl SharedPD {
    /// 以给定检查点与 store 集合初始化。
    fn with_checkpoint(checkpoint: u64, store_ids: &[u64]) -> Self {
        Self {
            inner: Arc::new(SharedPDInner {
                checkpoint: Mutex::new(checkpoint),
                stores: Mutex::new(
                    store_ids
                        .iter()
                        // BootAt 占位，计算器当前忽略。
                        .map(|id| Store { ID: *id, BootAt: 1 })
                        .collect(),
                ),
            }),
        }
    }

    /// 同时更新检查点与 store 视图（写两次保证可见性）。
    fn set_checkpoint(&self, checkpoint: u64, store_ids: &[u64]) {
        // 先写检查点再同步 store，最后再写一次以防中间态。
        *self.inner.checkpoint.lock().unwrap() = checkpoint;
        self.set_stores(store_ids);
        *self.inner.checkpoint.lock().unwrap() = checkpoint;
    }

    /// 替换存活 store 列表。
    fn set_stores(&self, store_ids: &[u64]) {
        *self.inner.stores.lock().unwrap() = store_ids
            .iter()
            .map(|id| Store { ID: *id, BootAt: 1 })
            .collect();
    }

    /// 返回当前 store id 快照。
    fn store_ids(&self) -> Vec<u64> {
        self.inner
            .stores
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.ID)
            .collect()
    }

    /// 当前 PD 全局检查点。
    fn global_checkpoint(&self) -> u64 {
        *self.inner.checkpoint.lock().unwrap()
    }
}

/// 供计算器读取的 PDMetaReader 实现。
impl PDMetaReader for SharedPD {
    fn GetGlobalCheckpointForTask(&self, _ctx: &Context, _task: &str) -> Result<u64, Error> {
        // 忽略 task，单任务仿真。
        Ok(self.global_checkpoint())
    }

    fn Stores(&self, _ctx: &Context) -> Result<Vec<Store>, Error> {
        // 返回克隆，避免锁跨越调用方。
        Ok(self.inner.stores.lock().unwrap().clone())
    }
}

#[derive(Clone)]
/// 内存上游存储，支持 SubDir/StartAfter。
struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
}

impl MemStorage {
    /// uri 仅用于 URI() 返回值。
    fn new(uri: &str) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.to_string(),
        }
    }

    /// 覆盖写入对象内容。
    fn write_file(&self, path: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_string(), data);
    }
}

/// Walk 按字典序，严格大于 StartAfter。
impl UpstreamStorageReader for MemStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        callback: &mut dyn FnMut(&str, i64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        // 字典序 Walk，匹配 StartAfter 语义。
        paths.sort();
        let prefix = if opt.SubDir.is_empty() {
            String::new()
        } else {
            format!("{}/", opt.SubDir.trim_end_matches('/'))
        };
        for path in paths {
            if !prefix.is_empty() && !path.starts_with(&prefix) {
                continue;
            }
            // 严格大于 StartAfter。
            if !opt.StartAfter.is_empty() && path <= opt.StartAfter {
                continue;
            }
            let size = self.files.lock().unwrap()[&path].len() as i64;
            callback(&path, size)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>, Error> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }

    fn URI(&self) -> String {
        self.uri.clone()
    }
}

#[derive(Clone, Default)]
/// 下游存在性集合：replicate 时 mark，检查时 exists。
struct SharedDownstream {
    files: Arc<Mutex<HashMap<String, bool>>>,
}

impl SharedDownstream {
    /// 标记对象已复制。
    fn mark_exists(&self, name: &str) {
        self.files.lock().unwrap().insert(name.to_string(), true);
    }

    /// 查询是否已标记存在。
    fn exists(&self, name: &str) -> bool {
        *self.files.lock().unwrap().get(name).unwrap_or(&false)
    }
}

/// 适配 ObjectSyncChecker 所需的存在性接口。
impl FileExistenceChecker for SharedDownstream {
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool, Error> {
        // 委托给内存集合。
        Ok(self.exists(name))
    }
}

/// 写入大写 hex 命名的 meta 与对应 log，返回路径对。
fn write_meta(storage: &MemStorage, flush_ts: u64, store_id: u64) -> (String, String) {
    // log 路径用小写 hex；meta 前缀用大写，贴合生产命名差异。
    let log_path = format!("v1/log/store-{store_id}/flush-{flush_ts:016x}.log");
    let meta_path = format!(
        "v1/backupmeta/{flush_ts:016X}{store_id:016X}-d{flush_ts:016X}l{flush_ts:016X}u{flush_ts:016X}.meta"
    );
    // 最小 BackupMetadata JSON，含 StoreId 与 FileGroups。
    let payload = json!({
        "StoreId": store_id,
        "FileGroups": [{
            "Path": log_path,
            "DataFilesInfo": [{"Path": log_path, "MinTs": flush_ts, "MaxTs": flush_ts}]
        }]
    });
    // 同步写入 meta 与 log 对象。
    storage.write_file(&meta_path, payload.to_string().into_bytes());
    // log 内容任意；存在性比内容更重要。
    storage.write_file(&log_path, b"log".to_vec());
    (meta_path, log_path)
}

#[allow(dead_code)]
/// 保留 thread 导入，避免未使用警告随特性门控漂移。
fn _keep_thread_import() {
    let _ = thread::spawn(|| {});
}
