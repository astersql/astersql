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
// Shuffle 分区哈希 splitter 的构造约束测试。
//
// worker 并发度必须为正；为 0 时应 panic，与执行器 Open 前的断言一致。

use std::sync::{Arc, Mutex};

use astersql_errors as errors;
use astersql_util_chunk as chunk;

use crate::shuffle::{
    ShuffleExpression, ShuffleGroupChecker, ShuffleRuntimeContext, buildPartitionHashSplitter,
    buildPartitionRangeSplitter, partitionSplitter,
};

struct TestGroupChecker {
    groups: Vec<(usize, usize)>,
    next: usize,
}

impl ShuffleGroupChecker for TestGroupChecker {
    fn SplitIntoGroups(&mut self, input: &chunk::Chunk) -> Result<(), errors::SharedError> {
        assert_eq!(input.NumRows(), self.groups.last().unwrap().1);
        self.next = 0;
        Ok(())
    }

    fn IsExhausted(&self) -> bool {
        self.next == self.groups.len()
    }

    fn GetNextGroup(&mut self) -> (usize, usize) {
        let group = self.groups[self.next];
        self.next += 1;
        group
    }
}

#[derive(Default)]
struct TestShuffleContext {
    groups: Mutex<Vec<(usize, usize)>>,
}

impl TestShuffleContext {
    fn with_groups(groups: Vec<(usize, usize)>) -> Self {
        Self {
            groups: Mutex::new(groups),
        }
    }
}

impl ShuffleRuntimeContext for TestShuffleContext {
    fn GetGroupKey(
        &self,
        _input: &chunk::Chunk,
        reuse: Vec<Vec<u8>>,
        _byItems: &[ShuffleExpression],
    ) -> Result<Vec<Vec<u8>>, errors::SharedError> {
        Ok(reuse)
    }

    fn NewGroupChecker(&self, _byItems: &[ShuffleExpression]) -> Box<dyn ShuffleGroupChecker> {
        Box::new(TestGroupChecker {
            groups: self.groups.lock().unwrap().clone(),
            next: 0,
        })
    }

    fn ShuffleNextError(&self) -> Option<errors::SharedError> {
        None
    }

    fn TriggerSourceFailpoint(&self) {}

    fn TriggerWorkerFailpoint(&self) {}

    fn InTest(&self) -> bool {
        true
    }

    fn RegisterConcurrencyStats(&self, _name: &str, _concurrency: usize) {}
}
#[test]
/// 正并发可构造；零并发触发 panic。
fn shuffle_hash_splitter_requires_positive_worker_concurrency() {
    let _splitter = buildPartitionHashSplitter(4, Vec::new());
    assert!(std::panic::catch_unwind(|| buildPartitionHashSplitter(0, Vec::new())).is_err());
}

#[test]
fn partition_range_splitter_matches_go_group_round_robin() {
    let ctx = Arc::new(TestShuffleContext::with_groups(vec![
        (0, 4),
        (4, 6),
        (6, 9),
        (9, 10),
        (10, 12),
        (12, 13),
    ]));
    let mut input = chunk::Chunk::default();
    input.SetNumVirtualRows(13);
    let mut splitter = buildPartitionRangeSplitter(ctx.as_ref(), 2, Vec::new());

    let obtained = splitter.split(ctx.as_ref(), &input, Vec::new()).unwrap();

    assert_eq!(obtained, vec![0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
}

#[test]
fn partition_range_splitter_keeps_round_robin_state_across_chunks() {
    let ctx = Arc::new(TestShuffleContext::with_groups(vec![(0, 1)]));
    let mut input = chunk::Chunk::default();
    input.SetNumVirtualRows(1);
    let mut splitter = buildPartitionRangeSplitter(ctx.as_ref(), 2, Vec::new());

    assert_eq!(
        splitter.split(ctx.as_ref(), &input, Vec::new()).unwrap(),
        vec![0]
    );
    assert_eq!(
        splitter.split(ctx.as_ref(), &input, Vec::new()).unwrap(),
        vec![1]
    );
}
