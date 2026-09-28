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

// 将物理执行计划构建为可下推的 DAG（Directed Acyclic Graph，有向无环图）请求。
//
// TiDB 把部分算子下推到 TiKV/TiFlash：树形结构（`RootExecutor`）面向 TiFlash，
// 列表结构（`Executors`）面向 TiKV。本模块提供构造这两种形态及会话相关元数据的工具。

use std::collections::HashMap;
use std::fmt;

/// 除法精度增量的默认值，对应会话变量 `div_precision_increment`。
pub const DefDivPrecisionIncrement: i32 = 4;

/// 下推目标存储类型：行存 TiKV 或列存加速引擎 TiFlash。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreType {
    TiKV,
    TiFlash,
}

/// 单个下推执行器节点：可选父节点下标与序列化后的算子载荷。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Executor {
    /// 非自然顺序计划中指向父节点的下标；列表形态下用于重建父子关系。
    pub ParentIdx: Option<u32>,
    /// 算子 protobuf 序列化字节。
    pub Payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncodeType {
    /// 默认的逐行编码，对应 Go 的 `tipb.EncodeType_TypeDefault`。
    #[default]
    TypeDefault,
    /// Chunk 编码，对应 Go 的 `tipb.EncodeType_TypeChunk`。
    TypeChunk,
}

/// 下推到存储层的 DAG 请求，携带时区、执行摘要开关、除法精度等会话侧元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DAGRequest {
    pub TimeZoneName: String,
    pub TimeZoneOffset: i64,
    /// 是否收集各算子执行摘要（runtime stats）。
    pub CollectExecutionSummaries: Option<bool>,
    pub Flags: u64,
    pub DivPrecisionIncrement: Option<u32>,
    /// TiFlash 使用的树根执行器。
    pub RootExecutor: Option<Executor>,
    /// TiKV 使用的扁平执行器列表。
    pub Executors: Vec<Executor>,
    pub EncodeType: EncodeType,
}

/// 构建 protobuf 时的上下文占位；完整迁移后会承载 pushdown 相关配置。
#[derive(Clone, Debug, Default)]
pub struct BuildPBContext;

/// 构建失败时的错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuilderError(pub String);

impl fmt::Display for BuilderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BuilderError {}

/// 物理计划节点：可转为指定存储类型的 protobuf 执行器。
pub trait PhysicalPlan: Send + Sync {
    fn ToPB(
        &self,
        context: &BuildPBContext,
        store_type: StoreType,
    ) -> Result<Executor, BuilderError>;
}

/// 构造 DAG 请求所需的会话变量子集。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionVars {
    pub TimeZoneName: String,
    pub TimeZoneOffset: i64,
    pub RuntimeStatsEnabled: bool,
    pub PushDownFlags: u64,
    pub DivPrecisionIncrement: i32,
}

impl Default for SessionVars {
    fn default() -> Self {
        Self {
            TimeZoneName: "UTC".to_owned(),
            TimeZoneOffset: 0,
            RuntimeStatsEnabled: false,
            PushDownFlags: 0,
            DivPrecisionIncrement: DefDivPrecisionIncrement,
        }
    }
}

/// 会话上下文：提供会话变量、构建上下文，并设置编码类型。
pub trait SessionContext {
    fn GetSessionVars(&self) -> &SessionVars;
    fn GetBuildPBCtx(&self) -> &BuildPBContext;
    fn SetEncodeType(&self, request: &mut DAGRequest);
}

/// 为 TiFlash 构造树形下推执行器列表（通常只含根节点）。
pub fn ConstructTreeBasedDistExec(
    context: &BuildPBContext,
    plan: &dyn PhysicalPlan,
) -> Result<Vec<Executor>, BuilderError> {
    Ok(vec![plan.ToPB(context, StoreType::TiFlash)?])
}

/// 为 TiKV 按计划顺序构造扁平执行器列表。
pub fn ConstructListBasedDistExec(
    context: &BuildPBContext,
    plans: &[Box<dyn PhysicalPlan>],
) -> Result<Vec<Executor>, BuilderError> {
    let mut executors = Vec::with_capacity(plans.len());
    for plan in plans {
        executors.push(plan.ToPB(context, StoreType::TiKV)?);
    }
    Ok(executors)
}

/// 在列表形态上补充非自然父子顺序：`unNatureOrders` 映射 child→parent 下标。
pub fn ConstructListBasedDistExecForUnNatureOrderPlans(
    context: &BuildPBContext,
    plans: &[Box<dyn PhysicalPlan>],
    unNatureOrders: &HashMap<usize, usize>,
) -> Result<Vec<Executor>, BuilderError> {
    let mut executors = ConstructListBasedDistExec(context, plans)?;
    for (child_index, parent_index) in unNatureOrders {
        executors[*child_index].ParentIdx = Some(*parent_index as u32);
    }
    Ok(executors)
}

/// 根据存储类型组装完整 `DAGRequest`：TiFlash 填 `RootExecutor`，TiKV 填 `Executors`。
pub fn ConstructDAGReq(
    context: &dyn SessionContext,
    plans: &[Box<dyn PhysicalPlan>],
    store_type: StoreType,
) -> Result<DAGRequest, BuilderError> {
    let session_vars = context.GetSessionVars();
    let mut request = DAGRequest {
        TimeZoneName: session_vars.TimeZoneName.clone(),
        TimeZoneOffset: session_vars.TimeZoneOffset,
        CollectExecutionSummaries: session_vars.RuntimeStatsEnabled.then_some(true),
        Flags: session_vars.PushDownFlags,
        // 仅在非默认除法精度时显式下发，避免冗余字段。
        DivPrecisionIncrement: (session_vars.DivPrecisionIncrement != DefDivPrecisionIncrement)
            .then_some(session_vars.DivPrecisionIncrement as u32),
        ..DAGRequest::default()
    };

    let build_result = if store_type == StoreType::TiFlash {
        match ConstructTreeBasedDistExec(context.GetBuildPBCtx(), plans[0].as_ref()) {
            Ok(mut executors) => {
                request.RootExecutor = Some(executors.remove(0));
                Ok(())
            }
            Err(error) => Err(error),
        }
    } else {
        match ConstructListBasedDistExec(context.GetBuildPBCtx(), plans) {
            Ok(executors) => {
                request.Executors = executors;
                Ok(())
            }
            Err(error) => Err(error),
        }
    };

    // Go invokes distsql.SetEncodeType after either executor-building branch,
    // including when ToPB returns an error. Keep that observable side effect.
    context.SetEncodeType(&mut request);
    build_result?;
    Ok(request)
}

/// 在已构造的 DAG 请求上写入非自然顺序的 `ParentIdx`（仅影响列表形态）。
pub fn ConstructDAGReqForUnNatureOrderPlans(
    context: &dyn SessionContext,
    plans: &[Box<dyn PhysicalPlan>],
    unNatureOrders: &HashMap<usize, usize>,
    store_type: StoreType,
) -> Result<DAGRequest, BuilderError> {
    let mut request = ConstructDAGReq(context, plans, store_type)?;
    for (child_index, parent_index) in unNatureOrders {
        request.Executors[*child_index].ParentIdx = Some(*parent_index as u32);
    }
    Ok(request)
}
