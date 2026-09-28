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

// 受管节点与节点资源。
//
// ManagedNode 描述框架管理的 TiDB 节点；NodeResource 是 CPU/内存/磁盘快照。
// DXF（分布式执行框架）可按百分比限额可用资源，并按任务 slot 比例切分
// 给某个 step 的 StepResource 或磁盘配额。

use super::subtask::{NewAllocatable, StepResource};
use super::task::TaskBase;
use bytesize::GIB;
use std::cmp::min;

// ManagedNode is a TiDB node that is managed by the framework.
// ManagedNode 对应 Go 中由框架管理的 TiDB 节点元信息。
pub struct ManagedNode {
    // 见 GenerateExecID；元数据表中列名为 host。
    // ID see GenerateExecID, it's named as host in the meta table.
    pub ID: String,
    // 节点角色："" 或 "background"；所有受管节点角色应一致。
    // Role of the node, either "" or "background"
    // all managed node should have the same role
    pub Role: String,
    /// 节点 CPU 核数。
    pub CPUCount: i32,
}

// NodeResource is the resource of the node.
// exported for test.
// NodeResource 对应 Go 节点资源快照，字段顺序保持一致。
pub struct NodeResource {
    /// 总 CPU（slot/核）容量。
    pub TotalCPU: i32,
    /// 总内存字节数。
    pub TotalMem: i64,
    /// 总磁盘容量（字节）。
    pub TotalDisk: u64,
}

/// 创建 NodeResource 快照。
// NewNodeResource creates a new NodeResource.
pub fn NewNodeResource(totalCPU: i32, totalMem: i64, totalDisk: u64) -> NodeResource {
    NodeResource {
        TotalCPU: totalCPU,
        TotalMem: totalMem,
        TotalDisk: totalDisk,
    }
}

impl NodeResource {
    /// 按百分比限额返回 DXF 可用资源。
    /// CPU 与内存按同一比例缩放；磁盘保持原值（全局排序无本地盘场景）。
    // LimitDXFResource returns the resource available to DXF under the given percentage limit.
    pub fn LimitDXFResource(&self, limit: i32) -> NodeResource {
        let usableCPU = getLimitedDXFCPU(self.TotalCPU, limit);
        if usableCPU == self.TotalCPU || self.TotalCPU <= 0 {
            // Go 在 CPU 不需要限额或非法时直接返回原资源，避免按比例换算内存。
            return NewNodeResource(self.TotalCPU, self.TotalMem, self.TotalDisk);
        }
        let usableMem = (usableCPU as f64 / self.TotalCPU as f64 * self.TotalMem as f64) as i64;
        // this feature is for premium based cluster, in which we are only support
        // global sort, there is no local disk. so we leave the disk as is.
        NewNodeResource(usableCPU, usableMem, self.TotalDisk)
    }

    /// 按任务 runtime slots 相对 TotalCPU 的比例，切分本 step 的 CPU/内存。
    // GetStepResource gets the step resource according to slots.
    pub fn GetStepResource(&self, task: &TaskBase) -> StepResource {
        let slots = task.GetRuntimeSlots();
        StepResource {
            CPU: NewAllocatable(slots as i64),
            // same proportion as CPU
            // Go 按 slot/TotalCPU 的比例折算内存；这里保留相同浮点换算方式。
            Mem: NewAllocatable(
                (slots as f64 / self.TotalCPU as f64 * self.TotalMem as f64) as i64,
            ),
        }
    }

    /// 按 slot 比例，在 TotalDisk 与 quotaHint 的较小值上切分任务可用磁盘。
    // GetTaskDiskResource gets available disk for a task.
    pub fn GetTaskDiskResource(&self, task: &TaskBase, quotaHint: u64) -> u64 {
        let slots = task.GetRuntimeSlots();
        let availableDisk = min(self.TotalDisk, quotaHint);
        // 原 Go 代码没有对 TotalCPU 为 0 做额外保护；Rust 实现保持这个前提。
        (slots as f64 / self.TotalCPU as f64 * availableDisk as f64) as u64
    }
}

/// 按百分比计算 DXF 可用 CPU；向上取整，且至少保留 1 核（在合法输入下）。
// getLimitedDXFCPU returns the CPU slots available to DXF under the given percentage limit.
fn getLimitedDXFCPU(totalCPU: i32, limit: i32) -> i32 {
    // limit>=100 或非法 totalCPU：不限额，直接返回原值。
    if totalCPU <= 0 || limit >= 100 {
        return totalCPU;
    }
    // 向上取整可能导致实际占比略高于给定 limit；
    // DXF 以 slot/核为资源单位，可接受。
    // use CEIL might cause the real limit to be higher than the given limit.
    // as DXF use slots or CPU cores as the unit of resource, that's acceptable.
    let usableCPU = (totalCPU as f64 * limit as f64 / 100.0).ceil() as i32;
    if usableCPU < 1 {
        return 1;
    }
    min(usableCPU, totalCPU)
}

// 仅测试使用；对应 Go 包级变量的默认大资源快照。
// NodeResourceForTest is only used for test.
// Go 这里是包级变量；保留测试默认资源的常量形状。
pub static NodeResourceForTest: NodeResource = NodeResource {
    TotalCPU: 32,
    TotalMem: 32 * GIB as i64,
    TotalDisk: 100 * GIB,
};
