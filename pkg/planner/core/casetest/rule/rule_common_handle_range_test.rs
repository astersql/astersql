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

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

fn rows(value: &str) -> Vec<Vec<String>> {
    astersql_testkit::Rows(&value.lines().collect::<Vec<_>>())
}

#[test]
fn common_handle_secondary_index_ranges() {
    for cascades in [false, true] {
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
        tk.MustExec("CREATE TABLE t_chr (tenant int, seq bigint, a int, b int, c int, PRIMARY KEY(tenant, seq) CLUSTERED, KEY ia(a), KEY ia_overlap(a, tenant), UNIQUE KEY ub(b))", Vec::new());
        tk.MustExec("insert into t_chr values (1,100,10,1,0),(1,200,10,2,0),(2,100,10,3,0),(2,200,20,4,0),(3,100,10,5,1)", Vec::new());
        for (sql, expected) in [
            (
                "select * from t_chr use index(ia) where a = 10 and tenant = 2",
                "2 100 10 3 0",
            ),
            (
                "select * from t_chr use index(ia) where a = 10 and tenant = 1 and seq > 100",
                "1 200 10 2 0",
            ),
            (
                "select * from t_chr use index(ia) where a = 10 and tenant > 1 order by tenant, seq",
                "2 100 10 3 0\n3 100 10 5 1",
            ),
            (
                "select * from t_chr use index(ia) where a in (10, 20) and tenant = 2 order by tenant, seq",
                "2 100 10 3 0\n2 200 20 4 0",
            ),
            (
                "select * from t_chr use index(ia) where a = 10 and seq = 100 order by tenant, seq",
                "1 100 10 1 0\n2 100 10 3 0\n3 100 10 5 1",
            ),
            (
                "select * from t_chr use index(ia_overlap) where a = 10 and tenant = 1 and seq > 100",
                "1 200 10 2 0",
            ),
            (
                "select * from t_chr use index(ub) where b > 0 and tenant = 2 order by tenant, seq",
                "2 100 10 3 0\n2 200 20 4 0",
            ),
            (
                "select tenant, seq, a from t_chr use index(ia) where a = 10 and tenant = 2",
                "2 100 10",
            ),
        ] {
            // The typed planner test verifies the exact physical ranges/schema.
            tk.MustQuery(sql, Vec::new()).Check(rows(expected));
        }
        tk.MustExec("CREATE TABLE t_chr_ci (tenant varchar(32) COLLATE utf8mb4_general_ci, seq int, a int, PRIMARY KEY(tenant, seq) CLUSTERED, KEY ia(a))", Vec::new());
        tk.MustExec(
            "insert into t_chr_ci values ('abc',1,10),('ABC',2,10),('xyz',1,10),('abc',3,20)",
            Vec::new(),
        );
        tk.MustQuery(
            "select * from t_chr_ci use index(ia) where a = 10 and tenant = 'AbC' order by seq",
            Vec::new(),
        )
        .Check(rows("abc 1 10\nABC 2 10"));
        tk.MustQuery(
            "select * from t_chr_ci use index(ia) where a = 10 and tenant = 'abc' and seq >= 2",
            Vec::new(),
        )
        .Check(rows("ABC 2 10"));
        tk.MustExec("CREATE TABLE t_chr_prefix (p1 varchar(64), p2 int, c int, PRIMARY KEY(p1(2), p2) CLUSTERED, KEY ic(c))", Vec::new());
        tk.MustExec(
            "insert into t_chr_prefix values ('abz',1,0),('abc',2,0),('axy',3,0),('abc',4,1)",
            Vec::new(),
        );
        tk.MustQuery(
            "select * from t_chr_prefix use index(ic) where c = 0 and p1 = 'abc'",
            Vec::new(),
        )
        .Check(rows("abc 2 0"));
        tk.MustQuery(
            "select * from t_chr_prefix use index(ic) where c = 0 and p1 = 'abz' and p2 = 1",
            Vec::new(),
        )
        .Check(rows("abz 1 0"));
        tk.MustExec("CREATE TABLE t_chr_single (pk varchar(32), a int, b int, PRIMARY KEY(pk) CLUSTERED, KEY ia(a))", Vec::new());
        tk.MustExec(
            "insert into t_chr_single values ('u1',10,1),('u2',10,2),('u3',10,3),('u4',20,4)",
            Vec::new(),
        );
        for (sql, expected) in [
            (
                "select * from t_chr_single use index(ia) where a = 10 and pk = 'u2'",
                "u2 10 2",
            ),
            (
                "select * from t_chr_single use index(ia) where a = 10 and pk > 'u1' order by pk",
                "u2 10 2\nu3 10 3",
            ),
            (
                "select pk, a from t_chr_single use index(ia) where a = 10 and pk = 'u2'",
                "u2 10",
            ),
        ] {
            tk.MustQuery(sql, Vec::new()).Check(rows(expected));
        }
        tk.MustExec("CREATE TABLE t_chr_dec (pk decimal(10,2), a int, PRIMARY KEY(pk) CLUSTERED, KEY ia(a))", Vec::new());
        tk.MustExec(
            "insert into t_chr_dec values (1.50,10),(2.50,10),(3.50,20)",
            Vec::new(),
        );
        let sql = "select * from t_chr_dec use index(ia) where a = 10 and pk = 2.50";
        tk.MustQuery(sql, Vec::new()).Check(rows("2.50 10"));
    }
}

