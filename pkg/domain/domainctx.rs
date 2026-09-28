// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 从会话上下文取出 Domain 的适配层。
//
// Domain 是 TiDB/AsterSQL 单实例内的领域服务容器（含 InfoSchema、DDL 等）。
// 跨 keyspace（键空间，多租户存储隔离单元）的会话故意不属于单一 Domain，
// 因此查询结果可能为 `None`。

// GetDomain gets domain from context.
// might return nil if the session is a cross keyspace one.
// GetDomain 对应 Go 的同名函数：从 ValueStoreContext 中取 Domain，并在类型不匹配时返回 None。
// pub fn GetDomain(ctx: &contextutil::ValueStoreContext) -> Option<&Domain> {
//     let v = ctx.GetDomain();
//     if let Some(domain) = v.downcast_ref::<Domain>() {
//         return Some(domain);
//     }
// Go 在跨 keyspace session 场景可能返回 nil；用 None 表示。
//     None
// }
// */
use crate::domain::Domain;
use std::sync::Arc;

/// Session contexts expose their domain indirectly because cross-keyspace
/// sessions intentionally do not belong to one domain.
///
/// 会话通过该 trait 间接暴露所属 Domain；跨 keyspace 会话可返回 `None`。
pub trait DomainContext {
    /// 返回当前会话绑定的 Domain；跨 keyspace 或未绑定时为 `None`。
    fn domain(&self) -> Option<Arc<Domain>>;
}

/// 对应 Go `GetDomain`：从实现了 [`DomainContext`] 的上下文取出 Domain。
pub fn get_domain(ctx: &dyn DomainContext) -> Option<Arc<Domain>> {
    ctx.domain()
}
