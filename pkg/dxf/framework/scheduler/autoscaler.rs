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
// DXF 调度侧资源估算：按数据量/CPU 推算节点数、槽位与 DistSQL 并发。
//
// `ResourceCalc` 结合 `TuneFactors.AmplifyFactor` 与索引体积比放大数据量，
// 再按基准核数与基准数据量估算 add-index / import-into 的最大节点数与所需槽位。

// limitations under the License.

use crate::schstatus::TuneFactors;

/// 估算用基准核数（与 BASE_DATA_SIZE 配套）。
pub const BASE_CORES: f64 = 8.0;
/// 基准数据量：约 200GiB，对应单节点负载参考。
pub const BASE_DATA_SIZE: f64 = 200.0 * 1024.0 * 1024.0 * 1024.0;
/// 每个并发槽位对应的数据量基准：约 25GiB。
pub const BASE_SIZE_PER_CONCURRENCY: f64 = 25.0 * 1024.0 * 1024.0 * 1024.0;
/// add-index 默认最大节点数上限。
pub const MAX_NODE_COUNT_FOR_ADD_INDEX: i32 = 30;
/// import-into 默认最大节点数上限。
pub const MAX_NODE_COUNT_FOR_IMPORT_INTO: i32 = 32;
/// DistSQL 扫描并发每核上限。
pub const MAX_DIST_SQL_CONCURRENCY_PER_CORE: i32 = 32;
/// 单节点时 DistSQL 扫描并发默认值（再乘 thread_count）。
pub const DEFAULT_DIST_SQL_SCAN_CONCURRENCY: i32 = 15;

#[derive(Clone)]
/// 按数据量与节点规格估算调度资源需求。
pub struct ResourceCalc {
    /// 原始数据字节数。
    pub data_size: i64,
    /// 单节点 CPU 核数。
    pub node_cpu: i32,
    /// 索引相对数据体积比，用于放大有效数据量。
    pub index_size_ratio: f64,
    /// 调谐因子（含 AmplifyFactor）。
    pub factors: TuneFactors,
}

impl ResourceCalc {
    /// 加索引场景：index_size_ratio 置 0。
    pub fn for_add_index(data_size: i64, node_cpu: i32, factors: &TuneFactors) -> Self {
        Self::new(data_size, node_cpu, 0.0, factors)
    }

    /// 通用构造。
    pub fn new(
        data_size: i64,
        node_cpu: i32,
        index_size_ratio: f64,
        factors: &TuneFactors,
    ) -> Self {
        Self {
            data_size,
            node_cpu,
            index_size_ratio,
            factors: factors.clone(),
        }
    }

    /// 估算 add-index 可用的最大节点数。
    pub fn max_node_count_for_add_index(&self) -> i32 {
        self.max_node_count_by_size(
            self.amplified_data_size(),
            self.factors.AmplifyFactor * f64::from(MAX_NODE_COUNT_FOR_ADD_INDEX),
        )
    }

    /// 估算 import-into 可用的最大节点数。
    pub fn max_node_count_for_import_into(&self) -> i32 {
        self.max_node_count_by_size(
            self.amplified_data_size(),
            self.factors.AmplifyFactor * f64::from(MAX_NODE_COUNT_FOR_IMPORT_INTO),
        )
    }

    /// 按放大后数据量估算所需槽位，夹在 [1, node_cpu]；非正数据量返回 4。
    pub fn required_slots(&self) -> i32 {
        let size = self.amplified_data_size();
        if size <= 0 {
            return 4;
        }
        (size as f64 / BASE_SIZE_PER_CONCURRENCY)
            .min(f64::from(self.node_cpu))
            .max(1.0)
            .round() as i32
    }

    /// AmplifyFactor * (1+index_size_ratio) * data_size。
    fn amplified_data_size(&self) -> i64 {
        (self.factors.AmplifyFactor * (1.0 + self.index_size_ratio) * self.data_size as f64) as i64
    }

