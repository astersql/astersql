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

// DML（数据操纵语言）AST 核心类型的单元测试。
//
// 覆盖 LIMIT、通配符字段、ORDER/GROUP BY 项、窗口 Frame、
// Join 还原，以及 `NewCrossJoin` 对右连接树的改写规则（与 Go 一致）。

use crate::dml::*;

/// 校验 Limit、WildCardField、ByItem 的 restore 文本与 Go 一致。
#[test]
fn limit_wildcard_and_by_item_restore_match_go() {
    assert_eq!(Limit::new(None, "10").restore(), "LIMIT 10");
    assert_eq!(Limit::new(Some("10"), "20").restore(), "LIMIT 10,20");
    assert_eq!(WildCardField::new(None, None).restore(), "*");
    assert_eq!(WildCardField::new(None, Some("t")).restore(), "`t`.*");
    assert_eq!(
        WildCardField::new(Some("testdb"), Some("t")).restore(),
        "`testdb`.`t`.*"
    );
    assert_eq!(ByItem::new("a", false).restore(), "`a`");
    assert_eq!(ByItem::new("a", true).restore(), "`a` DESC");
    assert_eq!(ByItem::new("NULL", false).restore(), "NULL");
}

/// 表名含分区列表与索引 Hint 时，还原顺序与转义应与 Go 一致。
#[test]
fn table_name_partitions_and_index_hints_match_go() {
    let mut table = TableName::new(Some("dbb"), "hello-world");
    table.partition_names = vec!["p0".into(), "p1".into()];
    table.index_hints = vec![
        IndexHint {
            hint_type: IndexHintType::Use,
            scope: IndexHintScope::OrderBy,
            index_names: vec!["foo`bar".into()],
        },
        IndexHint {
            hint_type: IndexHintType::Ignore,
            scope: IndexHintScope::Join,
            index_names: vec!["idx2".into()],
        },
    ];
    assert_eq!(
        table.restore(),
        "`dbb`.`hello-world` PARTITION(`p0`, `p1`) USE INDEX FOR ORDER BY (`foo``bar`) IGNORE INDEX FOR JOIN (`idx2`)"
    );
}

/// GROUP/ORDER BY 子句与窗口 Frame（ROWS/RANGE）还原关键字顺序。
#[test]
fn clauses_and_window_frames_match_go_restore_order() {
    let items = vec![ByItem::new("a", false), ByItem::new("b", true)];
    assert_eq!(
        GroupByClause::new(items.clone()).restore(),
        "GROUP BY `a`,`b` DESC"
    );
    assert_eq!(OrderByClause::new(items).restore(), "ORDER BY `a`,`b` DESC");
    assert_eq!(
        FrameClause::new(
            FrameType::Rows,
            FrameBound::current_row(),
            FrameBound::current_row()
        )
        .restore(),
        "ROWS BETWEEN CURRENT ROW AND CURRENT ROW"
    );
    assert_eq!(
        FrameClause::new(
            FrameType::Range,
            FrameBound::preceding("?"),
            FrameBound::following("?"),
        )
        .restore(),
        "RANGE BETWEEN ? PRECEDING AND ? FOLLOWING"
    );
}

/// NATURAL JOIN / USING 还原，以及 NewCrossJoin 对右连接优先级的改写。
#[test]
fn join_restore_and_cross_join_rewrite_match_go() {
    let left = ResultSet::table("t1");
    let right = ResultSet::table("t2");
    let join = Join::new(left, right, JoinType::LeftJoin)
        .natural()
        .using(vec!["b", "c"]);
    assert_eq!(
        join.restore(),
        "`t1` NATURAL LEFT JOIN `t2` USING (`b`,`c`)"
    );

    let right_tree = Join::new(
        ResultSet::table("t2"),
        ResultSet::table("t3"),
        JoinType::RightJoin,
    );
    // 无显式括号的右连接会被改写为左连接并插入交叉连接
    let rewritten = NewCrossJoin(
        ResultSet::table("t1"),
        ResultSet::Join(Box::new(right_tree)),
    );
    assert_eq!(rewritten.restore(), "(`t1` JOIN `t2`) RIGHT JOIN `t3`");

    let explicit = Join::new(
        ResultSet::table("t2"),
        ResultSet::table("t3"),
        JoinType::RightJoin,
    )
    .explicit_parens();
    let unchanged = NewCrossJoin(ResultSet::table("t1"), ResultSet::Join(Box::new(explicit)));
    assert_eq!(unchanged.restore(), "`t1` JOIN (`t2` RIGHT JOIN `t3`)");

    let nested_right = Join::new(
        ResultSet::table("t2"),
        ResultSet::table("t3"),
        JoinType::RightJoin,
    )
    .explicit_parens();
    let outer = Join::new(
        ResultSet::Join(Box::new(nested_right)),
        ResultSet::table("t4"),
        JoinType::CrossJoin,
    );
    // 嵌套右连接 + 外层交叉连接：验证最左叶插入规则
    let nested_rewritten = NewCrossJoin(ResultSet::table("t1"), ResultSet::Join(Box::new(outer)));
    assert_eq!(
        nested_rewritten.restore(),
        "((`t1` JOIN `t3`) LEFT JOIN `t2`) JOIN `t4`"
    );
}
