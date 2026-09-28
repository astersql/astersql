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

// 全文索引解析规则的单元测试。
//
// 覆盖 WHERE/TopN/Projection/Reject 各阶段与 Go 优化器规则一致的行为：
// 单列索引绑定、脏事务拒绝、评分列改写、多谓词与非常量查询报错。

use crate::{
    FTSQueryType, FullTextIndex, FullTextIndexResolverProjection,
    FullTextIndexResolverRejectRemaining, FullTextIndexResolverTopN, FullTextIndexResolverWhere,
    FullTextPushDown, PlanKind, PlanNode, ftsMatchWordDirtyTxnErrMsg, maxFTSTopK,
};

/// 构造测试用 DataSource 节点。
fn data_source() -> PlanNode {
    PlanNode::New(
        1,
        PlanKind::DataSource {
            table: "fts_t".to_owned(),
            alias: None,
            partition_id: None,
        },
        Vec::new(),
    )
}

/// 构造带给定谓词的 Selection 节点。
fn selection(conditions: &[&str], child: PlanNode) -> PlanNode {
    PlanNode::New(
        2,
        PlanKind::Selection {
            conditions: conditions.iter().map(|value| (*value).to_owned()).collect(),
        },
        vec![child],
    )
}

/// 返回单列与双列全文索引样例，供匹配/拒绝用例共用。
fn full_text_indexes() -> Vec<FullTextIndex> {
    vec![
        FullTextIndex {
            name: "ft_title".to_owned(),
            columns: vec!["title".to_owned()],
        },
        FullTextIndex {
            name: "ft_title_body".to_owned(),
            columns: vec!["title".to_owned(), "body".to_owned()],
        },
    ]
}

#[test]
// 规则名须与 Go 优化器注册名一致。
fn resolver_names_match_the_go_optimizer_rules() {
    assert_eq!(FullTextIndexResolverWhere.Name(), "fts_resolve_index_where");
    assert_eq!(FullTextIndexResolverTopN.Name(), "fts_resolve_index_topn");
    assert_eq!(
        FullTextIndexResolverProjection.Name(),
        "fts_resolve_index_projection"
    );
    assert_eq!(
        FullTextIndexResolverRejectRemaining.Name(),
        "fts_resolve_reject_remaining"
    );
}

#[test]
// WHERE：匹配单列索引，仅移除 FTS 谓词，并记录 NoScore 下推。
fn where_resolver_matches_one_column_index_and_removes_only_fts_predicate() {
    let plan = selection(&["FTS_MATCH_WORD('hello', title)", "id > 0"], data_source());
    let (plan, changed) = FullTextIndexResolverWhere
        .Optimize(plan, &full_text_indexes(), false)
        .expect("the full-text index matches title");

    assert!(changed);
    let PlanKind::Selection { conditions } = &plan.kind else {
        panic!("the non-FTS predicate must keep the selection");
    };
    assert_eq!(conditions, &["id > 0"]);
    let push_down = FullTextPushDown(&plan.children[0]).expect("pushdown must be recorded");
    assert_eq!(push_down.index_name, "ft_title");
    assert_eq!(push_down.column_name, "title");
    assert_eq!(push_down.query_text, "hello");
    assert_eq!(push_down.query_type, FTSQueryType::NoScore);
    assert_eq!(push_down.top_k, maxFTSTopK);

    let only_fts = selection(&["fts_match_word('it''s fine', `title`)"], data_source());
    let (only_fts, changed) = FullTextIndexResolverWhere
        .Optimize(only_fts, &full_text_indexes(), false)
        .expect("quoted query text and identifiers are accepted");
    assert!(changed);
    let PlanKind::Selection { conditions } = &only_fts.kind else {
        panic!("Go keeps a root selection because it has no parent to rewrite");
    };
    assert!(conditions.is_empty());
    assert_eq!(
        FullTextPushDown(&only_fts.children[0])
            .expect("pushdown remains on the root selection's scan")
            .query_text,
        "it's fine"
    );

    let nested = PlanNode::New(
        3,
        PlanKind::Projection,
        vec![selection(
            &["FTS_MATCH_WORD('hello', title)"],
            data_source(),
        )],
    );
    let (nested, changed) = FullTextIndexResolverWhere
        .Optimize(nested, &full_text_indexes(), false)
        .expect("a non-root empty selection is spliced out of its parent");
    assert!(changed);
    assert!(matches!(
        nested.children[0].kind,
        PlanKind::DataSource { .. }
    ));
}

