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
use super::*;
use astersql_metaservice::{
    Context, DialKeyspaceMeta, MetaServiceError, MetadataPdClient, PdClient, PdClientFactory,
    PdMember,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Pd {
    endpoint: String,
    closed: AtomicUsize,
    loaded: AtomicUsize,
}
impl PdClient for Pd {
    fn get_all_members(&self, _: &Context) -> std::result::Result<Vec<PdMember>, MetaServiceError> {
        panic!("dedicated group must not discover PD members")
    }
}
impl MetadataPdClient for Pd {
    fn load_keyspace(
        &self,
        _: &Context,
        name: &str,
    ) -> std::result::Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        assert_eq!(name, "ks1");
        self.loaded.fetch_add(1, Ordering::SeqCst);
        Ok(Some(DialKeyspaceMeta {
            id: 42,
            name: name.into(),
            config: [
                (astersql_metaservice::GROUP_ID_KEY.into(), "group1".into()),
                (
                    astersql_metaservice::GROUP_ADDRS_KEY.into(),
                    self.endpoint.clone(),
                ),
                (
                    astersql_metaservice::GC_MANAGEMENT_TYPE_KEY.into(),
                    "keyspace_level".into(),
                ),
            ]
            .into(),
        }))
    }
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
fn br_register_uses_dedicated_group_and_keyspace_namespace() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let pd = Arc::new(Pd {
        endpoint: endpoint.clone(),
        closed: AtomicUsize::new(0),
        loaded: AtomicUsize::new(0),
    });
    let captured = pd.clone();
    let factory: PdClientFactory = Arc::new(move |_, _, _| Ok(captured.clone()));
    let ctx = Context::default();
    let client = dialEtcdWithCfgAndFactory(
        &ctx,
        &Config {
            KeyspaceName: "ks1".into(),
            PD: vec!["invalid-pd:2379".into()],
            ..Default::default()
        },
        Some(&factory),
    )
    .unwrap();
    let lease = client.grant(60).unwrap();
    client
        .put(
            "/tidb/brie/import/restore/restore-test",
            Vec::new(),
            Some(lease),
        )
        .unwrap();
    let raw = astersql_metaservice::NewEtcdClientFromPDClient(
        &ctx,
        pd.as_ref(),
        None,
        &[endpoint],
        Default::default(),
    )
    .unwrap();
    let keys = raw.get("/keyspaces/tidb/42", true).unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0].0,
        b"/keyspaces/tidb/42/tidb/brie/import/restore/restore-test"
    );
    assert_eq!(pd.loaded.load(Ordering::SeqCst), 1);
    assert_eq!(pd.closed.load(Ordering::SeqCst), 1);
    client.revoke(lease).unwrap();
    client.close().unwrap();
    raw.close().unwrap();
}
