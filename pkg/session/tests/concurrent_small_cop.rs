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

use astersql_session::testutil::TestSession;
use astersql_store_copr as copr;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};
use protobuf::Message;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

struct Metadata;
impl Metadata {
    fn location() -> copr::KeyLocation {
        copr::KeyLocation {
            region: copr::RegionVerId::new(1, 1, 1),
            store: Some(copr::RegionStore {
                id: 1,
                address: "small-cop-rpc".into(),
                labels: HashMap::new(),
            }),
            peer: Some(copr::Peer { id: 1, store_id: 1 }),
            ..Default::default()
        }
    }
}
impl copr::RegionMetadataTransport for Metadata {
    fn batch_locate_key_ranges(
        &self,
        _: &[copr::KeyRange],
        _: bool,
        _: bool,
    ) -> copr::BatchResult<Vec<copr::KeyLocation>> {
        Ok(vec![Self::location()])
    }
    fn locate_key(&self, _: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn locate_end_key(&self, _: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn locate_region_by_id(&self, _: u64) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn invalidate_region(&self, _: copr::RegionVerId) {}
    fn is_store_alive(&self, _: &str, _: Duration) -> bool {
        true
    }
}
struct Locks;
impl copr::TransactionLockResolver for Locks {
    fn resolve_locks(&self, _: &[copr::TransactionLock], _: u64) -> copr::BatchResult<()> {
        Ok(())
    }
}
#[derive(Default)]
struct WaveState {
    expected: [usize; 2],
    arrived: [usize; 2],
}
struct Transport {
    kv: KVStore,
    state: Mutex<WaveState>,
    ready: Condvar,
}
impl copr::StandardCoprocessorTransport for Transport {
    fn send_unary(
        &self,
        request: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<copr::CopProtocolResponse> {
        let dag = protobuf::parse_from_bytes::<tipb::DagRequest>(&request.wire.data)
            .map_err(|error| copr::BatchError::Transport(error.to_string()))?;
        let scan = match dag
            .get_executors()
            .first()
            .map(|executor| executor.get_tp())
        {
            Some(tipb::ExecType::TypeIndexScan) => 0,
            Some(tipb::ExecType::TypeTableScan) => 1,
            other => {
                return Err(copr::BatchError::Transport(format!(
                    "unexpected scan {other:?}"
                )));
            }
        };
        if request.wire.connection_alias == "concurrent_small_cop" {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut state = self.state.lock().unwrap();
            state.arrived[scan] += 1;
            if state.arrived[scan] > state.expected[scan] {
                return Err(copr::BatchError::Transport(
                    "unexpected extra Cop request".into(),
                ));
            }
            self.ready.notify_all();
            while state.arrived[scan] < state.expected[scan] {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(copr::BatchError::Transport(format!(
                        "only {} of {} requests overlapped",
                        state.arrived[scan], state.expected[scan]
                    )));
                }
                state = self.ready.wait_timeout(state, remaining).unwrap().0;
            }
        }
        // The text dispatcher issues an unaliased COUNT before its local scan.
        // Evaluate that remote boundary from SQL-written MVCC records rather
        // than fabricating a row count or a successful empty response.
        if !dag.get_executors().iter().any(|executor| {
            executor.has_aggregation()
                && executor
                    .get_aggregation()
                    .get_agg_func()
                    .iter()
                    .all(|expr| expr.get_tp() == tipb::ExprType::Count)
        }) {
            return Err(copr::BatchError::Transport(
                "probe supports only the existing COUNT remote boundary".into(),
            ));
        }
        let snapshot = self.kv.GetSnapshot(request.wire.start_ts);
        let mut count = 0;
        for range in &request.wire.ranges {
            let mut iterator = astersql_kv::Retriever::Iter(
                &snapshot,
                astersql_kv::Key(range.start.clone()),
                Some(astersql_kv::Key(range.end.clone())),
            )
            .map_err(|error| copr::BatchError::Transport(error.to_string()))?;
            while iterator.Valid() {
                count += 1;
                iterator
                    .Next()
                    .map_err(|error| copr::BatchError::Transport(error.to_string()))?;
            }
            iterator.Close();
        }
        let encoded = astersql_util_codec::EncodeValue(
            astersql_util_codec::time::UTC,
            Vec::new(),
            vec![astersql_types::datum::NewIntDatum(count)],
        )
        .map_err(|error| copr::BatchError::Transport(error.to_string()))?;
        let mut chunk = tipb::Chunk::new();
        chunk.set_rows_data(encoded);
        let mut response = tipb::SelectResponse::new();
        response.set_chunks(vec![chunk].into());
        Ok(copr::CopProtocolResponse {
            data: response.write_to_bytes().unwrap(),
            ..Default::default()
        })
    }

    fn send_stream(
        &self,
        _: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<Box<dyn copr::CoprocessorResponseStream>> {
        panic!("unexpected streaming Cop")
    }
    fn close(&self) -> copr::BatchResult<()> {
        Ok(())
    }
    fn close_address(&self, _: &str) -> copr::BatchResult<()> {
        Ok(())
    }
}

fn original_sql_fixture(assert_waves: bool) {
    let kv = KVStore::NewMemoryWithWallClockTSO();
    let storage = Arc::try_unwrap(NewMockStorage(kv.clone(), None).unwrap())
        .ok()
        .unwrap();
    let client = storage.canonical_client.clone();
    let domain = Arc::new(astersql_domain::Domain::new(
        storage,
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        astersql_domain::DomainConfig {
            schema_lease: Duration::ZERO,
            stats_lease: Duration::ZERO,
            ..Default::default()
        },
    ));
    struct Close(Arc<astersql_domain::Domain>);
    impl Drop for Close {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = Close(domain.clone());
    domain.init().unwrap();
    let session = astersql_session::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();
    session.Execute("use test").unwrap();
    session.Execute("create table t1 (id int key, b int, c int, index idx_b(b)) partition by hash(id) partitions 10").unwrap();
    for i in 0..10 {
        session
            .Execute(&format!("insert into t1 values ({i},{i},{i})"))
            .unwrap();
    }
    session
        .Execute("create table t2 (id bigint unsigned key, b int, index idx_b(b))")
        .unwrap();
    session
        .Execute("insert into t2 values (1,1), (18446744073709551615,2)")
        .unwrap();
    session
        .Execute("set @@tidb_distsql_scan_concurrency=15")
        .unwrap();
    session
        .Execute("set @@tidb_executor_concurrency=15")
        .unwrap();
    session
        .Execute("set @@tidb_session_alias='concurrent_small_cop'")
        .unwrap();
    let transport = Arc::new(Transport {
        kv,
        state: Mutex::new(WaveState::default()),
        ready: Condvar::new(),
    });
    let backend = copr::NetworkBackend::from_transports(
        Arc::new(Metadata),
        transport.clone(),
        Arc::new(Locks),
    );
    let store = Arc::new(
        copr::Store::new(
            Arc::new(backend),
            &copr::CoprocessorCacheConfig::default(),
            false,
            false,
        )
        .unwrap(),
    );
    let mut driver = astersql_store_driver::TiKVDriver::with_backend(Arc::new(
        astersql_store_driver::InMemoryBackend::default(),
    ));
    let driver_store = driver.Open("tikv://small-cop-rpc:2379").unwrap();
    driver_store.set_coprocessor_store_for_test(store);
    client.SetClient(Arc::new(driver_store));
    let mut failures = Vec::new();
    for (sql, expected) in [
        (
            "select sum(c) from t1 use index (idx_b) where b < 10;",
            [10, 10],
        ),
        (
            "select id, b from t1 use index (idx_b) where b < 10;",
            [1, 0],
        ),
        ("select * from t1 where c < 10;", [0, 10]),
        (
            "select * from t2 where id >= 1 and id <= 18446744073709551615 order by id;",
            [0, 2],
        ),
    ] {
        *transport.state.lock().unwrap() = WaveState {
            expected,
            arrived: [0, 0],
        };
        session.Execute("do 0").unwrap();
        let text_result = session.Execute(sql).and_then(|mut sets| {
            let mut rows = Vec::new();
            for set in &mut sets {
                while let Some(row) = set.Next()? {
                    rows.push(row);
                }
                set.Close()?;
            }
            Ok(rows)
        });
        let expected_rows: Vec<Vec<String>> = if sql.starts_with("select sum") {
            vec![vec!["45".into()]]
        } else if sql.starts_with("select id") {
            (0..10)
                .map(|i| vec![i.to_string(), i.to_string()])
                .collect()
        } else if sql.contains("from t1") {
            (0..10).map(|i| vec![i.to_string(); 3]).collect()
        } else {
            vec![
                vec!["1".into(), "1".into()],
                vec!["18446744073709551615".into(), "2".into()],
            ]
        };
        let mut rows = text_result
            .as_ref()
            .expect("original Go SQL must execute")
            .clone();
        let mut expected_rows = expected_rows;
        rows.sort();
        expected_rows.sort();
        assert_eq!(rows, expected_rows, "{sql}");
        eprintln!(
            "text dispatcher: {sql}: result={text_result:?}, arrived={:?}",
            transport.state.lock().unwrap().arrived
        );
        if !assert_waves {
            continue;
        }
        let text_arrived = transport.state.lock().unwrap().arrived;
        if text_arrived != expected {
            failures.push(format!("text SQL {sql}: {text_arrived:?} != {expected:?}"));
        }
        *transport.state.lock().unwrap() = WaveState {
            expected,
            arrived: [0, 0],
        };
        session.Execute("do 0").unwrap();
        let result = session
            .PreparePlannedKVSelect(sql, domain.info_schema())
            .and_then(|id| session.ExecutePreparedPlannedKVSelectThroughAdapter(id, &[]));
        let arrived = transport.state.lock().unwrap().arrived;
        eprintln!("{sql}: result={result:?}, arrived={arrived:?}, expected={expected:?}");
        if result.is_err() || arrived != expected {
            failures.push(format!("{sql}: {arrived:?} != {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn port_48861f9004_original_sql_results() {
    original_sql_fixture(false);
}

#[test]
fn port_48861f9004_original_sql_waves() {
    original_sql_fixture(true);
}
