// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `pkg/executor/join_pkg_test.go` 的包级 Hash Join 回归测试。
//
// Go 测试覆盖 V2 在 Apply 场景下反复 Open/Close，以及 V1 在并发度、数据量和
// spill 组合下的完整结果。这里直接驱动真实 Rust Hash Join 执行器，保留相同矩阵。

use astersql_executor_join::hash_join_base::HashJoinContextBase;
use astersql_executor_join::hash_join_v1::{HashJoinCtxV1, HashJoinV1Exec};
use astersql_executor_join::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use astersql_executor_join::joiner::{JoinType, Joiner, Row};
use astersql_executor_join::row_table_builder::Value;

fn rows(count: usize) -> Vec<Row> {
    (0..count)
        .map(|row| vec![Value::Int(row as i64), Value::Float(row as f64)])
        .collect()
}

fn inner_joiner(max_chunk_size: usize) -> Joiner {
    Joiner::new(
        JoinType::Inner,
        true,
        Vec::new(),
        Vec::new(),
        None,
        false,
        max_chunk_size,
    )
    .expect("build inner joiner")
}

fn hash_join_v2(row_count: usize, concurrency: usize) -> HashJoinV2Exec {
    let context = HashJoinCtxV2::new(
        JoinType::Inner,
        vec![0, 1],
        vec![0, 1],
        false,
        false,
        concurrency,
        1024,
        None,
    )
    .expect("build V2 hash join context");
    let data = rows(row_count);
    HashJoinV2Exec::new(context, inner_joiner(1024), vec![data.clone()], vec![data])
        .expect("build V2 hash join executor")
}

fn assert_hash_join_v2_reopens(row_count: usize) {
    let mut executor = hash_join_v2(row_count, 4);
    for _ in 0..10 {
        executor.open().expect("open V2 hash join");
        let result = executor.execute_all().expect("drain V2 hash join");
        assert!(result.len() >= row_count);
        executor.close();
    }
}

#[test]
fn hash_join_v2_under_apply_reopens_the_same_executor_ten_times() {
    assert_hash_join_v2_reopens(4096);
}

#[test]
#[ignore = "exact Go 100000-row scale; run explicitly for the migration evidence gate"]
fn hash_join_v2_under_apply_runs_exact_go_scale() {
    assert_hash_join_v2_reopens(100_000);
}

fn assert_join_result(result: &[Row], row_count: usize) {
    assert_eq!(result.len(), row_count);
    let mut visited = vec![false; row_count];
    for row in result {
        assert_eq!(row.len(), 4);
        let [
            Value::Int(left_int),
            Value::Float(left_float),
            Value::Int(right_int),
            Value::Float(right_float),
        ] = row.as_slice()
        else {
            panic!("unexpected joined row: {row:?}");
        };
        assert_eq!(*left_float, *left_int as f64);
        assert_eq!(*right_int, *left_int);
        assert_eq!(*right_float, *left_int as f64);
        visited[*left_int as usize] = true;
    }
    assert!(visited.into_iter().all(|seen| seen));
}

#[test]
fn hash_join_exec_covers_concurrency_rows_and_spill_matrix() {
    for concurrency in [1, 4] {
        for row_count in [3, 1024, 4096] {
            for spill in [false, true] {
                let context = HashJoinCtxV1 {
                    base: HashJoinContextBase::default(),
                    join_type: JoinType::Inner,
                    build_key_indices: vec![0, 1],
                    probe_key_indices: vec![0, 1],
                    null_aware: false,
                    build_side_is_outer: false,
                    concurrency,
                    max_chunk_size: 1024,
                };
                let data = rows(row_count);
                let mut executor = HashJoinV1Exec::new(
                    context,
                    inner_joiner(1024),
                    vec![data.clone()],
                    vec![data],
                )
                .expect("build V1 hash join executor");
                if spill {
                    executor.SetMemoryLimit(Some(1));
                }

                executor.open().expect("open V1 hash join");
                let result = executor.execute_all().expect("drain V1 hash join");
                assert_eq!(executor.IsSpillTriggered(), spill);
                assert_join_result(&result, row_count);
                executor.close();
            }
        }
    }
}
