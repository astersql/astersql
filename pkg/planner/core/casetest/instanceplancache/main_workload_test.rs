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

use super::*;

#[test]
fn query_shards_cover_every_template_once() {
    let mut coverage = vec![0; MAIN_QUERY_COUNT];
    for range in MAIN_QUERY_SHARDS {
        assert!(range.start < range.end, "query shard must not be empty");
        assert!(
            range.end <= MAIN_QUERY_COUNT,
            "query shard must stay within the generated workload"
        );
        for index in range {
            coverage[index] += 1;
        }
    }
    assert_eq!(coverage, vec![1; MAIN_QUERY_COUNT]);
}

#[test]
#[should_panic(expected = "query shard must not be empty")]
fn empty_query_shard_fails_explicitly() {
    test_instance_plan_cache_shard(1..1);
}

#[test]
#[should_panic(expected = "exceeds the 340-template workload")]
fn out_of_bounds_query_shard_fails_explicitly() {
    test_instance_plan_cache_shard(0..MAIN_QUERY_COUNT + 1);
}

#[test]
fn generated_workload_matches_go_suite_shape() {
    let patterns = queryPattern();
    assert_eq!(patterns.len(), MAIN_QUERY_COUNT);
    assert!(patterns.iter().any(|sql| sql.contains(" order by ")));
    assert!(patterns.iter().any(|sql| sql.contains(" limit 10")));
    assert!(patterns.iter().any(|sql| sql.contains(" group by ")));
    assert!(patterns.iter().any(|sql| sql.contains(" join ")));

    let tables = prepareTables(1);
    assert_eq!(tables.len(), 2);
    assert!(tables[0].contains("index idx1"));
    assert!(tables[0].contains("index idx2"));
    assert_eq!(tables[1].matches("),(").count() + 1, 100);
}

#[test]
fn prepared_direct_results_match_for_captured_case() {
    let mut harness = Harness::new();
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table captured(c2 int primary key, c3 double)",
    );
    exec(
        &mut harness.tk,
        "insert into captured values (-2147483646, 0)",
    );
    let mut case = TestCase {
        prepared_sql: "select * from captured where c2 between ? and ? or c3 like ?".to_owned(),
        sel_stmts: vec!["select * from captured where c2 between 9223372036854775807 and null or c3 like -0.000000000".to_owned()],
        arguments: vec![vec!["9223372036854775807".to_owned(), "null".to_owned(), "-0.000000000".to_owned()]],
    };
    case.sel_stmts.push(
        "select * from captured where c2 between 9223372036854775807 and null or c3 like 0"
            .to_owned(),
    );
    case.arguments.push(vec![
        "9223372036854775807".to_owned(),
        "null".to_owned(),
        "0".to_owned(),
    ]);
    query(&harness.tk, &case.sel_stmts[0]).Check(Vec::<Vec<String>>::new());
    query(&harness.tk, &case.sel_stmts[1]).Check(rows(&["-2147483646 0".to_owned()]));
    for _ in 0..2 {
        run_case(harness.session(), case.clone());
    }
    // The captured NULL argument is deliberately uncacheable. Separately prove
    // a cacheable input takes the first/repeated/cross-session cache paths.
    for connection in 0..2 {
        let mut tk = harness.session();
        exec(&mut tk, "use test");
        exec(
            &mut tk,
            "prepare hit_probe from 'select c3 from captured where c2=?'",
        );
        exec(&mut tk, "set @probe=-2147483646");
        for execution in 0..3 {
            query(&tk, "execute hit_probe using @probe").Check(rows(&["0".to_owned()]));
            let expected_hit = if connection == 0 && execution == 0 {
                "0"
            } else {
                "1"
            };
            query(&tk, "select @@last_plan_from_cache").Check(rows(&[expected_hit.to_owned()]));
        }
        exec(&mut tk, "deallocate prepare hit_probe");
    }
}