#[test]
fn common_handle_index_ranges_with_tuple_compare() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("CREATE TABLE t_chr_tuple (a bigint not null, b bigint not null, c bigint not null, PRIMARY KEY(b, c) CLUSTERED, KEY ia(a))", Vec::new());
    tk.MustExec(
        "insert into t_chr_tuple values (1,2,3),(1,2,4),(1,3,1),(2,1,1)",
        Vec::new(),
    );
    // Exact physical ranges are checked through the real optimizer in
    // core/integration_test.rs::common_handle_secondary_index_range_planning.
    tk.MustQuery(
        "select * from t_chr_tuple where (a,b,c) > (1,2,3) order by a,b,c",
        Vec::new(),
    )
    .Check(rows("1 2 4\n1 3 1\n2 1 1"));
    tk.MustExec("CREATE TABLE t_chr_tuple3 (a bigint not null, b bigint not null, c bigint not null, d bigint not null, PRIMARY KEY(b,c,d) CLUSTERED, KEY ia(a))", Vec::new());
    tk.MustExec(
        "insert into t_chr_tuple3 values (1,2,3,4),(1,2,3,5),(1,2,4,1)",
        Vec::new(),
    );
    tk.MustQuery(
        "select * from t_chr_tuple3 where (a,b,c,d) > (1,2,3,4) order by a,b,c,d",
        Vec::new(),
    )
    .Check(rows("1 2 3 5\n1 2 4 1"));
}

#[test]
fn index_range_estimation_with_prefixed_common_handle() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(p1 varchar(64), p2 int, c int, primary key(p1(2),p2) clustered, key ic(c))",
        Vec::new(),
    );
    let values = (1..=100)
        .map(|i| format!("('pp_{i:03}',{i},{})", i % 10))
        .collect::<Vec<_>>();
    tk.MustExec(
        &format!("insert into t values {}", values.join(",")),
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    // Scan ranges, retained prefix filters, and 40/50 row estimates are
    // checked at the optimizer/cardinality boundaries in their owning crates.
    let sql = "select * from t use index(ic) where c = 5 and p1 = 'pp_055'";
    let sql2 = "select * from t use index(ic) where c = 5 and p1 = 'pp_055' and p2 = 55";
    let tuple = "select * from t where (c,p1,p2) > (5,'pp_055',55)";
    let forced = "select * from t use index(ic) where (c,p1,p2) > (5,'pp_055',55)";
    tk.MustQuery(sql, Vec::new()).Check(rows("pp_055 55 5"));
    tk.MustQuery(sql2, Vec::new()).Check(rows("pp_055 55 5"));
    assert_eq!(tk.MustQuery(tuple, Vec::new()).Rows().len(), 44);
    assert_eq!(tk.MustQuery(forced, Vec::new()).Rows().len(), 44);
}
