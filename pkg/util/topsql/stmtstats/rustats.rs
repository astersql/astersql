// Copyright 2026 PingCAP, Inc.
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

// TopSQL / TopRU 的 RU（Request Unit，请求单元）统计基础类型。
//
// RU 是资源组控制器用来衡量语句消耗的计量单位；本模块定义版本号、聚合键、
// 执行上下文以及增量合并结构，供 stmtstats 在语句起止时累计资源用量。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::BinaryDigest;
use crate::execdetails::{RUDetails, RUV2Metrics, RUV2Weights};

/// The wire values used by PD's resource-group controller.
/// PD（Placement Driver）资源组控制器使用的 RU 协议版本线网取值。
pub type RUVersion = i32;
/// RU 协议 v1。
pub const RU_VERSION_V1: RUVersion = 1;
/// RU 协议 v2（含 TiKV/TiFlash 细分权重）。
pub const RU_VERSION_V2: RUVersion = 2;
/// 未显式指定时使用的默认 RU 版本。
pub const DEFAULT_RU_VERSION: RUVersion = RU_VERSION_V1;

/// 提供当前 RU 协议版本的抽象；集群侧可动态切换。
pub trait RUVersionProvider: Send + Sync {
    /// 返回当前生效的 RU 版本。
    fn GetRUVersion(&self) -> RUVersion;
}

/// 返回默认 RU 版本。
pub fn DefaultRUVersion() -> RUVersion {
    DEFAULT_RU_VERSION
}

/// 将 0（未设置）规范为默认版本，其余原样返回。
pub fn NormalizeRUVersion(version: RUVersion) -> RUVersion {
    if version == 0 {
        DEFAULT_RU_VERSION
    } else {
        version
    }
}

/// 线程安全共享的 RU 明细句柄（读写锁保护）。
pub type SharedRUDetails = Arc<RwLock<RUDetails>>;

/// 按用户 + SQL digest + Plan digest 唯一标识一条 RU 统计键。
/// digest 是规范化 SQL/执行计划的哈希指纹，用于聚合同类语句。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RUKey {
    /// 用户名。
    pub User: String,
    /// 规范化 SQL 的二进制 digest。
    pub SQLDigest: BinaryDigest,
    /// 执行计划（query plan）的二进制 digest。
    pub PlanDigest: BinaryDigest,
}

impl RUKey {
    /// 由用户与 SQL/Plan digest 字节构造聚合键。
    pub fn new(user: impl Into<String>, sql_digest: &[u8], plan_digest: &[u8]) -> Self {
        Self {
            User: user.into(),
            SQLDigest: BinaryDigest::from(sql_digest),
            PlanDigest: BinaryDigest::from(plan_digest),
        }
    }
}

/// 单次语句执行期间的 RU 采样上下文：明细、权重、上次总量与协议版本。
pub struct ExecutionContext {
    /// 可变 RU 明细（读写侧可能持续累加）。
    pub RUDetails: Option<SharedRUDetails>,
    /// v2 指标计算器。
    pub RUV2Metrics: Option<Arc<RUV2Metrics>>,
    /// 本语句对应的 RU 聚合键。
    pub Key: RUKey,
    /// v2 权重配置。
    pub RUV2Weights: RUV2Weights,
    /// 上次采样时的 RU 总量，用于计算增量 delta。
    pub LastRUTotal: f64,
    /// 本语句使用的 RU 协议版本。
    pub RUVersion: RUVersion,
}

/// 某聚合键上的 RU 增量：总量、执行次数与执行时长。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RUIncrement {
    /// 累计 RU。
    pub TotalRU: f64,
    /// 执行次数。
    pub ExecCount: u64,
    /// 执行时长（纳秒累计）。
    pub ExecDuration: u64,
}

impl RUIncrement {
    /// 将另一增量合并进自身（字段累加）。
    pub fn Merge(&mut self, other: &RUIncrement) {
        self.TotalRU += other.TotalRU;
        self.ExecCount += other.ExecCount;
        self.ExecDuration += other.ExecDuration;
    }
}

/// 按 `RUKey` 索引的 RU 增量映射。
pub type RUIncrementMap = HashMap<RUKey, RUIncrement>;

/// 为 `RUIncrementMap` 提供按键合并能力。
pub trait RUIncrementMapMerge {
    /// 合并另一张增量表：同键累加，异键插入。
    fn Merge(&mut self, other: RUIncrementMap);
}

impl RUIncrementMapMerge for RUIncrementMap {
    fn Merge(&mut self, other: RUIncrementMap) {
        // 按键归并；缺失键用 Default 新建后再 Merge。
        for (key, increment) in other {
            self.entry(key).or_default().Merge(&increment);
        }
    }
}
