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

//! CommonHandle secondary-index ordering parity tests.

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

fn new_testkit(cascades: bool) -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        &format!(
            "set @@tidb_enable_cascades_planner = {}",
            if cascades { "on" } else { "off" }
        ),
        Vec::new(),
    );
    tk
}

/// Go `explainHas`: scan every EXPLAIN cell for the requested fragment.
fn explain_has(rows: &[Vec<String>], fragment: &str) -> bool {
    rows.iter().flatten().any(|cell| cell.contains(fragment))
}

fn assert_ordered_without_topn(tk: &TestKit, sql: &str, case: &str) {
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    assert!(
        explain_has(&rows, "keep order:true"),
        "{case}: expected keep order:true, plan={rows:?}"
    );
    assert!(
        !explain_has(&rows, "TopN"),
        "{case}: unexpected TopN, plan={rows:?}"
    );
}

fn assert_not_ordered_without_sort(tk: &TestKit, sql: &str, case: &str) {
    let rows = tk.MustQuery(sql, Vec::new()).Rows();
    let keep_order = explain_has(&rows, "keep order:true");
    let has_sort = explain_has(&rows, "TopN") || explain_has(&rows, "Sort");
    assert!(
        !keep_order || has_sort,
        "{case}: index order must not satisfy ORDER BY alone, plan={rows:?}"
    );
}

