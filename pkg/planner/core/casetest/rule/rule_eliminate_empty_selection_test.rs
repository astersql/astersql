// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 消除空 Selection 规则 casetest。
//
// 对应 Go `rule_eliminate_empty_selection_test.go`：无谓词的 Selection 应被剥掉，
// 有条件的 Selection 则保留。Rust 侧保留 plan_tree 对照并做可执行回归。
//
// Selection：过滤算子；Empty Selection：conditions 为空、对结果无影响的冗余节点。

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

/// 对齐 Go `testkit.RunTestUnderCascades`，两种 planner 模式各用独立会话。
fn run_test_under_cascades<F>(mut test: F)
where
    F: FnMut(&mut TestKit, &str),
{
    for cascades in ["off", "on"] {
        let (store, _domain) = CreateMockStoreAndDomain();
        let mut test_kit = TestKit::new(store);
        test_kit.MustExec("use test;", Vec::new());
        test_kit.MustExec(
            &format!("set @@tidb_enable_cascades_planner = {cascades}"),
            Vec::new(),
        );
        test(&mut test_kit, cascades);
    }
}

#[test]
fn test_empty_selection_eliminator() {
    run_test_under_cascades(|test_kit, cascades| {
        for ddl in [
            "CREATE TABLE A ( col_int int(11) DEFAULT NULL, col_varchar_10 varchar(10) DEFAULT NULL, pk int(11) NOT NULL AUTO_INCREMENT, col_varchar_10_not_null varchar(10) NOT NULL, col_int_not_null int(11) NOT NULL, col_decimal decimal(10,0) DEFAULT NULL, col_datetime datetime DEFAULT NULL, col_decimal_not_null decimal(10,0) NOT NULL, col_varchar_1024 varchar(1024) DEFAULT NULL, col_datetime_not_null datetime NOT NULL, col_varchar_1024_not_null varchar(1024) NOT NULL, PRIMARY KEY (pk) ) ENGINE=InnoDB AUTO_INCREMENT=101 DEFAULT CHARSET=latin1;",
            "CREATE TABLE F ( col_datetime_not_null datetime NOT NULL, col_decimal decimal(10,0) DEFAULT NULL, col_datetime datetime DEFAULT NULL, col_varchar_10_not_null varchar(10) NOT NULL, pk int(11) NOT NULL AUTO_INCREMENT, col_int int(11) DEFAULT NULL, col_varchar_1024_not_null varchar(1024) NOT NULL, col_decimal_not_null decimal(10,0) NOT NULL, col_varchar_1024 varchar(1024) DEFAULT NULL, col_int_not_null int(11) NOT NULL, col_varchar_10 varchar(10) DEFAULT NULL, PRIMARY KEY (pk) ) ENGINE=InnoDB AUTO_INCREMENT=71 DEFAULT CHARSET=latin1;",
            "CREATE TABLE G ( col_varchar_10 varchar(10) DEFAULT NULL, col_datetime_not_null datetime NOT NULL, col_int_not_null int(11) NOT NULL, col_int int(11) DEFAULT NULL, col_varchar_1024_not_null varchar(1024) NOT NULL, col_varchar_1024 varchar(1024) DEFAULT NULL, col_decimal decimal(10,0) DEFAULT NULL, col_decimal_not_null decimal(10,0) NOT NULL, pk int(11) NOT NULL AUTO_INCREMENT, col_datetime datetime DEFAULT NULL, col_varchar_10_not_null varchar(10) NOT NULL, PRIMARY KEY (pk) ) ENGINE=InnoDB DEFAULT CHARSET=latin1;",
            "CREATE TABLE J ( col_varchar_10 varchar(10) DEFAULT NULL, col_int int(11) DEFAULT NULL, col_varchar_10_not_null varchar(10) NOT NULL, pk int(11) NOT NULL AUTO_INCREMENT, col_datetime datetime DEFAULT NULL, col_int_not_null int(11) NOT NULL, col_decimal decimal(10,0) DEFAULT NULL, col_datetime_not_null datetime NOT NULL, col_varchar_1024_not_null varchar(1024) NOT NULL, col_varchar_1024 varchar(1024) DEFAULT NULL, col_decimal_not_null decimal(10,0) NOT NULL, PRIMARY KEY (pk) ) ENGINE=InnoDB AUTO_INCREMENT=21 DEFAULT CHARSET=latin1;",
            "CREATE TABLE L ( col_decimal decimal(10,0) DEFAULT NULL, col_int_not_null int(11) NOT NULL, col_datetime_not_null datetime NOT NULL, col_decimal_not_null decimal(10,0) NOT NULL, col_datetime datetime DEFAULT NULL, col_varchar_1024_not_null varchar(1024) NOT NULL, col_varchar_10_not_null varchar(10) NOT NULL, col_int int(11) DEFAULT NULL, col_varchar_1024 varchar(1024) DEFAULT NULL, pk int(11) NOT NULL AUTO_INCREMENT, col_varchar_10 varchar(10) DEFAULT NULL, PRIMARY KEY (pk) ) ENGINE=InnoDB AUTO_INCREMENT=23 DEFAULT CHARSET=latin1;",
        ] {
            test_kit.MustExec(ddl, Vec::new());
        }

        let queries = [
            "SELECT table1.pk AS field1, table2.col_int_not_null AS field2, table1.pk AS field3, table1.col_int_not_null AS field4 FROM A AS table1 LEFT JOIN G AS table2 INNER JOIN J AS table3 INNER JOIN J AS table4 RIGHT JOIN L AS table5 ON table4.col_datetime = table5.col_datetime_not_null ON table3.col_int_not_null = table5.pk ON table2.col_datetime_not_null = table3.col_datetime_not_null ON table1.col_datetime = table2.col_datetime_not_null WHERE table1.pk = 3 HAVING (field1 != 7 OR field2 > 1) ORDER BY field1, field2, field3, field4 ASC LIMIT 2 OFFSET 7",
            "SELECT SUM(table1.pk) AS field1 FROM F AS table1 RIGHT JOIN L AS table2 ON table1.col_decimal = table2.col_decimal_not_null WHERE ((table2.pk <> 4 OR table1.pk IS NOT NULL) AND table2.pk IN (41)) HAVING field1 <> 4",
        ];
        let expected = [
            Rows(&[
                "Projection root  test.a.pk, test.g.col_int_not_null, test.a.pk, test.a.col_int_not_null",
                "└─TopN root  test.a.pk, test.g.col_int_not_null, test.a.col_int_not_null, offset:7, count:2",
                "  └─Selection root  or(ne(test.a.pk, 7), gt(test.g.col_int_not_null, 1))",
                "    └─HashJoin root  left outer join, left side:Point_Get, equal:[eq(test.a.col_datetime, test.g.col_datetime_not_null)]",
                "      ├─Point_Get(Build) root table:A handle:3",
                "      └─HashJoin(Probe) root  inner join, equal:[eq(test.j.col_datetime_not_null, test.g.col_datetime_not_null)]",
                "        ├─TableReader(Build) root  data:TableFullScan",
                "        │ └─TableFullScan cop[tikv] table:table2 keep order:false, stats:pseudo",
                "        └─HashJoin(Probe) root  inner join, equal:[eq(test.l.pk, test.j.col_int_not_null)]",
                "          ├─TableReader(Build) root  data:TableFullScan",
                "          │ └─TableFullScan cop[tikv] table:table3 keep order:false, stats:pseudo",
                "          └─HashJoin(Probe) root  right outer join, left side:TableReader, equal:[eq(test.j.col_datetime, test.l.col_datetime_not_null)]",
                "            ├─TableReader(Build) root  data:Selection",
                "            │ └─Selection cop[tikv]  not(isnull(test.j.col_datetime))",
                "            │   └─TableFullScan cop[tikv] table:table4 keep order:false, stats:pseudo",
                "            └─TableReader(Probe) root  data:TableFullScan",
                "              └─TableFullScan cop[tikv] table:table5 keep order:false, stats:pseudo",
            ]),
            Rows(&[
                "Selection root  ne(Column, 4)",
                "└─StreamAgg root  funcs:sum(Column)->Column",
                "  └─Projection root  cast(test.f.pk, decimal(10,0) BINARY)->Column",
                "    └─HashJoin root  right outer join, left side:TableReader, equal:[eq(test.f.col_decimal, test.l.col_decimal_not_null)]",
                "      ├─Point_Get(Build) root table:L handle:41",
                "      └─TableReader(Probe) root  data:Selection",
                "        └─Selection cop[tikv]  not(isnull(test.f.col_decimal))",
                "          └─TableFullScan cop[tikv] table:table1 keep order:false, stats:pseudo",
            ]),
        ];

        for (query, expected) in queries.into_iter().zip(expected) {
            test_kit
                .MustQuery(&format!("explain format = 'plan_tree' {query}"), Vec::new())
                .Check(expected);
        }
        let _ = cascades;
    });
}

/// 回归：仅无条件 Selection 被消除；带谓词的 Selection 保持不变。
#[test]
fn only_conditionless_selection_is_eliminated() {
    use astersql_planner_core::rule_eliminate_empty_selection::EmptySelectionEliminator;
    use astersql_planner_core::task::{PlanKind, PlanNode};
    // Go 规则只消除作为孩子出现的空 Selection，且 planChanged 固定为 false。
    let scan = PlanNode::new(PlanKind::TableScan);
    let empty = PlanNode::new(PlanKind::Selection).with_children(vec![scan.clone()]);
    let root = PlanNode::new(PlanKind::Projection).with_children(vec![empty]);
    let (result, changed) = EmptySelectionEliminator.Optimize(root);
    assert!(!changed);
    assert_eq!(result.children[0].kind, PlanKind::TableScan);

    // 有条件时不应改写。
    let mut guarded = PlanNode::new(PlanKind::Selection).with_children(vec![scan]);
    guarded.conditions = vec![crate::support::expr("a_gt_0", Some(0))];
    let (result, changed) = EmptySelectionEliminator.Optimize(guarded);
    assert!(!changed);
    assert_eq!(result.kind, PlanKind::Selection);
}
