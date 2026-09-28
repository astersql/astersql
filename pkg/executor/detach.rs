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

// 执行器 Detach（拆离）支持：将算子树从会话绑定上下文复制为可独立使用的静态副本。
//
// 典型场景是游标（cursor）在会话继续执行其他语句后仍需继续拉取结果。
// 拆离要求整棵子树均可拆离；任一子节点失败则整体失败（原子性），避免半拆离状态。

#![allow(non_snake_case)]

/// 可拆离执行器接口：浅拷贝自身并递归处理子节点。
///
/// An executor's shallow detach must leave the original usable even when a
/// sibling later rejects detaching.
pub trait DetachableExecutor: Sized {
    /// 浅拆离：复制本节点状态但不附带子树；不可拆离时返回 `None`。
    fn detach_shallow(&self) -> Option<Self>;
    /// 返回全部子执行器。
    fn all_children(&self) -> Vec<Self>;
    /// 替换全部子执行器（用于挂上已拆离的子树）。
    fn set_all_children(&mut self, children: Vec<Self>);
}

/// 递归拆离整棵执行器树；任一子节点失败则返回 `(None, false)`。
pub fn Detach<E: DetachableExecutor>(original: &E) -> (Option<E>, bool) {
    let Some(mut detached) = original.detach_shallow() else {
        return (None, false);
    };
    let children = original.all_children();
    let mut detached_children = Vec::with_capacity(children.len());
    // 深度优先：子树全部成功后才挂回，保证原子性
    for child in &children {
        let (detached_child, ok) = Detach(child);
        if !ok {
            return (None, false);
        }
        detached_children.push(detached_child.expect("successful detach must return an executor"));
    }
    detached.set_all_children(detached_children);
    (Some(detached), true)
}

/// 可拆离的表达式上下文：转为不依赖会话可变状态的静态副本。
pub trait DetachableExprContext: Clone {
    /// `None` means this is not a session expression context and must be kept.
    /// 返回 `None` 表示本就非会话绑定上下文，应原样保留。
    fn into_static(&self) -> Option<Self>;
}

/// DistSQL（分布式 SQL / Coprocessor 请求构建）上下文的拆离接口。
pub trait DetachableDistSqlContext: Clone {
    fn detach(&self) -> Self;
}

/// 范围（key range）构建上下文的拆离接口，依赖静态表达式上下文。
pub trait DetachableRangeContext<E>: Clone {
    fn detach(&self, expression_context: &E) -> Self;
}

/// 构建 tipb/protobuf 计划上下文的拆离接口。
pub trait DetachableBuildPbContext<E>: Clone {
    fn detach(&self, expression_context: &E) -> Self;
}

/// TableReader 执行器的上下文聚合：表达式、DistSQL、范围与 PB 构建。
#[derive(Clone)]
pub struct TableReaderExecutorContext<E, D, R, P> {
    pub expression_context: E,
    pub distsql_context: D,
    pub range_context: R,
    pub build_pb_context: P,
}

impl<E, D, R, P> TableReaderExecutorContext<E, D, R, P>
where
    E: DetachableExprContext,
    D: DetachableDistSqlContext,
    R: DetachableRangeContext<E>,
    P: DetachableBuildPbContext<E>,
{
    /// 将各子上下文拆离为静态版本；表达式无法静态化时退回整体 clone。
    pub fn Detach(&self) -> Self {
        let Some(static_expression_context) = self.expression_context.into_static() else {
            return self.clone();
        };
        Self {
            distsql_context: self.distsql_context.detach(),
            range_context: self.range_context.detach(&static_expression_context),
            build_pb_context: self.build_pb_context.detach(&static_expression_context),
            expression_context: static_expression_context,
        }
    }
}

/// IndexReader 与 TableReader 共用同一套上下文结构。
pub type IndexReaderExecutorContext<E, D, R, P> = TableReaderExecutorContext<E, D, R, P>;