/// Executable equivalent of Go `TestCommonHandleIndexOrdering` in both planner modes.
#[test]
fn test_common_handle_index_ordering() {
    for cascades in [false, true] {
        let mut tk = new_testkit(cascades);
        tk.MustExec("drop table if exists t_ch", Vec::new());
        tk.MustExec("create table t_ch(a1 varchar(64),a2 int,b int,c int,d int,primary key(a1,a2) clustered,key ic(d),key ic_overlap(d,a1),key ic_multi(b,d),unique key uic(c))", Vec::new());
        tk.MustExec(
            "insert into t_ch values ('x',1,10,1,0),('y',2,20,2,0),('a',3,10,3,0),('m',4,30,4,1)",
            Vec::new(),
        );

        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(ic) where d=0 order by a1,a2 limit 100",
            "case 1",
        );
        tk.MustQuery(
            "select * from t_ch use index(ic) where d=0 order by a1,a2 limit 100",
            Vec::new(),
        )
        .Check(Rows(&["a 3 10 3 0", "x 1 10 1 0", "y 2 20 2 0"]));

        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(ic_overlap) where d=0 order by a1,a2 limit 100",
            "case 2",
        );
        tk.MustQuery(
            "select * from t_ch use index(ic_overlap) where d=0 order by a1,a2 limit 100",
            Vec::new(),
        )
        .Check(Rows(&["a 3 10 3 0", "x 1 10 1 0", "y 2 20 2 0"]));

        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(ic_multi) where b=10 and d=0 order by a1,a2 limit 100",
            "case 3",
        );
        tk.MustQuery(
            "select * from t_ch use index(ic_multi) where b=10 and d=0 order by a1,a2 limit 100",
            Vec::new(),
        )
        .Check(Rows(&["a 3 10 3 0", "x 1 10 1 0"]));

        assert_not_ordered_without_sort(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(uic) where c>0 order by a1,a2 limit 100",
            "case 4",
        );

        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(ic) where d=0 order by a1 desc,a2 desc limit 100",
            "case 5",
        );
        tk.MustQuery(
            "select * from t_ch use index(ic) where d=0 order by a1 desc,a2 desc limit 100",
            Vec::new(),
        )
        .Check(Rows(&["y 2 20 2 0", "x 1 10 1 0", "a 3 10 3 0"]));

        assert_not_ordered_without_sort(
            &tk,
            "explain format='plan_tree' select * from t_ch use index(ic) where d=0 order by a1 asc,a2 desc limit 100",
            "case 6",
        );

        tk.MustExec("drop table if exists t_prefix", Vec::new());
        tk.MustExec("create table t_prefix(p1 varchar(64),p2 int,c int,primary key(p1(2),p2) clustered,key ic_p(c))", Vec::new());
        tk.MustExec(
            "insert into t_prefix values ('abz',1,0),('abc',2,0),('axy',3,0)",
            Vec::new(),
        );
        assert_not_ordered_without_sort(
            &tk,
            "explain format='plan_tree' select * from t_prefix use index(ic_p) where c=0 order by p1,p2 limit 100",
            "case 7",
        );
        tk.MustQuery(
            "select * from t_prefix use index(ic_p) where c=0 order by p1,p2 limit 100",
            Vec::new(),
        )
        .Check(Rows(&["abc 2 0", "abz 1 0", "axy 3 0"]));

        tk.MustExec("drop table if exists t_varchar_pk", Vec::new());
        tk.MustExec("create table t_varchar_pk(pk varchar(64) primary key clustered,c int,d int,key ic_vc(c))", Vec::new());
        tk.MustExec("insert into t_varchar_pk values ('banana',0,1),('apple',0,2),('cherry',0,3),('date',1,4)", Vec::new());
        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_varchar_pk use index(ic_vc) where c=0 order by pk limit 100",
            "case 8",
        );
        tk.MustQuery(
            "select * from t_varchar_pk use index(ic_vc) where c=0 order by pk limit 100",
            Vec::new(),
        )
        .Check(Rows(&["apple 0 2", "banana 0 1", "cherry 0 3"]));

        tk.MustExec("drop table if exists t_binary_pk", Vec::new());
        tk.MustExec("create table t_binary_pk(pk varbinary(64) primary key clustered,c int,d int,key ic_bin(c))", Vec::new());
        tk.MustExec(
            "insert into t_binary_pk values (x'CC',0,1),(x'AA',0,2),(x'DD',0,3),(x'BB',1,4)",
            Vec::new(),
        );
        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_binary_pk use index(ic_bin) where c=0 order by pk limit 100",
            "case 9",
        );
        let binary_rows = tk
            .MustQuery(
                "select * from t_binary_pk use index(ic_bin) where c=0 order by pk limit 100",
                Vec::new(),
            )
            .Rows();
        assert_eq!(binary_rows.len(), 3, "case 9 rows={binary_rows:?}");
        assert_eq!(
            binary_rows
                .iter()
                .map(|row| row[1].as_str())
                .collect::<Vec<_>>(),
            ["0", "0", "0"]
        );
        assert_eq!(
            binary_rows
                .iter()
                .map(|row| row[2].as_str())
                .collect::<Vec<_>>(),
            ["2", "1", "3"]
        );

        tk.MustExec("drop table if exists t_int_varchar_pk", Vec::new());
        tk.MustExec("create table t_int_varchar_pk(pk1 int,pk2 varchar(64),c int,key ic_iv(c),primary key(pk1,pk2) clustered)", Vec::new());
        tk.MustExec(
            "insert into t_int_varchar_pk values (1,'b',0),(1,'a',0),(2,'a',0),(3,'x',1)",
            Vec::new(),
        );
        assert_ordered_without_topn(
            &tk,
            "explain format='plan_tree' select * from t_int_varchar_pk use index(ic_iv) where c=0 order by pk1,pk2 limit 100",
            "case 10",
        );
        tk.MustQuery(
            "select * from t_int_varchar_pk use index(ic_iv) where c=0 order by pk1,pk2 limit 100",
            Vec::new(),
        )
        .Check(Rows(&["1 a 0", "1 b 0", "2 a 0"]));
    }
}

/// Regression: eliminating an identity Projection preserves IndexScan ordering.
#[test]
fn eliminating_identity_projection_preserves_ordered_common_handle_scan() {
    use astersql_planner_core::rule_eliminate_projection::eliminatePhysicalProjection;
    use astersql_planner_core::task::{Expression, PlanKind, PlanNode};

    let ty = crate::support::int_type();
    let mut scan = PlanNode::new(PlanKind::IndexScan);
    scan.schema = vec![ty.clone()];
    scan.flags.keep_order = true;
    let mut projection = PlanNode::new(PlanKind::Projection);
    projection.schema = vec![ty];
    projection.expressions = vec![Expression {
        column: Some(0),
        ..Default::default()
    }];
    projection.children = vec![scan];

    let result = eliminatePhysicalProjection(projection);
    assert_eq!(result.kind, PlanKind::IndexScan);
    assert!(result.flags.keep_order);
}
