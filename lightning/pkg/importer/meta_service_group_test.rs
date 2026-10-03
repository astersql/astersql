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
use astersql_metaservice::{
    Context as MetaContext, DialKeyspaceMeta, EtcdMetadataStore, MetaServiceError,
    MetadataPdClient, PdClient, PdClientFactory, PdMember,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct Pd {
    meta: DialKeyspaceMeta,
    names: Mutex<Vec<String>>,
    closed: AtomicUsize,
}
impl PdClient for Pd {
    fn get_all_members(
        &self,
        _: &MetaContext,
    ) -> std::result::Result<Vec<PdMember>, MetaServiceError> {
        panic!("dedicated group or caller proxy")
    }
}
impl MetadataPdClient for Pd {
    fn load_keyspace(
        &self,
        _: &MetaContext,
        name: &str,
    ) -> std::result::Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        self.names.lock().unwrap().push(name.into());
        assert_eq!(name, self.meta.name);
        Ok(Some(self.meta.clone()))
    }
    fn close(&self) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}
struct Store(Arc<Pd>);
impl EtcdMetadataStore for Store {
    fn pd_client(&self) -> std::result::Result<Arc<dyn MetadataPdClient>, MetaServiceError> {
        Ok(self.0.clone())
    }
    fn keyspace_meta(&self) -> std::result::Result<Option<DialKeyspaceMeta>, MetaServiceError> {
        Ok(Some(self.0.meta.clone()))
    }
}
fn controller(cfg: &config::Config, keyspace: &str) -> Controller {
    NewImportControllerWithPauser(
        context::Background(),
        cfg,
        ControllerParam {
            DBMetas: vec![],
            Status: None,
            DumpFileStorage: storeapi::Storage::new("file:///tmp"),
            OwnExtStorage: false,
            Pauser: None,
            DB: None,
            CheckpointStorage: None,
            CheckpointName: String::new(),
            DupIndicator: None,
            KeyspaceName: keyspace.into(),
            ResourceGroupName: String::new(),
            TaskType: String::new(),
        },
    )
    .unwrap()
}
#[test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
fn lightning_precheck_register_and_local_backend_consume_real_metadata_group() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let pd = Arc::new(Pd {
        meta: DialKeyspaceMeta {
            id: 43,
            name: "ks2".into(),
            config: [
                (astersql_metaservice::GROUP_ID_KEY.into(), "group2".into()),
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
        names: Mutex::new(vec![]),
        closed: AtomicUsize::new(0),
    });
    let captured = pd.clone();
    let factory: PdClientFactory = Arc::new(move |_, _, _| Ok(captured.clone()));
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.TikvImporter.KeyspaceName = "wrong-keyspace".into();
    cfg.TiDB.PdAddr = "invalid-pd:2379".into();
    cfg.MetadataRuntime.pd_factory = Some(factory);
    cfg.MetadataRuntime.store = Some(Arc::new(Store(pd.clone())));
    let mut rc = controller(&cfg, "ks2");
    let target = NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None).unwrap();
    let pre = NewPreImportInfoGetter(
        &cfg,
        vec![],
        storeapi::Storage::new("file:///"),
        target,
        None,
        None,
        vec![],
    )
    .unwrap();
    rc.precheckItemBuilder = Some(NewPrecheckItemBuilder(&cfg, vec![], pre, None, None, None));
    rc.doPreCheckOnItem(
        context::Background(),
        astersql_lightning_pkg_precheck::CheckTargetUsingCDCPITR,
    )
    .unwrap();
    assert_eq!(*pd.names.lock().unwrap(), ["ks2"]);
    assert_eq!(pd.closed.load(Ordering::SeqCst), 1);
    let raw = astersql_metaservice::NewEtcdClientFromPDClient(
        &Default::default(),
        pd.as_ref(),
        None,
        &[endpoint.clone()],
        Default::default(),
    )
    .unwrap();
    let undo = rc.registerTaskToPD(context::Background()).unwrap();
    let entries = raw
        .get("/keyspaces/tidb/43/tidb/brie/import/lightning/", true)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(*pd.names.lock().unwrap(), ["ks2", "ks2"]);
    undo();
    undo();
    assert!(
        raw.get("/keyspaces/tidb/43/tidb/brie/import/lightning/", true)
            .unwrap()
            .is_empty()
    );
    let client = rc
        .newEtcdClientForLocalBackend(context::Background(), &Store(pd.clone()))
        .unwrap();
    client.put("checksum-key", b"1".to_vec(), None).unwrap();
    assert_eq!(
        raw.get("/keyspaces/tidb/43checksum-key", false).unwrap()[0].1,
        b"1"
    );
    assert_eq!(pd.closed.load(Ordering::SeqCst), 2); // borrowed storage PD was not closed
    client.delete("checksum-key").unwrap();
    client.close().unwrap();
    let global_pd = Arc::new(Pd {
        meta: DialKeyspaceMeta {
            id: 44,
            name: "global".into(),
            config: Default::default(),
        },
        names: Mutex::new(vec![]),
        closed: AtomicUsize::new(0),
    });
    cfg.TiDB.PdAddr = endpoint.clone();
    let global = controller(&cfg, "global");
    let client = global
        .newEtcdClientForLocalBackend(context::Background(), &Store(global_pd.clone()))
        .unwrap();
    assert_eq!(client.endpoints(), [endpoint]);
    assert_eq!(client.namespace(), "/keyspaces/tidb/44");
    client.close().unwrap();
    assert_eq!(global_pd.closed.load(Ordering::SeqCst), 0);
    raw.close().unwrap();
}