#[test]
// 无匹配索引或脏事务时 WHERE 解析应失败。
fn where_resolver_rejects_a_non_matching_index_and_dirty_transaction() {
    let body_match = selection(&["FTS_MATCH_WORD('hello', body)"], data_source());
    let error = FullTextIndexResolverWhere
        .Optimize(body_match, &full_text_indexes(), false)
        .expect_err("the two-column index must not match a one-column search");
    assert_eq!(
        error,
        "Full text search can only be used with a matching fulltext index"
    );

    let dirty_match = selection(&["FTS_MATCH_WORD('hello', title)"], data_source());
    let error = FullTextIndexResolverWhere
        .Optimize(dirty_match, &full_text_indexes(), true)
        .expect_err("uncommitted changes prohibit FTS");
    assert_eq!(error, ftsMatchWordDirtyTxnErrMsg);
}

#[test]
// TopN/Projection 复用 WHERE 下推，改为 WithScore 与 `_FTS_SCORE`。
fn top_n_and_projection_resolvers_reuse_the_where_pushdown() {
    let (pushed_data_source, _) = FullTextIndexResolverWhere
        .Optimize(
            selection(&["FTS_MATCH_WORD('hello', title)"], data_source()),
            &full_text_indexes(),
            false,
        )
        .expect("WHERE creates the FTS pushdown");
    let top_n = PlanNode::New(
        3,
        PlanKind::TopN {
            by_items: vec!["FTS_MATCH_WORD('hello', title) DESC".to_owned()],
            offset: u64::from(u32::MAX),
            count: 10,
        },
        vec![pushed_data_source],
    );
    let (top_n, changed) = FullTextIndexResolverTopN
        .Optimize(top_n, &full_text_indexes())
        .expect("ORDER BY matches WHERE");
    assert!(changed);
    let PlanKind::TopN { by_items, .. } = &top_n.kind else {
        unreachable!();
    };
    assert_eq!(by_items, &["_FTS_SCORE DESC"]);
    let push_down = FullTextPushDown(&top_n.children[0].children[0])
        .expect("pushdown remains on the scan below the root selection");
    assert_eq!(push_down.query_type, FTSQueryType::WithScore);
    assert_eq!(push_down.top_k, maxFTSTopK);

    let mut projection = PlanNode::New(4, PlanKind::Projection, vec![top_n]);
    projection.operator_info = "FTS_MATCH_WORD('hello', title)".to_owned();
    let (projection, changed) = FullTextIndexResolverProjection
        .Optimize(projection, &full_text_indexes())
        .expect("SELECT expression matches WHERE");
    assert!(changed);
    assert_eq!(projection.operator_info, "_FTS_SCORE");
}

#[test]
// ORDER BY 查询文本必须与 WHERE 一致。
fn top_n_resolver_rejects_a_different_query() {
    let (pushed_data_source, _) = FullTextIndexResolverWhere
        .Optimize(
            selection(&["FTS_MATCH_WORD('hello', title)"], data_source()),
            &full_text_indexes(),
            false,
        )
        .expect("WHERE creates the FTS pushdown");
    let top_n = PlanNode::New(
        3,
        PlanKind::TopN {
            by_items: vec!["FTS_MATCH_WORD('world', title) DESC".to_owned()],
            offset: 0,
            count: 10,
        },
        vec![pushed_data_source],
    );
    let error = FullTextIndexResolverTopN
        .Optimize(top_n, &full_text_indexes())
        .expect_err("ORDER BY must use the same query as WHERE");
    assert_eq!(
        error,
        "'FTS_MATCH_WORD()' in ORDER BY must match the one in WHERE"
    );
}

