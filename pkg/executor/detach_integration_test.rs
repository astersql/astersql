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

// Detach（拆离）能力的集成测试。
//
// 前半为大段保留的 Go TestKit 语义注释（游标、IndexReader/IndexLookUp、
// Selection/Projection 与会话并发等）；底部可执行用例验证
// `TableReaderExecutorContext` 拆离后不再绑定真实会话。

use crate::detach::{
    DetachableBuildPbContext, DetachableDistSqlContext, DetachableExprContext,
    DetachableRangeContext, TableReaderExecutorContext,
};

/// 测试用表达式上下文：`session_bound` 表示仍依赖会话。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ExprContext {
    session_bound: bool,
}

impl DetachableExprContext for ExprContext {
    /// 会话绑定上下文可转为静态副本；否则返回 `None`。
    fn into_static(&self) -> Option<Self> {
        self.session_bound.then(|| Self {
            session_bound: false,
        })
    }
}

/// 测试用 DistSQL 上下文：持有可选的会话连接 ID。
#[derive(Clone, Debug, Eq, PartialEq)]
struct DistSqlContext {
    session_id: Option<u64>,
}

impl DetachableDistSqlContext for DistSqlContext {
    /// 拆离后清除会话 ID。
    fn detach(&self) -> Self {
        Self { session_id: None }
    }
}

/// 测试用范围上下文：标记是否已基于静态表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
struct RangeContext {
    static_expression: bool,
}

impl DetachableRangeContext<ExprContext> for RangeContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self {
            static_expression: !expression_context.session_bound,
        }
    }
}

/// 测试用 PB 构建上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildContext {
    static_expression: bool,
}

impl DetachableBuildPbContext<ExprContext> for BuildContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self {
            static_expression: !expression_context.session_bound,
        }
    }
}

/// 在真实 TestKit 会话上拆离 TableReader 上下文，关闭会话后副本仍可用。
#[test]
fn detached_reader_context_survives_its_real_sql_session() {
    let store = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
    let mut testkit = astersql_testkit::TestKit::new(store.clone());
    // 写入会话侧 KV，确认 TestKit 会话可用
    testkit.MustExec(
        "insert into aster_session_kv(k, v) values ('detach', 'ready')",
        Vec::new(),
    );
    testkit
        .MustQuery(
            "select v from aster_session_kv where k = 'detach'",
            Vec::new(),
        )
        .Check(vec![vec!["ready".to_owned()]]);

    // 构造仍绑定会话的 TableReader 上下文并拆离
    let context = TableReaderExecutorContext {
        expression_context: ExprContext {
            session_bound: true,
        },
        distsql_context: DistSqlContext {
            session_id: Some(testkit.ConnectionID()),
        },
        range_context: RangeContext {
            static_expression: false,
        },
        build_pb_context: BuildContext {
            static_expression: false,
        },
    };
    let detached = context.Detach();
    assert!(!detached.expression_context.session_bound);
    assert_eq!(detached.distsql_context.session_id, None);
    assert!(detached.range_context.static_expression);
    assert!(detached.build_pb_context.static_expression);

    // 关闭底层 store 后，拆离副本仍应保持静态字段
    astersql_testkit::Database::close(store.as_ref()).unwrap();
    assert_eq!(detached.distsql_context.session_id, None);
}
