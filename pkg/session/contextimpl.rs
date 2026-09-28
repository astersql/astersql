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

// 计划上下文（Plan Context）实现。
//
// 将会话与规划器扩展上下文绑定，供优化器/执行器在编译 SQL 时读取会话状态。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::sync::Arc;

use astersql_planner_plannersession::{NewPlanCtxExtended, PlanCtxExtended, SessionContext};

/// 计划上下文实现：持有会话引用及其扩展上下文。
pub struct planContextImpl {
    /// 绑定的会话。
    pub session: Arc<dyn SessionContext>,
    /// 由会话构造的扩展计划上下文。
    pub plan_ctx_extended: PlanCtxExtended,
}

/// 根据会话构造 `planContextImpl`。
pub fn newPlanContextImpl(session: Arc<dyn SessionContext>) -> planContextImpl {
    let plan_ctx_extended = NewPlanCtxExtended(Arc::clone(&session));
    planContextImpl {
        session,
        plan_ctx_extended,
    }
}
