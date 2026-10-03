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
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[derive(Default)]
struct Pd {
    meta: Option<DialKeyspaceMeta>,
    members: AtomicUsize,
    loads: AtomicUsize,
    closes: AtomicUsize,
}
impl PdClient for Pd {
    fn get_all_members(&self, _: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        self.members.fetch_add(1, Ordering::SeqCst);
        Ok(vec![PdMember {
            client_urls: vec!["http://pd:2379".into()],
        }])
    }
}
impl MetadataPdClient for Pd {
    fn load_keyspace(
        &self,
        _: &Context,
        _: &str,
    ) -> Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.meta.clone())
    }
    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn resolve_preserves_proxy_and_filters_only_empty_endpoints() {
    let pd = Pd::default();
    let ctx = Context::default();
    let info = ResolveEtcdDialInfo(
        &ctx,
        &pd,
        None,
        &["".into(), "proxy:2379".into(), " ".into()],
    )
    .unwrap();
    assert_eq!(info.endpoints, ["proxy:2379", " "]);
    assert_eq!(pd.members.load(Ordering::SeqCst), 0);
    assert_eq!(
        ResolveEtcdDialInfo(&ctx, &pd, None, &[]).unwrap().endpoints,
        ["pd:2379"]
    );
    assert_eq!(pd.members.load(Ordering::SeqCst), 1);
}
#[test]
fn dedicated_group_uses_metadata_without_pd_discovery() {
    let pd = Pd::default();
    let meta = DialKeyspaceMeta {
        id: 42,
        name: "tenant".into(),
        config: [
            (GROUP_ID_KEY.into(), "tenant-group".into()),
            (crate::GROUP_ADDRS_KEY.into(), "etcd:2379".into()),
            (
                crate::GC_MANAGEMENT_TYPE_KEY.into(),
                crate::GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL.into(),
            ),
        ]
        .into(),
    };
    let info = ResolveEtcdDialInfo(&Context::default(), &pd, Some(&meta), &[]).unwrap();
    assert_eq!(info.endpoints, ["etcd:2379"]);
    assert_eq!(info.namespace, "/keyspaces/tidb/42");
    assert_eq!(pd.members.load(Ordering::SeqCst), 0);
}
#[test]
fn factory_loads_once_and_closes_on_missing_metadata() {
    let pd = Arc::new(Pd::default());
    let captured = pd.clone();
    let factory: PdClientFactory = Arc::new(move |_, _, _| Ok(captured.clone()));
    let result = DialEtcdClient(
        &Context::default(),
        "missing",
        &["pd:2379".into()],
        &PdSecurity::default(),
        Some(&factory),
        EtcdDialConfig::default(),
    );
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("keyspace meta not found for keyspace \"missing\"")
    );
    assert_eq!(pd.loads.load(Ordering::SeqCst), 1);
    assert_eq!(pd.closes.load(Ordering::SeqCst), 1);
    assert_eq!(pd.members.load(Ordering::SeqCst), 0);
}

#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
fn actual_etcd_namespace_roundtrip_lease_cancellation_and_pd_ownership() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let meta = DialKeyspaceMeta {
        id: 41,
        name: "network".into(),
        config: [
            (GROUP_ID_KEY.into(), "group41".into()),
            (crate::GROUP_ADDRS_KEY.into(), endpoint.clone()),
            (
                crate::GC_MANAGEMENT_TYPE_KEY.into(),
                "keyspace_level".into(),
            ),
        ]
        .into(),
    };
    let pd = Arc::new(Pd {
        meta: Some(meta.clone()),
        ..Default::default()
    });
    let ctx = Context::default();
    let client = NewEtcdClientFromPDClient(
        &ctx,
        pd.as_ref(),
        Some(&meta),
        &["invalid-pd:2379".into()],
        Default::default(),
    )
    .unwrap();
    let raw = NewEtcdClientFromPDClient(
        &Default::default(),
        pd.as_ref(),
        None,
        &[endpoint.clone()],
        Default::default(),
    )
    .unwrap();
    let lease = client.grant(60).unwrap();
    client
        .put("/network-test", vec![0, 1, 255], Some(lease))
        .unwrap();
    assert_eq!(
        client.get("/network-test", false).unwrap(),
        vec![(b"/network-test".to_vec(), vec![0, 1, 255])]
    );
    assert_eq!(
        raw.get("/keyspaces/tidb/41/network-test", false).unwrap()[0].1,
        [0, 1, 255]
    );
    assert_eq!(
        client.get_entries("/network-test", false).unwrap()[0].lease,
        lease
    );
    client.keepalive(lease).unwrap();
    assert!(client.time_to_live(lease).unwrap() > 0);
    assert_eq!(pd.loads.load(Ordering::SeqCst), 0);
    assert_eq!(pd.closes.load(Ordering::SeqCst), 0);
    ctx.cancel();
    assert!(matches!(
        client.put("/cancelled", vec![], None),
        Err(MetaServiceError::Cancelled)
    ));
    client
        .with_context(Default::default())
        .revoke(lease)
        .unwrap();
    assert!(
        raw.get("/keyspaces/tidb/41/network-test", false)
            .unwrap()
            .is_empty()
    );
    client.close().unwrap();
    assert!(client.is_closed());
    assert!(client.get("/network-test", false).is_err());
    let captured = pd.clone();
    let factory: PdClientFactory = Arc::new(move |_, _, _| Ok(captured.clone()));
    let factory_client = DialEtcdClient(
        &Default::default(),
        "network",
        &["invalid-pd:2379".into()],
        &Default::default(),
        Some(&factory),
        Default::default(),
    )
    .unwrap();
    factory_client
        .put("/factory-success", b"1".to_vec(), None)
        .unwrap();
    assert_eq!(
        raw.get("/keyspaces/tidb/41/factory-success", false)
            .unwrap()[0]
            .1,
        b"1"
    );
    assert_eq!(pd.loads.load(Ordering::SeqCst), 1);
    assert_eq!(pd.closes.load(Ordering::SeqCst), 1);
    factory_client.delete("/factory-success").unwrap();
    factory_client.close().unwrap();
    raw.close().unwrap();
}

#[test]
fn factory_closes_pd_after_invalid_group_configuration() {
    let pd = Arc::new(Pd {
        meta: Some(DialKeyspaceMeta {
            id: 4,
            name: "invalid".into(),
            config: [(GROUP_ID_KEY.into(), "group4".into())].into(),
        }),
        ..Default::default()
    });
    let captured = pd.clone();
    let factory: PdClientFactory = Arc::new(move |_, _, _| Ok(captured.clone()));
    assert!(
        DialEtcdClient(
            &Default::default(),
            "invalid",
            &[],
            &Default::default(),
            Some(&factory),
            Default::default()
        )
        .is_err()
    );
    assert_eq!(pd.loads.load(Ordering::SeqCst), 1);
    assert_eq!(pd.closes.load(Ordering::SeqCst), 1);
}
