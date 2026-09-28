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

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, Ordering},
};
use std::time::Duration;

use astersql_types::field::NewFieldType;
use astersql_util_chunk as chunk;

use crate::adapter::{AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, SchemaColumn};
use crate::builder::{
    BuildError, CTEStorages, CteProducer, CteStorage, buildIndexJoinHashJoinChildrenWithCleanup,
};

struct CloseCountExecutor {
    closed: Arc<AtomicI32>,
}

impl ExecExecutor for CloseCountExecutor {
    fn Open(&mut self) -> AdapterResult {
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig::default()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(vec![NewFieldType(8)], 1, 1)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &[]
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        Vec::new()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _duration: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

struct Storage;
impl CteStorage for Storage {}
struct Producer;
impl CteProducer for Producer {}

#[test]
fn build_executor_for_index_join_hash_join_error_cleans_children() {
    let lookup_closed = Arc::new(AtomicI32::new(0));
    let other_closed = Arc::new(AtomicI32::new(0));
    let lookup_counter = Arc::clone(&lookup_closed);
    let result = buildIndexJoinHashJoinChildrenWithCleanup(
        move || {
            Ok(Box::new(CloseCountExecutor {
                closed: lookup_counter,
            }))
        },
        || Err(BuildError::new("Unknown Plan mockPhysicalIndexReader")),
    );

    let error = match result {
        Ok(_) => panic!("second child build unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Unknown Plan"));
    assert_eq!(lookup_closed.load(Ordering::SeqCst), 1);
    assert_eq!(other_closed.load(Ordering::SeqCst), 0);
}

#[test]
fn build_cte_storage_producer_cleans_storages_on_recursive_build_error() {
    let storages = CTEStorages {
        res_tbl: Mutex::new(Some(Arc::new(Storage))),
        iter_in_tbl: Mutex::new(Some(Arc::new(Storage))),
        producer: Mutex::new(Some(Arc::new(Producer))),
        init_result: Default::default(),
    };

    storages.clear_after_build_error();

    assert!(storages.is_cleared());
}
