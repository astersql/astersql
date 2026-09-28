// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use crate::main_test::ensure_test_env;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows};

/// Go `TestConstantPropagation` 的真实 SQL/计划回归。
#[test]
fn test_constant_propagation() {
    ensure_test_env();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = NewTestKit(store);
    testkit.MustExec("use test", Vec::new());

    // Missing Cast Expr.
    testkit.MustExec(
        r#"create table tl50cb7440 (
             col_43 decimal(30,30) not null,
             primary key (col_43) /*t![clustered_index] clustered */,
             unique key idx_12 (col_43),
             key idx_13 (col_43),
             unique key idx_14 (col_43)
           ) engine=innodb default charset=utf8 collate=utf8_bin;"#,
        Vec::new(),
    );
    testkit.MustExec(
        r#"insert into tl50cb7440 values
           (0.000000000000000000000000000000),
           (0.400000000000000000000000000000);"#,
        Vec::new(),
    );
    testkit
        .MustQuery(
            r#"with cte_8911 (col_47665) as
                 (select mid(tl50cb7440.col_43, 6, 9) as r0
                  from tl50cb7440
                  where tl50cb7440.col_43 in (0, 0)
                    and tl50cb7440.col_43 in (0))
               (select 1
                from cte_8911
                where cte_8911.col_47665!='');"#,
            Vec::new(),
        )
        .Check(Rows(&["1"]));

    // The constant should skip the pushdown checking.
    testkit.MustExec(
        r#"CREATE TABLE t373b8b5b (
             col_53 tinyint(1) NOT NULL
           ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"#,
        Vec::new(),
    );
    testkit.MustExec(
        r#"CREATE TABLE tafab9ab4 (
             col_32 json DEFAULT NULL,
             col_35 tinyint unsigned NOT NULL,
             col_36 binary(117) NOT NULL DEFAULT 'k#Vf)%G$9T6)'
           ) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_unicode_ci"#,
        Vec::new(),
    );
    testkit
        .MustQuery(
            r#"explain format='plan_tree'
               select /*+ NO_HASH_JOIN( t373b8b5b , tafab9ab4 */
                      tafab9ab4.col_32 as r0,
                      substring_index(tafab9ab4.col_36, ',', 2) as r1,
                      tafab9ab4.col_32 as r2
               from t373b8b5b
               join tafab9ab4
                 on t373b8b5b.col_53 = tafab9ab4.col_35
               where tafab9ab4.col_35 in (78, 177)
                 and t373b8b5b.col_53 between 0 and 1
               order by r0, r1, r2"#,
            Vec::new(),
        )
        .Check(vec![
            vec!["Sort", "root", "", "test.tafab9ab4.col_32, Column"],
            vec![
                "└─Projection",
                "root",
                "",
                "test.tafab9ab4.col_32, substring_index(test.tafab9ab4.col_36, ,, 2)->Column, test.tafab9ab4.col_32",
            ],
            vec![
                "",
                "",
                "└─HashJoin",
                "root  inner join, equal:[eq(test.t373b8b5b.col_53, test.tafab9ab4.col_35)]",
            ],
            vec!["", "", "", " ├─TableReader(Build) root  data:Selection"],
            vec![
                "",
                "",
                "",
                " │ └─Selection cop[tikv]  ge(test.t373b8b5b.col_53, 0), in(test.t373b8b5b.col_53, 78, 177), le(test.t373b8b5b.col_53, 1)",
            ],
            vec![
                "",
                "",
                "",
                " │   └─TableFullScan cop[tikv] table:t373b8b5b keep order:false, stats:pseudo",
            ],
            vec!["", "", "", " └─TableReader(Probe) root  data:Selection"],
            vec![
                "",
                "",
                "",
                "   └─Selection cop[tikv]  in(test.tafab9ab4.col_35, 78, 177), le(test.tafab9ab4.col_35, 1)",
            ],
            vec![
                "",
                "",
                "",
                "     └─TableFullScan cop[tikv] table:tafab9ab4 keep order:false, stats:pseudo",
            ],
        ]);

    testkit.MustExec("CREATE TABLE a1 (a int PRIMARY KEY, b int);", Vec::new());
    testkit.MustExec("CREATE TABLE a2 (a int PRIMARY KEY, b int);", Vec::new());
    for _ in 0..20 {
        testkit
            .MustQuery(
                r#"EXPLAIN FORMAT='plan_tree'
                   SELECT STRAIGHT_JOIN *
                   FROM a1
                   LEFT JOIN a2 ON a1.a = a2.a
                   WHERE a1.a IN(a2.a, a2.b);"#,
                Vec::new(),
            )
            .Check(vec![
                vec![
                    "MergeJoin",
                    "root",
                    "",
                    "inner join, left key:test.a1.a, right key:test.a2.a",
                ],
                vec!["├─TableReader(Build)", "root", "", "data:TableFullScan"],
                vec![
                    "│",
                    "└─TableFullScan",
                    "cop[tikv]",
                    "table:a2 keep order:true, stats:pseudo",
                ],
                vec!["└─TableReader(Probe)", "root", "", "data:TableFullScan"],
                vec![
                    "",
                    "",
                    "└─TableFullScan",
                    "cop[tikv] table:a1 keep order:true, stats:pseudo",
                ],
            ]);
    }
}
