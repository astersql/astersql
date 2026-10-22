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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;

use crate::canonical_domain_test::TestStorage;
use crate::{Domain, DomainConfig, InfoSchemaLoader, LoadedInfoSchema};

struct DiagnosticSchemaLoader;

impl InfoSchemaLoader for DiagnosticSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        let schema: SchemaRef = infoschema::infoschema::MockInfoSchemaWithSchemaVer(Vec::new(), 7);
        Ok(LoadedInfoSchema::new(schema, 70))
    }

    fn load_snapshot_info_schema(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
        _timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load_info_schema(store, keyspace)
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(true)
    }
}

struct RecordingDdl {
    starts: Arc<AtomicUsize>,
}

impl crate::domain::DdlService for RecordingDdl {
    fn start(&self, _mode: crate::domain::StartMode) -> Result<(), String> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    fn owner_id(&self) -> Option<String> {
        None
    }

    fn alter_table_mode(&self, _target: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn start_diagnostic_bypasses_ddl_and_starts_only_metadata_readers() {
    let domain = Domain::new(
        TestStorage::new(),
        Arc::new(DiagnosticSchemaLoader),
        DomainConfig {
            schema_lease: Duration::from_secs(60),
            ..DomainConfig::default()
        },
    );
    domain.init().expect("initialize diagnostic Domain");
    let starts = Arc::new(AtomicUsize::new(0));
    domain.set_ddl(Arc::new(RecordingDdl {
        starts: Arc::clone(&starts),
    }));

    domain.start_diagnostic().expect("start diagnostic readers");
    assert_eq!(starts.load(Ordering::Acquire), 0, "DDL must remain stopped");
    assert_eq!(domain.info_schema().SchemaMetaVersion(), 7);
    assert_eq!(domain.background_worker_names(), vec!["schema-reload"]);

    domain.close();
}

/// Go TestDiagnosticServerID: real sessions remain available in diagnostic mode.
#[test]
#[ignore = "requires isolated etcd fixture in ASTERSQL_TEST_ETCD_ENDPOINT; run with --include-ignored --test-threads=1"]
fn diagnostic_server_id_real_etcd_lifecycle() {
    use astersql_domain_serverinfo::{Context, EtcdClient, RealEtcdClient, SyncerOption};

    struct RestoreMode(Option<Box<dyn FnOnce()>>);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            self.0.take().unwrap()();
        }
    }
    let previous = astersql_util_intest::InTest.swap(true, Ordering::SeqCst);
    struct RestoreIntest(bool);
    impl Drop for RestoreIntest {
        fn drop(&mut self) {
            astersql_util_intest::InTest.store(self.0, Ordering::SeqCst);
        }
    }
    let _intest = RestoreIntest(previous);
    let _mode = RestoreMode(Some(Box::new(astersql_config::diagnosticmode::SetForTest(
        true,
    ))));
    let endpoint = std::env::var("ASTERSQL_TEST_ETCD_ENDPOINT").expect("isolated etcd endpoint");
    let context = Context::Background();
    for bits32 in [false, true] {
        let client = Arc::new(
            RealEtcdClient::connect(vec![endpoint.clone()], None)
                .unwrap()
                .with_namespace(format!(
                    "/diagnostic-id-test/{}/{bits32}",
                    std::process::id()
                )),
        );
        let first = Domain::new(
            TestStorage::new(),
            Arc::new(DiagnosticSchemaLoader),
            DomainConfig {
                server_id_ttl: Duration::from_secs(3),
                ..DomainConfig::default()
            },
        );
        let second = Domain::new(
            TestStorage::new(),
            Arc::new(DiagnosticSchemaLoader),
            DomainConfig {
                server_id_ttl: Duration::from_secs(3),
                ..DomainConfig::default()
            },
        );
        for (domain, name) in [(&first, "first"), (&second, "second")] {
            domain
                .install_server_info_syncer(
                    name.into(),
                    client.clone(),
                    &[SyncerOption::WithoutStatusEndpointClaim],
                )
                .unwrap();
        }
        // Domain accepts a proposal function; use the Go 32/64-bit ID ranges.
        let base = if bits32 { 1 } else { 1 << 11 };
        let first_id = first
            .acquire_server_id(|attempt| base + u64::from(attempt), |_| false)
            .unwrap();
        assert_ne!(first_id, 0);
        let key = format!("/tidb/server_id/{first_id}");
        let kv = client.Get(&context, &key, false).unwrap();
        assert_eq!(
            kv.len(),
            1,
            "server ID must have a real etcd key (32bits={bits32})"
        );
        assert_eq!(kv[0].value, b"0");
        let lease = kv[0]
            .lease
            .expect("server ID key must be attached to a session");
        let second_id = second
            .acquire_server_id(|attempt| base + u64::from(attempt), |_| false)
            .unwrap();
        assert_ne!(
            first_id, second_id,
            "etcd must arbitrate conflicting proposals"
        );
        // Wait beyond the original TTL: the real session must keep both keys alive.
        std::thread::sleep(Duration::from_secs(4));
        assert_eq!(
            client.Get(&context, &key, false).unwrap()[0].lease,
            Some(lease)
        );
        assert_eq!(first.server_id(), first_id);
        // Refresh must recreate the actual lease-bound key, not just update Instant.
        client.Delete(&context, &key).unwrap();
        first.refresh_server_id_ttl().unwrap();
        assert_eq!(
            client.Get(&context, &key, false).unwrap()[0].lease,
            Some(lease)
        );
        let syncer = first.server_info_syncer().unwrap();
        let syncer = syncer.lock().unwrap();
        syncer.StoreServerInfo(context.clone()).unwrap();
        assert!(
            client
                .Get(&context, "/tidb/server/info/", true)
                .unwrap()
                .is_empty()
        );
        let getter = syncer
            .GetLocalServerInfo()
            .StaticInfo
            .ServerIDGetter
            .unwrap();
        assert_eq!(getter(), first_id);
        drop(syncer);
        assert_eq!(first.release_server_id(), Some(first_id));
        assert!(client.Get(&context, &key, false).unwrap().is_empty());
        assert!(
            client
                .Put(&context, &key, b"0".to_vec(), Some(lease))
                .is_err(),
            "released lease must no longer accept keys"
        );
        assert_eq!(first.server_id(), 0);
        assert_eq!(getter(), 0);
        second.release_server_id();
        first.close();
        second.close();
        assert!(
            client
                .Get(&context, "/tidb/server_id/", true)
                .unwrap()
                .is_empty()
        );
        eprintln!(
            "32bits={bits32}: unique IDs {first_id}/{second_id}, real lease {lease}, refresh/release and empty server-info verified"
        );
    }
    drop(_mode);
    let _normal = RestoreMode(Some(Box::new(astersql_config::diagnosticmode::SetForTest(
        false,
    ))));
    let client = Arc::new(
        RealEtcdClient::connect(vec![endpoint], None)
            .unwrap()
            .with_namespace(format!("/diagnostic-id-test/{}/normal", std::process::id())),
    );
    let normal = Domain::new(
        TestStorage::new(),
        Arc::new(DiagnosticSchemaLoader),
        DomainConfig::default(),
    );
    normal
        .install_server_info_syncer(
            "normal".into(),
            client.clone(),
            &[SyncerOption::WithoutStatusEndpointClaim],
        )
        .unwrap();
    let id = normal.acquire_server_id(|_| 7, |_| false).unwrap();
    normal
        .server_info_syncer()
        .unwrap()
        .lock()
        .unwrap()
        .StoreServerInfo(context.clone())
        .unwrap();
    let info = client
        .Get(&context, "/tidb/server/info/normal", false)
        .unwrap();
    assert_eq!(info.len(), 1, "normal mode still registers server-info");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&info[0].value).unwrap()["server_id"],
        id
    );
    normal.close();
    assert!(
        client
            .Get(&context, "/tidb/server/", true)
            .unwrap()
            .is_empty()
    );
    assert!(
        client
            .Get(&context, "/tidb/server_id/", true)
            .unwrap()
            .is_empty()
    );
    eprintln!("normal mode: server-info registration and Domain close cleanup verified");
}
