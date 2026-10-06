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

use astersql_session::runtime::{ConcreteSession, CreateAnalyzeSession};
use std::sync::Arc;

#[test]
fn create_table_presplit_sends_physical_keys_and_scatter_group_to_storage() {
    #[derive(Default)]
    struct Store(std::sync::Mutex<Vec<(Vec<Vec<u8>>, bool, Option<i64>)>>);
    impl astersql_kv::SplittableStore for Store {
        fn SplitRegions(
            &self,
            _: &astersql_kv::Context,
            keys: &[Vec<u8>],
            scatter: bool,
            group: Option<i64>,
        ) -> Result<Vec<u64>, astersql_kv::errors::SharedError> {
            self.0.lock().unwrap().push((keys.to_vec(), scatter, group));
            Ok(vec![1])
        }
        fn WaitScatterRegionFinish(
            &self,
            _: &astersql_kv::Context,
            _: u64,
            _: i32,
        ) -> Result<(), astersql_kv::errors::SharedError> {
            Ok(())
        }
        fn CheckRegionInScattering(
            &self,
            _: u64,
        ) -> Result<bool, astersql_kv::errors::SharedError> {
            Ok(false)
        }
    }
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let store = Arc::new(Store::default());
    domain.storage_handle().set_region_splitter(store.clone());
    let session = ConcreteSession::new(domain.clone());
    session.execute("set tidb_scatter_region='global'").unwrap();
    session
        .execute(
            "create table test.scatter_requests (a bigint) shard_row_id_bits=2 pre_split_regions=1",
        )
        .unwrap();
    let (_, table) = domain.stats_table("test", "scatter_requests").unwrap();
    let calls = store.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0.len(), 2);
    assert_eq!(
        calls[0].0[0],
        astersql_tablecodec::GenTablePrefix(table.ID).0
    );
    for (_, scatter, group) in calls.iter() {
        assert!(*scatter);
        assert_eq!(*group, Some(-1));
    }
}
