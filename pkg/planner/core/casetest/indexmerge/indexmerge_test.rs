// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

fn new_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

/// 对应 Go TestIndexMergePathGeneration：每个 suite 输入都经过真实
/// Parse/Build/LogicalOptimize/RecursiveDeriveStats，并精确比对优化前候选
/// IndexMerge 路径的索引组合与表过滤器 digest。
#[test]
fn test_index_merge_path_generation() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t (a int primary key, b int not null, c int not null, d int not null, e int, c_str varchar(32), d_str varchar(32), e_str varchar(32), f int not null, g int not null, h int, i_date date, unique key c_d_e(c,d,e), unique key f(f), key g(g), unique key f_g(f,g), key c_d_e_str(c_str,d_str,e_str), key e_d_c_str_prefix(e_str(10),d_str(10),c_str(10)))",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    let suite = crate::main_test::load_index_merge_suite();
    let (input, output) = suite
        .LoadTestCasesByName("TestIndexMergePathGeneration", false)
        .unwrap();
    let input = input.as_array().unwrap();
    let output = output.as_array().unwrap();
    assert_eq!(input.len(), 6);
    for (sql, expected) in input.iter().zip(output) {
        let sql = sql.as_str().unwrap();
        let expected = expected.as_str().unwrap();
        let actual = tk
            .Session()
            .IndexMergePathDigestForTest(sql)
            .unwrap_or_else(|error| panic!("derive IndexMerge paths for {sql}: {error}"));
        assert_eq!(actual, expected, "sql={sql}");
    }
}