/// IndexLookUp（先查索引再回表）执行器上下文。
#[derive(Clone)]
pub struct IndexLookUpExecutorContext<C> {
    pub table_reader_context: C,
}

impl<C: Clone> IndexLookUpExecutorContext<C> {
    /// Go currently computes a detached table-reader context but returns the
    /// original lookup context. Preserve that observable behavior exactly.
    /// Go 侧虽会计算拆离后的 table-reader 上下文，但最终仍返回原 lookup 上下文；
    /// 此处保持与 Go 一致的可观察行为。
    pub fn Detach(&self) -> Self {
        self.clone()
    }
}

/// 可拆离的求值（evaluation）上下文。
pub trait DetachableEvalContext: Clone {
    fn into_static(&self) -> Option<Self>;
}

/// Projection（投影）执行器上下文，持有求值上下文。
#[derive(Clone)]
pub struct ProjectionExecutorContext<E> {
    pub evaluation_context: E,
}

impl<E: DetachableEvalContext> ProjectionExecutorContext<E> {
    /// 优先转为静态求值上下文，否则 clone 原上下文。
    pub fn Detach(&self) -> Self {
        Self {
            evaluation_context: self
                .evaluation_context
                .into_static()
                .unwrap_or_else(|| self.evaluation_context.clone()),
        }
    }
}

/// Selection（过滤）与 Projection 共用上下文类型。
pub type SelectionExecutorContext<E> = ProjectionExecutorContext<E>;

/// 带上下文的 TableReader 执行器壳。
#[derive(Clone)]
pub struct TableReaderExecutor<C> {
    pub context: C,
}

impl<C: Clone> TableReaderExecutor<C> {
    /// 使用调用方提供的上下文转换函数完成拆离。
    pub fn DetachWith(&self, detach_context: impl FnOnce(&C) -> C) -> (Option<Self>, bool) {
        (
            Some(Self {
                context: detach_context(&self.context),
            }),
            true,
        )
    }
}

/// IndexReader / IndexLookUp 与 TableReader 共享同一执行器壳。
pub type IndexReaderExecutor<C> = TableReaderExecutor<C>;
pub type IndexLookUpExecutor<C> = TableReaderExecutor<C>;

/// 可选求值属性集合；非空时通常依赖会话状态，禁止拆离。
pub trait OptionalEvalProperties {
    fn is_empty(&self) -> bool;
}

/// Projection 执行器：上下文 + 所需可选属性。
#[derive(Clone)]
pub struct ProjectionExec<C, P> {
    pub context: C,
    pub required_optional_properties: P,
}

impl<C: Clone, P: OptionalEvalProperties + Clone> ProjectionExec<C, P> {
    /// 若依赖非空可选属性（如会话变量副作用）则拒绝拆离。
    pub fn DetachWith(&self, detach_context: impl FnOnce(&C) -> C) -> (Option<Self>, bool) {
        if !self.required_optional_properties.is_empty() {
            return (None, false);
        }
        (
            Some(Self {
                context: detach_context(&self.context),
                required_optional_properties: self.required_optional_properties.clone(),
            }),
            true,
        )
    }
}

/// 可拆离的过滤条件：检查其可选属性是否为空。
pub trait DetachableFilter: Clone {
    fn required_optional_properties_are_empty(&self) -> bool;
}

/// Selection 执行器：上下文 + 过滤表达式列表。
#[derive(Clone)]
pub struct SelectionExec<C, F> {
    pub context: C,
    pub filters: Vec<F>,
}

impl<C: Clone, F: DetachableFilter> SelectionExec<C, F> {
    /// 任一过滤条件依赖可选属性时拒绝拆离。
    pub fn DetachWith(&self, detach_context: impl FnOnce(&C) -> C) -> (Option<Self>, bool) {
        if self
            .filters
            .iter()
            .any(|filter| !filter.required_optional_properties_are_empty())
        {
            return (None, false);
        }
        (
            Some(Self {
                context: detach_context(&self.context),
                filters: self.filters.clone(),
            }),
            true,
        )
    }
}
