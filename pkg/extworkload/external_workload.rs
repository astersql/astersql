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

// TiDB background-workload contract for the external workload controller.
//
// TiDB 后台工作负载与外部控制器之间的契约（`Manager` trait）。
//
// 定义 Close、角色查询，以及 GCV2 / TTL / Auto Analyze 的注册、回收与配置更新。
// 带 Context 的方法保留调用方取消与超时传递位置。

use crate::{config, context, keyspacepb};

/// Manager 各操作返回的错误类型（对应 Go `error`）。
// ManagerError 对应 Go 接口各操作返回的 error，具体实现错误类型由后续实现决定。
pub type ManagerError = Box<dyn std::error::Error + Send + Sync>;

/// 协调 TiDB 后台工作负载与外部工作负载控制器的接口。
///
/// 所有带 Context 的方法都保留调用方取消和超时的传递位置。
// Manager 对应 Go 同名接口，协调 TiDB 后台工作负载与外部工作负载控制器。
// 所有带 Context 的方法都保留调用方取消和超时的传递位置。
pub trait Manager {
    /// 释放控制器客户端及管理器持有的资源。
    // Close 释放控制器客户端及管理器持有的资源。
    fn Close(&mut self) -> Result<(), ManagerError>;

    /// 返回当前 TiDB 承担的外部工作负载角色。
    // Role 返回当前 TiDB 承担的外部工作负载角色。
    fn Role(&self) -> config::ExternalWorkloadRole;

    /// 返回绑定到当前 TiDB 的 keyspace 元数据。
    ///
    /// Go 返回指针；Rust 借用避免复制，同时用 Option 表达原指针可能为空。
    // Meta 返回绑定到当前 TiDB 的 keyspace 元数据。
    // Go 返回指针；Rust 借用避免复制，同时用 Option 表达原指针可能为空。
    fn Meta(&self) -> Option<&keyspacepb::KeyspaceMeta>;

    /// 使用初始 keyspace 级 GC 任务初始化控制器。
    // InitializeGCV2 使用初始 keyspace 级 GC 任务初始化控制器。
    fn InitializeGCV2(&mut self, context: &context::Context) -> Result<(), ManagerError>;

    /// 请求控制器终止尚未完成的全部 keyspace 级 GC 任务。
    // AbortGCV2 请求控制器终止尚未完成的全部 keyspace 级 GC 任务。
    fn AbortGCV2(&mut self, context: &context::Context) -> Result<(), ManagerError>;

    /// 上报 safePoint（安全点）对应的 keyspace 级 GC 轮次已经完成。
    // RegisterGCV2 上报 safePoint 对应的 keyspace 级 GC 轮次已经完成。
    fn RegisterGCV2(
        &mut self,
        context: &context::Context,
        safePoint: u64,
        gcLifeTime: i64,
    ) -> Result<(), ManagerError>;

    /// 上报截至 safePoint 的 keyspace 级 GC 已处理完毕。
    // RecycleGCV2 上报截至 safePoint 的 keyspace 级 GC 已处理完毕。
    fn RecycleGCV2(
        &mut self,
        context: &context::Context,
        safePoint: u64,
    ) -> Result<(), ManagerError>;

    /// 上报用户 `gc_life_time`（GC 数据保留时长）配置发生变化。
    // UpdateGCLifeTime 上报用户 gc_life_time 配置发生变化。
    fn UpdateGCLifeTime(
        &mut self,
        context: &context::Context,
        gcLifeTime: i64,
    ) -> Result<(), ManagerError>;

    /// 上报创建了 TTL 表，或已有 TTL 表发生变更。
    // RegisterTTLTask 上报创建了 TTL 表，或已有 TTL 表发生变更。
    fn RegisterTTLTask(
        &mut self,
        context: &context::Context,
        tableID: i64,
        ttlJobEnable: bool,
    ) -> Result<(), ManagerError>;

    /// 上报表已移除 TTL 属性或整个表已被删除。
    // DeleteTTLTableInfo 上报表已移除 TTL 属性或整个表已被删除。
    fn DeleteTTLTableInfo(
        &mut self,
        context: &context::Context,
        tableID: i64,
    ) -> Result<(), ManagerError>;

    /// 上报以 `completedJobCreateTime` 标识的 TTL 作业已经完成。
    // RecycleTTLTask 上报以 completedJobCreateTime 标识的 TTL 作业已经完成。
    fn RecycleTTLTask(
        &mut self,
        context: &context::Context,
        completedJobCreateTime: u64,
    ) -> Result<(), ManagerError>;

    /// 上报 `tidb_ttl_job_enable` 系统变量发生变化。
    // UpdateTTLJobEnable 上报 tidb_ttl_job_enable 系统变量发生变化。
    fn UpdateTTLJobEnable(
        &mut self,
        context: &context::Context,
        ttlJobEnable: bool,
    ) -> Result<(), ManagerError>;

    /// 上报新注册的自动分析任务。
    // RegisterAutoAnalyze 上报新注册的自动分析任务。
    fn RegisterAutoAnalyze(
        &mut self,
        context: &context::Context,
        taskID: u64,
    ) -> Result<(), ManagerError>;

    /// 上报指定自动分析任务已经完成。
    // RecycleAutoAnalyze 上报指定自动分析任务已经完成。
    fn RecycleAutoAnalyze(
        &mut self,
        context: &context::Context,
        taskID: u64,
    ) -> Result<(), ManagerError>;
}
