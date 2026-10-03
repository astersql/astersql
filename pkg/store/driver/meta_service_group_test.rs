// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::*;
use astersql_metaservice::EtcdMetadataStore;
use std::sync::Arc;
#[derive(Debug)]
struct Backend(String);
impl DriverBackend for Backend {
    fn open_pd(
        &self,
        _: &[String],
        _: &str,
        _: &Security,
        _: &PdClientOptions,
    ) -> Result<u64, DriverError> {
        Ok(840047)
    }
    fn new_safe_point_kv(
        &self,
        _: u64,
        _: &str,
        _: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError> {
        Ok(SafePointKvSetup {
            meta_service_info: MetaServiceInfo {
                pd_addrs: vec![self.0.clone()],
                group_addrs: vec![self.0.clone()],
            },
            pd_addrs: vec![self.0.clone()],
            group_addrs: vec![self.0.clone()],
            safe_point_id: "metadata-test".into(),
        })
    }
    fn current_timestamp(&self, _: &str) -> Result<u64, DriverError> {
        Ok(1)
    }
}
#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT with PD metadata RPC fixture"]
fn store_retains_full_network_pd_codec_metadata_and_owns_pd_lifetime() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let store = TiKVDriver::with_backend(Arc::new(Backend(endpoint)))
        .Open("tikv://127.0.0.1:2379?keyspaceName=ks-driver")
        .unwrap();
    let pd = store.pd_client().unwrap();
    assert!(Arc::ptr_eq(&pd, &store.pd_client().unwrap()));
    let meta = store.keyspace_meta().unwrap().unwrap();
    assert_eq!(meta.id, 47);
    assert_eq!(meta.name, "ks-driver");
    assert_eq!(meta.config[astersql_metaservice::GROUP_ID_KEY], "group47");
    assert_eq!(store.etcd_namespace().unwrap(), "/keyspaces/tidb/47");
    let client = astersql_metaservice::NewEtcdClientFromStore(
        &Default::default(),
        &store,
        &["invalid-pd:2379".into()],
        Default::default(),
    )
    .unwrap();
    client.put("/store-key", b"1".to_vec(), None).unwrap();
    assert_eq!(client.get("/store-key", false).unwrap()[0].1, b"1");
    client.delete("/store-key").unwrap();
    client.close().unwrap();
    assert!(!pd.get_all_members(&Default::default()).unwrap().is_empty());
    store.Close().unwrap();
    assert!(pd.get_all_members(&Default::default()).is_err());
    assert!(store.keyspace_meta().is_err());
}
