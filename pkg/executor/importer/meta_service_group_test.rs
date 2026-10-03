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
use astersql_lightning_backend_kv::AllocatorType;
use astersql_lightning_common as common;
use astersql_metaservice::{
    Context, DialKeyspaceMeta, EtcdMetadataStore, MetaServiceError, MetadataPdClient, PdClient,
    PdMember,
};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};
struct Pd {
    members: Vec<String>,
    meta: DialKeyspaceMeta,
    closed: AtomicUsize,
}
impl PdClient for Pd {
    fn get_all_members(&self, _: &Context) -> Result<Vec<PdMember>, MetaServiceError> {
        assert!(!self.members.is_empty(), "dedicated group or caller proxy");
        Ok(vec![PdMember {
            client_urls: self.members.clone(),
        }])
    }
}
impl MetadataPdClient for Pd {
    fn load_keyspace(
        &self,
        _: &Context,
        _: &str,
    ) -> Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        panic!("borrow codec metadata")
    }
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}
struct Store(Arc<Pd>);
impl EtcdMetadataStore for Store {
    fn pd_client(&self) -> Result<Arc<dyn MetadataPdClient>, MetaServiceError> {
        Ok(self.0.clone())
    }
    fn keyspace_meta(&self) -> Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        Ok(Some(self.0.meta.clone()))
    }
}
struct Allocator(AtomicI64);
impl common::Allocator for Allocator {
    fn NextGlobalAutoID(&self) -> Result<i64, common::CommonError> {
        Ok(self.0.load(Ordering::SeqCst) + 1)
    }
    fn GetType(&self) -> common::AllocatorType {
        common::AllocatorType::RowID
    }
    fn Rebase(
        &self,
        _: &common::Context,
        base: i64,
        allocate: bool,
    ) -> Result<(), common::CommonError> {
        assert!(!allocate);
        self.0.store(base, Ordering::SeqCst);
        Ok(())
    }
}
struct Requirement(Arc<Allocator>);
impl common::AutoIDRequirement for Requirement {
    fn StoreAvailable(&self) -> bool {
        true
    }
    fn NewAllocator(
        &self,
        db: i64,
        table: i64,
        _: bool,
        kind: common::AllocatorType,
        step: u64,
        _: u16,
    ) -> Arc<dyn common::Allocator> {
        assert_eq!(
            (db, table, kind, step),
            (1, 2, common::AllocatorType::RowID, 2)
        );
        self.0.clone()
    }
}
#[test]
fn allocator_rebase_rejects_store_without_pd() {
    assert_eq!(
        newEtcdClientForAllocatorRebase(&Default::default(), None, &[], Default::default())
            .unwrap_err(),
        "TiKV store does not expose PD client"
    );
}
#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
fn allocator_rebase_consumes_scoped_discovery_and_resets_after_close() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let pd = Arc::new(Pd {
        members: vec![],
        meta: DialKeyspaceMeta {
            id: 45,
            name: "ks4".into(),
            config: [
                (astersql_metaservice::GROUP_ID_KEY.into(), "group4".into()),
                (
                    astersql_metaservice::GROUP_ADDRS_KEY.into(),
                    endpoint.clone(),
                ),
                (
                    astersql_metaservice::GC_MANAGEMENT_TYPE_KEY.into(),
                    "keyspace_level".into(),
                ),
            ]
            .into(),
        },
        closed: AtomicUsize::new(0),
    });
    let allocator = Arc::new(Allocator(AtomicI64::new(0)));
    let reset = Arc::new(AtomicUsize::new(0));
    let reset_copy = reset.clone();
    let plan = Plan {
        DBID: 1,
        DesiredTableInfo: Some(Arc::new(astersql_meta_model::TableInfo {
            ID: 2,
            ..Default::default()
        })),
        ..Default::default()
    };
    RebaseAllocatorsWithMetadata(
        &Store(pd.clone()),
        &["invalid-pd:2379".into()],
        Default::default(),
        &[(AllocatorType::RowIDAllocType, 123)].into(),
        &plan,
        |client| {
            client.put("rebase-key", b"1".to_vec(), None).unwrap();
            let client = client.clone();
            Ok(AllocatorRebaseBindings {
                Requirement: Arc::new(Requirement(allocator.clone())),
                ResetConnection: Box::new(move || {
                    assert!(client.is_closed());
                    reset_copy.fetch_add(1, Ordering::SeqCst);
                }),
            })
        },
    )
    .unwrap();
    assert_eq!(allocator.0.load(Ordering::SeqCst), 123);
    assert_eq!(reset.load(Ordering::SeqCst), 1);
    assert_eq!(pd.closed.load(Ordering::SeqCst), 0);
    let raw = astersql_metaservice::NewEtcdClientFromPDClient(
        &Default::default(),
        pd.as_ref(),
        None,
        &[endpoint.clone()],
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        raw.get("/keyspaces/tidb/45rebase-key", false).unwrap()[0].1,
        b"1"
    );
    raw.delete("/keyspaces/tidb/45rebase-key").unwrap();
    let global_pd = Arc::new(Pd {
        members: vec![endpoint.clone()],
        meta: DialKeyspaceMeta {
            id: 46,
            name: "global".into(),
            config: Default::default(),
        },
        closed: AtomicUsize::new(0),
    });
    let client = newEtcdClientForAllocatorRebase(
        &Default::default(),
        Some(&Store(global_pd.clone())),
        &[endpoint.clone()],
        Default::default(),
    )
    .unwrap();
    assert_eq!(client.endpoints(), [endpoint.clone()]);
    assert_eq!(client.namespace(), "/keyspaces/tidb/46");
    client.close().unwrap();
    let discovered = newEtcdClientForAllocatorRebase(
        &Default::default(),
        Some(&Store(global_pd.clone())),
        &[],
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        discovered.endpoints(),
        [endpoint.trim_start_matches("http://").to_owned()]
    );
    discovered
        .put("storage-pd-key", b"injected".to_vec(), None)
        .unwrap();
    assert_eq!(
        raw.get("/keyspaces/tidb/46storage-pd-key", false).unwrap()[0].1,
        b"injected"
    );
    discovered.delete("storage-pd-key").unwrap();
    discovered.close().unwrap();
    assert_eq!(global_pd.closed.load(Ordering::SeqCst), 0);
    raw.close().unwrap();
}
