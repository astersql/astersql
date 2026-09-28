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

//! CRR 测试共享类型与确定性随机源，对齐 Go `types.go`。
//! 提供任务名/物理时间常量、Region 边界与 FlushRecord 快照结构。
//! DeterministicRNG 用 FNV-1a 从 (seed, component) 派生种子，保证跨语言复现。
//! TestContext 把种子与日志串在一起，供 pd_sim/flush_sim 等组件各自取独立 RNG。
//! 本文件不含仿真逻辑，只定义数据契约；字段命名保持 Go 导出风格便于对照。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::{SystemTime, UNIX_EPOCH};

/// 默认日志备份任务名；与 Go `defaultTaskName` 一致，供 PDSim 校验任务绑定。
pub const DEFAULT_TASK_NAME: &str = "drr_test_task";
/// 任务起始物理时间（毫秒量级常量），用于构造可复现的 checkpoint 时间线。
pub const DEFAULT_TASK_START_PHYSICAL: i64 = 1_700_000_000_000;
/// Region ID 编码前缀标签字节，避免与普通 key 字节混淆。
pub const REGION_ID_TAG: u8 = b'r';

// Go-exported aliases matching the original unexported const names used in this package.
// 保留未导出风格别名，方便机械对照 Go 包内标识符。
pub(crate) const defaultTaskName: &str = DEFAULT_TASK_NAME;
pub(crate) const defaultTaskStartPhysical: i64 = DEFAULT_TASK_START_PHYSICAL;
pub(crate) const regionIDTag: u8 = REGION_ID_TAG;

/// Deterministic RNG matching Go `deterministicRNG` seed derivation (FNV-1a).
/// 确定性 RNG：同一 seed+component 必须得到同一序列，供布局/刷盘抖动复现。
pub struct DeterministicRNG {
    rng: StdRng,
}

/// 按组件名派生种子后构造 RNG；component 用于隔离 pd-sim / flush-sim 等子流。
pub fn newDeterministicRNG(seed: i64, component: &str) -> DeterministicRNG {
    DeterministicRNG {
        rng: StdRng::seed_from_u64(deriveDeterministicSeed(seed, component) as u64),
    }
}

/// 与 Go `deriveDeterministicSeed` 对齐：FNV-1a 异或后清符号位，零值回退为 1。
pub fn deriveDeterministicSeed(seed: i64, component: &str) -> i64 {
    let mut derived = (seed as u64) ^ fnv1a64(component.as_bytes());
    // 清最高位，保证结果为正 i64，与 Go int64 约束一致。
    derived &= !(1u64 << 63);
    if derived == 0 {
        derived = 1;
    }
    derived as i64
}

/// 标准 FNV-1a 64 位哈希，常数与 Go hash/fnv 一致。
fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for b in data {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

impl DeterministicRNG {
    /// 半开区间 [0, n) 的均匀整数，n 须 > 0（由调用方保证）。
    pub fn IntN(&mut self, n: usize) -> usize {
        self.rng.gen_range(0..n)
    }

    /// 与 Go `rand.Int63n` 对应的有符号范围采样。
    pub fn Int63n(&mut self, n: i64) -> i64 {
        self.rng.gen_range(0..n)
    }

    /// 闭区间 [lower, upper]；若 lower>=upper 则直接返回 lower，避免空区间 panic。
    pub fn Uint64InRange(&mut self, lower: u64, upper: u64) -> u64 {
        if lower >= upper {
            return lower;
        }
        lower + self.Int63n((upper - lower + 1) as i64) as u64
    }
}

/// TestContext groups testing-only shared state for deterministic helpers.
/// 测试上下文：固定种子 + 日志缓冲，供多仿真器共享同一可复现根种子。
pub struct TestContext {
    seed: i64,
    pub logs: Vec<String>,
}

/// 用当前 Unix 秒作种子；失败时回退 1，保证总能构造上下文。
pub fn NewTestContext() -> TestContext {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1);
    NewTestContextWithSeed(seed)
}

/// 固定种子构造；首条日志写入 `SEED:` 便于失败时回放。
pub fn NewTestContextWithSeed(seed: i64) -> TestContext {
    let mut tc = TestContext {
        seed,
        logs: Vec::new(),
    };
    tc.logs.push(format!("SEED: {seed}"));
    tc
}

impl TestContext {
    pub fn Seed(&self) -> i64 {
        self.seed
    }

    /// 按组件名派生独立 RNG，避免不同仿真器互相污染序列。
    pub fn RNG(&self, component: &str) -> DeterministicRNG {
        newDeterministicRNG(self.Seed(), component)
    }
}

/// RegionBoundary describes a static region layout for a test.
/// 静态 region 边界：左闭右开键区间 + 所属 store，用于构建 PDSim 初始布局。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionBoundary {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub StoreID: u64,
}

/// RegionState is a read-only snapshot of one simulated region.
/// 运行时 region 快照：含 epoch/checkpoint，供查询与断言，不等于可变内部状态句柄。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionState {
    pub ID: u64,
    pub Epoch: u64,
    pub StoreID: u64,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub Checkpoint: u64,
}

/// FlushRecord tracks one simulated store flush and generated files.
/// 一次 store flush 的产物记录：时间戳区间、元数据路径与日志文件列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlushRecord {
    pub Sequence: u64,
    pub StoreID: u64,
    pub RegionIDs: Vec<u64>,
    pub CheckpointTS: u64,
    pub FlushTS: u64,
    pub MinTS: u64,
    pub MaxTS: u64,
    pub MetadataPath: String,
    pub LogPaths: Vec<String>,
}

impl FlushRecord {
    /// 深拷贝记录；路径与 RegionIDs 需独立副本，避免测试间共享可变缓冲。
    pub fn clone_record(&self) -> FlushRecord {
        FlushRecord {
            Sequence: self.Sequence,
            StoreID: self.StoreID,
            RegionIDs: self.RegionIDs.clone(),
            CheckpointTS: self.CheckpointTS,
            FlushTS: self.FlushTS,
            MinTS: self.MinTS,
            MaxTS: self.MaxTS,
            MetadataPath: self.MetadataPath.clone(),
            LogPaths: self.LogPaths.clone(),
        }
    }
}