    /// 按相对基准核数折算节点数，并夹在 [1, limit*baseline_ratio]。
    fn max_node_count_by_size(&self, size: i64, limit: f64) -> i32 {
        if self.node_cpu <= 0 {
            return 0;
        }
        // 核数越多，同样数据量需要的节点数越少。
        let baseline_ratio = BASE_CORES / f64::from(self.node_cpu);
        (size as f64 * baseline_ratio / BASE_DATA_SIZE)
            .min(limit * baseline_ratio)
            .max(1.0)
            .round() as i32
    }
}

/// The Go implementation asks PD for every store, then uses at least three
/// workers and otherwise one DXF node per three stores.
/// 按 PD store 数量估算：至少 3，否则约每 3 个 store 一个 DXF 节点。
pub fn calc_max_node_count_by_store_count(store_count: usize) -> i32 {
    3.max((store_count / 3) as i32)
}

/// Return the CPU count of the execution node selected for the current target
/// scope. In tests, match Go's fallback to the local CPU count when the DXF
/// service task manager has not been initialized.
#[cfg(target_os = "windows")]
pub fn GetExecCPUNode(ctx: crate::interface::Context) -> crate::interface::Result<i32> {
    let mgr = match astersql_dxf_framework_storage::GetDXFSvcTaskMgr() {
        Ok(mgr) => mgr,
        Err(_err) if astersql_util_intest::InTest => return Ok(astersql_util_cpu::GetCPUCount()),
        Err(err) => return Err(crate::interface::SchedulerError::new(err.to_string())),
    };
    let scope = astersql_dxf_framework_handle::GetTargetScope()
        .map_err(|err| crate::interface::SchedulerError::new(err.to_string()))?;
    mgr.GetCPUCountOfNodeByRole(ctx, scope)
        .map_err(|err| crate::interface::SchedulerError::new(err.to_string()))
}

/// 多节点时在默认与每核上限之间线性插值 DistSQL 并发。
pub fn calc_dist_sql_concurrency(thread_count: i32, max_node_count: i32, node_cpu: i32) -> i32 {
    // 单节点：thread_count * 默认扫描并发。
    if max_node_count <= 1 {
        return thread_count * DEFAULT_DIST_SQL_SCAN_CONCURRENCY;
    }
    let start = DEFAULT_DIST_SQL_SCAN_CONCURRENCY * node_cpu;
    let interval =
        node_cpu * (MAX_DIST_SQL_CONCURRENCY_PER_CORE - DEFAULT_DIST_SQL_SCAN_CONCURRENCY);
    let total_steps = MAX_NODE_COUNT_FOR_IMPORT_INTO - 1;
    let steps = total_steps.min(max_node_count - 1);
    (f64::from(start) + f64::from(interval) * f64::from(steps) / f64::from(total_steps)) as i32
}

// Go-compatible exported spellings used by callers ported mechanically.
/// Go 风格导出名：加索引 ResourceCalc。
pub fn NewRCCalcForAddIndex(data_size: i64, node_cpu: i32, factors: &TuneFactors) -> ResourceCalc {
    ResourceCalc::for_add_index(data_size, node_cpu, factors)
}

/// Go 风格导出名：通用 ResourceCalc。
pub fn NewRCCalc(
    data_size: i64,
    node_cpu: i32,
    index_size_ratio: f64,
    factors: &TuneFactors,
) -> ResourceCalc {
    ResourceCalc::new(data_size, node_cpu, index_size_ratio, factors)
}

/// Go 风格导出名：按 store 数估节点数。
pub fn CalcMaxNodeCountByStoresNum(store_count: usize) -> i32 {
    calc_max_node_count_by_store_count(store_count)
}

/// Go 风格导出名：DistSQL 并发估算。
pub fn CalcDistSQLConcurrency(thread_count: i32, max_node_count: i32, node_cpu: i32) -> i32 {
    calc_dist_sql_concurrency(thread_count, max_node_count, node_cpu)
}
