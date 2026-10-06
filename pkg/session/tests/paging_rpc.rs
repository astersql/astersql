// Copyright 2026 AsterSQL.

use astersql_kv as kv;
use astersql_session::testutil::TestSession;
use astersql_store_copr as copr;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};
use protobuf::Message;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

struct Metadata;
impl Metadata {
    fn location() -> copr::KeyLocation {
        copr::KeyLocation {
            region: copr::RegionVerId::new(1, 1, 1),
            store: Some(copr::RegionStore {
                id: 1,
                address: "paging-rpc".into(),
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
struct Observation {
    budgets: Vec<u64>,
    pages: usize,
    update: Option<String>,
}
struct Transport {
    kv: KVStore,
    domain: Weak<astersql_domain::Domain>,
    observation: Mutex<Observation>,
}
impl copr::StandardCoprocessorTransport for Transport {
    fn send_unary(
        &self,
        request: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<copr::CopProtocolResponse> {
        // Only the remote CSE boundary is replaced. Read actual SQL-written
        // MVCC records at the request timestamp and return standard DAG packets.
        assert_eq!(
            request.wire.paging_size, 0,
            "row-count paging must stay disabled"
        );
        let update = {
            let mut observation = self.observation.lock().unwrap();
            observation.budgets.push(request.wire.paging_size_bytes);
            observation.update.take()
        };
        if let Some(value) = update {
            let writer = astersql_session::runtime::ConcreteSession::new(
                self.domain.upgrade().expect("test domain remains open"),
            );
            writer
                .Execute(&format!("set global tidb_paging_size_bytes={value}"))
                .unwrap();
        }
        let dag = protobuf::parse_from_bytes::<tipb::DagRequest>(&request.wire.data).unwrap();
        let columns = dag
            .get_executors()
            .iter()
            .find(|executor| executor.has_tbl_scan())
            .expect("real SQL table scan DAG")
            .get_tbl_scan()
            .get_columns();
        let types = columns
            .iter()
            .map(|column| {
                (
                    column.get_column_id(),
                    Box::new(astersql_parser_types::NewFieldType(column.get_tp() as u8)),
                )
            })
            .collect::<HashMap<_, _>>();
        let snapshot = self.kv.GetSnapshot(request.wire.start_ts);
        let mut encoded = Vec::new();
        let mut read_bytes = 0_u64;
        let mut count = 0_u64;
        let mut page_end = None;
        for range in &request.wire.ranges {
            let mut iterator = kv::Retriever::Iter(
                &snapshot,
                kv::Key(range.start.clone()),
                Some(kv::Key(range.end.clone())),
            )
            .unwrap();
            while iterator.Valid() {
                let key = iterator.Key();
                let value = iterator.Value();
                let (_, handle) = astersql_tablecodec::DecodeRecordKey(key.clone()).unwrap();
                let values = astersql_tablecodec::DecodeRowToDatumMap(
                    Some(value.clone()),
                    types.clone(),
                    Some(astersql_tablecodec::time::UTC),
                )
                .unwrap();
                let row = columns
                    .iter()
                    .map(|column| {
                        if column.get_pk_handle() {
                            astersql_types::datum::NewIntDatum(handle.IntValue())
                        } else {
                            values
                                .get(&column.get_column_id())
                                .expect("stored column")
                                .clone()
                        }
                    })
                    .collect();
                encoded =
                    astersql_util_codec::EncodeValue(astersql_util_codec::time::UTC, encoded, row)
                        .unwrap();
                count += 1;
                read_bytes += (key.0.len() + value.len()) as u64;
                iterator.Next().unwrap();
                if request.wire.paging_size_bytes > 0
                    && read_bytes >= request.wire.paging_size_bytes
                {
                    if iterator.Valid() {
                        page_end = Some(iterator.Key().0);
                    }
                    break;
                }
            }
            iterator.Close();
            if page_end.is_some() {
                break;
            }
        }
        let mut chunk = tipb::Chunk::new();
        chunk.set_rows_data(encoded);
        let mut response = tipb::SelectResponse::new();
        response.set_chunks(protobuf::RepeatedField::from_vec(vec![chunk]));
        let range = if request.wire.paging_size_bytes > 0 {
            self.observation.lock().unwrap().pages += 1;
            Some(copr::KeyRange {
                start: request.wire.ranges[0].start.clone(),
                end: page_end.unwrap_or_else(|| request.wire.ranges.last().unwrap().end.clone()),
            })
        } else {
            None
        };
        Ok(copr::CopProtocolResponse {
            data: response.write_to_bytes().unwrap(),
            range,
            read_bytes,
            scanned_keys: count,
            processed_keys: count,
            ..Default::default()
        })
    }
    fn send_stream(
        &self,
        _: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<Box<dyn copr::CoprocessorResponseStream>> {
        panic!("SQL scan must use unary Cop RPC")
    }
    fn close(&self) -> copr::BatchResult<()> {
        Ok(())
    }
    fn close_address(&self, _: &str) -> copr::BatchResult<()> {
        Ok(())
    }
}

#[test]
fn global_budget_updates_preserve_all_rpc_pages_and_full_sql_results() {
    struct Restore(i64);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::PagingSizeBytes.Store(self.0);
        }
    }
    {
        let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK.lock().unwrap();
        unsafe {
            if (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Total)).is_none() {
                astersql_metrics::ru_v2::InitRUV2Metrics();
            }
        }
    }
    let _restore = Restore(astersql_sessionctx_vardef::PagingSizeBytes.Load());
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
    struct CloseDomain(Arc<astersql_domain::Domain>);
    impl Drop for CloseDomain {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close_domain = CloseDomain(domain.clone());
    domain.init().unwrap();
    let writer = astersql_session::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();
    writer
        .Execute("set global tidb_paging_size_bytes=0")
        .unwrap();
    writer
        .Execute("set global tidb_enable_resource_control=on")
        .unwrap();
    writer
        .Execute("alter resource group `default` ru_per_sec=100000 burstable=off")
        .unwrap();
    writer
        .Execute("create table paging_global (id int primary key, payload varchar(1024))")
        .unwrap();
    let payload = "x".repeat(1024);
    // Store real rows before injecting the RPC transport, retaining the complete
    // fixture and the production transaction/row encoding implementation.
    writer.Execute("begin").unwrap();
    for id in 0..512 {
        writer
            .Execute(&format!(
                "insert into paging_global values ({id},'{payload}')"
            ))
            .unwrap();
    }
    writer.Execute("commit").unwrap();
    let transport = Arc::new(Transport {
        kv,
        domain: Arc::downgrade(&domain),
        observation: Mutex::new(Observation::default()),
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
    let driver_store = driver.Open("tikv://paging-rpc:2379").unwrap();
    driver_store.set_coprocessor_store_for_test(store.clone());
    client.SetClient(Arc::new(driver_store));
    let isolated = NewMockStorage(KVStore::NewMemoryWithWallClockTSO(), None).unwrap();
    assert!(
        !kv::Storage::GetClient(isolated.as_ref())
            .IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeBasic),
        "RPC injection must not affect another store"
    );
    let reader = astersql_session::runtime::ConcreteSession::new(domain.clone());
    reader.Execute("set tidb_enable_paging=off").unwrap();
    reader
        .Execute("set tidb_distsql_scan_concurrency=1")
        .unwrap();
    let prepared = reader
        .PreparePlannedKVSelect(
            "select * from paging_global order by id",
            domain.info_schema(),
        )
        .unwrap();
    let scan = |budget: u64, update: Option<&str>| {
        *transport.observation.lock().unwrap() = Observation {
            update: update.map(str::to_owned),
            ..Default::default()
        };
        // Execute the existing canonical prepared adapter after a SQL statement
        // boundary reset. The text dispatcher currently routes explicit
        // transactions to its snapshot evaluator; no paging loop is mocked.
        reader.Execute("do 0").unwrap();
        let results = reader
            .ExecutePreparedPlannedKVSelectThroughAdapter(prepared, &[])
            .unwrap();
        assert_eq!(results.Rows.len(), 512);
        for (id, row) in results.Rows.iter().enumerate() {
            assert_eq!(
                row.0,
                vec![
                    astersql_executor_sortexec::SortValue::Int(id as i64),
                    astersql_executor_sortexec::SortValue::Bytes("x".repeat(1024).into_bytes())
                ]
            );
        }
        let observation = transport.observation.lock().unwrap();
        assert!(
            !observation.budgets.is_empty(),
            "must send real SQL DAG requests"
        );
        assert!(
            observation.budgets.iter().all(|actual| *actual == budget),
            "budgets={:?}",
            observation.budgets
        );
        if budget > 0 {
            assert!(observation.pages > 1, "must exercise range continuation");
        } else {
            assert_eq!(observation.pages, 0);
        }
        eprintln!(
            "budget={budget} update={update:?} requests={} pages={} rows=512",
            observation.budgets.len(),
            observation.pages
        );
    };
    reader.Execute("begin").unwrap();
    for budget in [0, 4096, 65536, 1024, 0] {
        writer
            .Execute(&format!("set global tidb_paging_size_bytes={budget}"))
            .unwrap();
        scan(budget, None);
    }
    reader.Execute("rollback").unwrap();
    scan(0, Some("4096"));
    scan(4096, None);
    scan(4096, Some("0"));
    scan(0, None);
    writer
        .Execute("set global tidb_paging_size_bytes=4096")
        .unwrap();
    writer
        .Execute("alter resource group `default` ru_per_sec=100000 burstable=unlimited")
        .unwrap();
    scan(0, None);
    writer
        .Execute("alter resource group `default` ru_per_sec=100000 burstable=off")
        .unwrap();
    writer
        .Execute("set global tidb_enable_resource_control=off")
        .unwrap();
    scan(0, None);
    store.close();
    domain.close();
}
