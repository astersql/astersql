// Copyright 2026 AsterSQL.
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

// 全局内存仲裁器（global arbitrator）及相关探针的单元测试。
//
// 覆盖软限制解析、运行时内存状态落盘/回读、meminfo/memstats 探测失败回退，
// 以及 ResourcePool 预算申请与释放是否与 Go 语义对齐。

use super::arbitrator::{ArbitratorRuntimeStats, DefMaxLimit};
use super::global_arbitrator::{
    CleanupGlobalMemArbitratorForTest, GlobalMemArbitrator, GlobalMemArbitratorMetrics,
    HandleGlobalMemArbitratorRuntime, RuntimeMemStateRecorder, SetGlobalMemArbitratorSoftLimit,
    SetGlobalMemArbitratorWorkMode, SetRuntimeMemStatsSamplerForTest,
    SetupGlobalMemArbitratorForTest, SoftLimitMode, WorkMode, parse_soft_limit,
};
use super::meminfo::get_mem_total_ignore_err_with;
use super::memstats::{ForceReadMemStats, ReadMemStats};
use super::pool::NewResourcePoolDefault;

/// 校验 `parse_soft_limit`：`"0"`/`"auto"`/字节数/比例/非法值，以及 WorkMode 文本解析。
#[test]
fn soft_limit_parsing_matches_go_fallbacks() {
    assert_eq!(parse_soft_limit("0"), (0, 0.0, SoftLimitMode::Disable));
    assert_eq!(parse_soft_limit("auto"), (0, 0.0, SoftLimitMode::Auto));
    assert_eq!(
        parse_soft_limit("4096"),
        (4096, 0.0, SoftLimitMode::Specified)
    );
    assert_eq!(parse_soft_limit("1"), (0, 1.0, SoftLimitMode::Specified));
    assert_eq!(
        parse_soft_limit("0.75"),
        (0, 0.75, SoftLimitMode::Specified)
    );
    assert_eq!(parse_soft_limit("bad"), (0, 0.0, SoftLimitMode::Disable));
    assert_eq!(WorkMode::from_text("priority"), WorkMode::Priority);
    assert_eq!(WorkMode::from_text("unknown"), WorkMode::Disable);
}

/// 校验 RuntimeMemStateRecorder：空目录无状态、JSON 往返，且忽略其它版本文件名。
#[test]
fn state_recorder_round_trips_and_ignores_other_versions() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(dir.path());
    assert_eq!(recorder.load().unwrap(), None);
    recorder
        .store(&serde_json::json!({"heap_alloc": 42}))
        .unwrap();
    assert_eq!(recorder.load().unwrap().unwrap()["heap_alloc"], 42);
    std::fs::write(dir.path().join("mem-state.v2.json"), b"{}").unwrap();
    assert_eq!(recorder.load().unwrap().unwrap()["heap_alloc"], 42);
}

/// 校验 meminfo 探测失败时 `get_mem_total_ignore_err_with` 回退为 0。
#[test]
fn memory_probe_errors_fall_back_to_zero() {
    assert_eq!(
        get_mem_total_ignore_err_with(|| Ok::<_, std::io::Error>(123)),
        123
    );
    assert_eq!(
        get_mem_total_ignore_err_with(|| Err(std::io::Error::other("probe failed"))),
        0
    );
}

/// 校验强制刷新与缓存读取的 MemStats 一致且 heap_inuse 为正。
#[test]
fn memstats_cache_can_be_forced_and_read() {
    let forced = ForceReadMemStats();
    let cached = ReadMemStats();
    assert!(forced.heap_inuse > 0);
    assert_eq!(cached, forced);
}

/// 校验 ResourcePool：预算增长触顶报错、Clear/ResizeTo 释放后 Allocated 与 Go 对齐。
#[test]
fn resource_pool_enforces_limit_and_releases_budget() {
    let mut pool = NewResourcePoolDefault("root".to_owned(), 1);
    pool.Start(None, 100);
    let mut first = pool.CreateBudget();
    let mut second = pool.CreateBudget();
    first.Grow(40).unwrap();
    second.Grow(60).unwrap();
    assert_eq!(pool.Allocated(), 100);
    assert!(first.Grow(1).is_err());
    first.Clear();
    second.ResizeTo(20).unwrap();
    // Go Budget.Shrink retains one allocation-alignment block for reuse.
    assert_eq!(pool.Allocated(), 21);
    second.Clear();
    assert_eq!(pool.Stop(), 0);
}

#[test]
fn global_runtime_hook_samples_the_shared_core_arbitrator() {
    let dir = tempfile::tempdir().unwrap();
    SetupGlobalMemArbitratorForTest(dir.path().display().to_string());
    assert!(SetGlobalMemArbitratorWorkMode("standard".to_owned()));
    SetRuntimeMemStatsSamplerForTest(|| ArbitratorRuntimeStats {
        heap_alloc: DefMaxLimit,
        heap_inuse: DefMaxLimit,
        ..ArbitratorRuntimeStats::default()
    });
    HandleGlobalMemArbitratorRuntime();
    let arbitrator = GlobalMemArbitrator().unwrap();
    assert!(arbitrator.AtMemRisk());
    assert!(arbitrator.AtOOMRisk());
    assert_eq!(GlobalMemArbitratorMetrics().runtime_updates, 1);

    assert!(arbitrator.allocate(1_000));
    SetGlobalMemArbitratorSoftLimit("auto".to_owned());
    HandleGlobalMemArbitratorRuntime();
    assert_eq!(GlobalMemArbitratorMetrics().record_success, 1);
    let recorded = RuntimeMemStateRecorder::new(dir.path())
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(recorded["version"], 1);
    assert_eq!(recorded["last-risk"]["quota"], 1_000);
    CleanupGlobalMemArbitratorForTest();
}
