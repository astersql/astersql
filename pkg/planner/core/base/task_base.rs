// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 物理优化阶段的任务（Task）抽象。
//
// Task 把物理计划挂到某种执行形态上（如 TiDB 本地 RootTask、TiKV Coprocessor 的
// CopTask、或 MPP fragment），并携带成本与估算行数，供物理优化器比较候选路径。
// MPP（Massively Parallel Processing）指跨节点并行执行的分布式算子流水线。

// 新方法应追加在接口末尾，便于其它包中的实现者按 Go 文件顺序定位。

/// 对应 Go 的 `Task`，是 PhysicalPlanInfo 的新版抽象，可表示 CopTask、RootTask、MPPTaskMeta
/// 或 ParallelTask，并随计划保存成本和估算行数信息。
pub trait Task {
    /// 返回当前任务的估算行数。
    fn count(&self) -> f64;

    /// 浅拷贝任务；返回对象仍与原任务共享同一个物理计划指针语义。
    fn copy(&self) -> Box<dyn Task>;

    /// 返回当前任务承载的物理计划。
    fn plan(&self) -> &dyn crate::PhysicalPlan;

    /// 判断任务是否已经失效，失效任务不得继续参与候选成本比较。
    fn invalid(&self) -> bool;

    /// 把当前任务转换为 root task。
    /// 返回 trait object 对应 Go 用接口返回值规避 import cycle 的设计，本抽象层不依赖具体 RootTask。
    fn convert_to_root_task(&self, ctx: crate::ContextRef) -> Box<dyn Task>;

    /// 返回当前任务持有数据结构的内存字节数。
    fn memory_usage(&self) -> i64;

    /// 追加优化过程中产生的非致命告警；告警由任务携带到后续计划选择或展示阶段。
    fn append_warning(&mut self, error: crate::Error);

    /// 返回 MPP 任务向父 Join 传播的分区布局。
    fn mpp_partition_type(&self) -> property::MPPPartitionType {
        property::AnyType
    }

    /// 返回 MPP 任务向父 Join 传播的 Hash 列。
    fn mpp_hash_cols(&self) -> Vec<property::MPPPartitionColumn> {
        Vec::new()
    }

    /// 记录当前任务的 MPP 分区布局。
    fn set_mpp_partition(
        &mut self,
        _partition_type: property::MPPPartitionType,
        _hash_cols: Vec<property::MPPPartitionColumn>,
    ) {
    }
    /// 可变访问当前任务承载的物理计划。
    fn plan_mut(&mut self) -> &mut dyn crate::PhysicalPlan;
}

/// 对应 Go 的包级 `InvalidTask`。它由 core 包的空 RootTask 初始化为公共无效单例；
// / Rust 保留延迟赋值槽位，读写全局可变状态时必须由后续接线层提供同步和安全边界。
pub static mut INVALID_TASK: Option<Box<dyn Task>> = None;

/// 对应 Go 的 `MPPSink`，表示向父 fragment 发送数据的物理算子，例如 ExchangeSender。
pub trait MPPSink: crate::PhysicalPlan {
    /// 返回 exchange 数据的压缩模式。
    fn get_compression_mode(&self) -> vardef::ExchangeCompressionMode;

    /// 返回当前 fragment 自身包含的 MPP 任务。
    fn get_self_tasks(&self) -> &[kv::MPPTask];

    /// 整体替换当前 fragment 的任务列表；Vec 保留 Go slice 的顺序与批量设置语义。
    fn set_self_tasks(&mut self, tasks: Vec<kv::MPPTask>);

    /// 整体替换接收该 sink 数据的目标任务列表。
    fn set_target_tasks(&mut self, tasks: Vec<kv::MPPTask>);

    /// 按输入顺序把任务追加到现有目标列表末尾，不覆盖已设置的目标。
    fn append_target_tasks(&mut self, tasks: Vec<kv::MPPTask>);
}