#[test]
// UnionScan 内或紧邻其上的 Selection 均视为脏事务形态。
fn reject_remaining_detects_both_dirty_transaction_shapes() {
    let union_scan = PlanNode::New(
        5,
        PlanKind::UnionScan {
            conditions: vec!["FTS_MATCH_WORD('hello', title)".to_owned()],
        },
        vec![data_source()],
    );
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(union_scan)
        .expect_err("FTS in UnionScan reads uncommitted changes");
    assert_eq!(error, ftsMatchWordDirtyTxnErrMsg);

    let union_scan = PlanNode::New(
        5,
        PlanKind::UnionScan {
            conditions: Vec::new(),
        },
        vec![data_source()],
    );
    let selection = selection(&["FTS_MATCH_WORD('hello', title)"], union_scan);
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(selection)
        .expect_err("selection immediately above UnionScan is also dirty");
    assert_eq!(error, ftsMatchWordDirtyTxnErrMsg);
}

#[test]
// 裸投影、表达式包裹、无 LIMIT 的 Sort 等用法错误文案对齐 Go。
fn reject_remaining_preserves_go_usage_errors() {
    let mut bare_projection = PlanNode::New(6, PlanKind::Projection, vec![data_source()]);
    bare_projection.operator_info = "FTS_MATCH_WORD('hello', title)".to_owned();
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(bare_projection)
        .expect_err("score projection requires a matching WHERE predicate");
    assert_eq!(
        error,
        "'FTS_MATCH_WORD()' in SELECT requires a matching 'FTS_MATCH_WORD()' in WHERE. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)"
    );

    let mut projection = PlanNode::New(6, PlanKind::Projection, vec![data_source()]);
    projection.operator_info = "2 * FTS_MATCH_WORD('hello', title)".to_owned();
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(projection)
        .expect_err("wrapped projection is unsupported");
    assert_eq!(
        error,
        "'FTS_MATCH_WORD()' in SELECT must not be wrapped in expressions. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)"
    );

    let mut sort = PlanNode::New(7, PlanKind::Sort, vec![data_source()]);
    sort.operator_info = "FTS_MATCH_WORD('hello', title)".to_owned();
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(sort)
        .expect_err("ORDER BY FTS without LIMIT is unsupported");
    assert_eq!(
        error,
        "Currently 'FTS_MATCH_WORD()' in ORDER BY without a LIMIT clause is not supported, try specify a very large LIMIT as a workaround"
    );
}

#[test]
// SELECT 与 WHERE 查询不一致，以及多 FTS 谓词，应由对应阶段报错。
fn projection_and_multiple_where_predicates_preserve_go_errors() {
    let (pushed_data_source, _) = FullTextIndexResolverWhere
        .Optimize(
            selection(&["FTS_MATCH_WORD('hello', title)"], data_source()),
            &full_text_indexes(),
            false,
        )
        .expect("WHERE creates pushdown");
    let mut projection = PlanNode::New(8, PlanKind::Projection, vec![pushed_data_source]);
    projection.operator_info = "FTS_MATCH_WORD('world', title)".to_owned();
    let error = FullTextIndexResolverProjection
        .Optimize(projection, &full_text_indexes())
        .expect_err("SELECT and WHERE query strings must match");
    assert_eq!(
        error,
        "'FTS_MATCH_WORD()' in SELECT must match the one in WHERE"
    );

    let (plan, changed) = FullTextIndexResolverWhere
        .Optimize(
            selection(
                &[
                    "FTS_MATCH_WORD('hello', title)",
                    "FTS_MATCH_WORD('world', title)",
                ],
                data_source(),
            ),
            &full_text_indexes(),
            false,
        )
        .expect("first predicate resolves before reject-remaining validates the second");
    assert!(changed);
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(plan)
        .expect_err("multiple FTS predicates are unsupported");
    assert_eq!(
        error,
        "Currently 'FTS_MATCH_WORD()' must be used alone. It cannot be placed inside any other function or expression as a parameter, or used multiple times. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...)"
    );
}

#[test]
// 参数化查询文本由 reject-remaining 诊断为非常量。
fn parameterized_search_text_reports_the_go_constant_error() {
    let plan = selection(&["FTS_MATCH_WORD(?, title)"], data_source());
    let (plan, changed) = FullTextIndexResolverWhere
        .Optimize(plan, &full_text_indexes(), false)
        .expect("unresolved parameter is diagnosed by reject-remaining");
    assert!(!changed);
    let error = FullTextIndexResolverRejectRemaining
        .Optimize(plan)
        .expect_err("FTS query text must be constant");
    assert_eq!(error, "match against a non-constant string");
}
