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

// 物理/辅助计划节点的初始化入口。
//
// 为 `LoadData` 与 `ImportInto` 构造和 Go `baseimpl.NewBasePlan` 等价的
// 基础计划身份。标量子查询的对应初始化由 `ScalarSubqueryEvalCtx::New`
// 完成，因为 Rust 构造器还必须取得求值所需的物理计划与信息模式。

use base_dependency as base;

use crate::{ImportInto, LoadData};

/// `LoadData` / `ImportInto` 内嵌的基础计划身份。
///
/// 这两个简单计划不实现完整物理算子接口，但仍须保留 Go `baseimpl.Plan`
/// 的上下文、类型、ID 与查询块偏移契约。
#[derive(Clone)]
pub struct InitializedPlan {
    ctx: base::ContextRef,
    tp: &'static str,
    id: i32,
    query_block_offset: i32,
}

impl std::fmt::Debug for InitializedPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InitializedPlan")
            .field("tp", &self.tp)
            .field("id", &self.id)
            .field("query_block_offset", &self.query_block_offset)
            .finish_non_exhaustive()
    }
}

impl PartialEq for InitializedPlan {
    fn eq(&self, other: &Self) -> bool {
        self.tp == other.tp
            && self.id == other.id
            && self.query_block_offset == other.query_block_offset
            && std::sync::Arc::ptr_eq(&self.ctx, &other.ctx)
    }
}

impl Eq for InitializedPlan {}

impl InitializedPlan {
    fn new(ctx: base::ContextRef, tp: &'static str, query_block_offset: i32) -> Self {
        let id = ctx.alloc_plan_id();
        Self {
            ctx,
            tp,
            id,
            query_block_offset,
        }
    }

    pub fn SCtx(&self) -> &base::ContextRef {
        &self.ctx
    }

    pub fn TP(&self) -> &'static str {
        self.tp
    }

    pub fn ID(&self) -> i32 {
        self.id
    }

    pub fn QueryBlockOffset(&self) -> i32 {
        self.query_block_offset
    }
}

impl LoadData {
    /// 构造类型为 `LoadData`、查询块偏移为零的基础计划。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.Plan = Some(InitializedPlan::new(ctx, "LoadData", 0));
        self
    }

    pub fn SCtx(&self) -> &base::ContextRef {
        self.Plan
            .as_ref()
            .expect("LoadData must be initialized")
            .SCtx()
    }

    pub fn TP(&self) -> &'static str {
        self.Plan
            .as_ref()
            .expect("LoadData must be initialized")
            .TP()
    }

    pub fn ID(&self) -> i32 {
        self.Plan
            .as_ref()
            .expect("LoadData must be initialized")
            .ID()
    }

    pub fn QueryBlockOffset(&self) -> i32 {
        self.Plan
            .as_ref()
            .expect("LoadData must be initialized")
            .QueryBlockOffset()
    }
}

impl ImportInto {
    /// 构造类型为 `ImportInto`、查询块偏移为零的基础计划。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.Plan = Some(InitializedPlan::new(ctx, "ImportInto", 0));
        self
    }

    pub fn SCtx(&self) -> &base::ContextRef {
        self.Plan
            .as_ref()
            .expect("ImportInto must be initialized")
            .SCtx()
    }

    pub fn TP(&self) -> &'static str {
        self.Plan
            .as_ref()
            .expect("ImportInto must be initialized")
            .TP()
    }

    pub fn ID(&self) -> i32 {
        self.Plan
            .as_ref()
            .expect("ImportInto must be initialized")
            .ID()
    }

    pub fn QueryBlockOffset(&self) -> i32 {
        self.Plan
            .as_ref()
            .expect("ImportInto must be initialized")
            .QueryBlockOffset()
    }
}
