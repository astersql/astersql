// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_ddl_jobsubmit::ServerState;
use astersql_domain_serverinfo::{Context, EtcdClient, MemoryEtcdClient};

use super::{
    CreateAnalyzeSession,
    crossks_schema::{CrossKSSchemaSyncer, CrossKSStateSyncer},
};

#[test]
fn go_merge_43_crossks_schema_and_state_use_target_etcd() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let etcd = Arc::new(MemoryEtcdClient::default());
    let syncer = CrossKSSchemaSyncer::new(Arc::clone(&domain), etcd.clone(), "virtual-a".into());
    syncer.start().unwrap();
    let key = "/tidb/ddl/all_schema_versions/virtual-a";
    assert!(etcd.Snapshot().contains_key(key));
    syncer.publish_global().unwrap();
    assert!(
        etcd.Snapshot()
            .contains_key("/tidb/ddl/global_schema_version")
    );
    etcd.Put(
        &Context::Background(),
        "/tidb/server/info/virtual-b",
        b"{}".to_vec(),
        None,
    )
    .unwrap();
    assert!(
        syncer
            .wait_all_versions(std::time::Duration::from_millis(5))
            .is_err()
    );
    etcd.Put(
        &Context::Background(),
        "/tidb/ddl/all_schema_versions/virtual-b",
        domain
            .info_schema()
            .SchemaMetaVersion()
            .to_string()
            .into_bytes(),
        None,
    )
    .unwrap();
    syncer
        .wait_all_versions(std::time::Duration::from_millis(50))
        .unwrap();
    let state = CrossKSStateSyncer::new(etcd.clone());
    etcd.Put(
        &Context::Background(),
        "/tidb/server/global_state",
        br#"{"state":"upgrading"}"#.to_vec(),
        None,
    )
    .unwrap();
    state.refresh().unwrap();
    assert!(state.is_upgrading());
    etcd.Put(
        &Context::Background(),
        "/tidb/server/global_state",
        br#"{"state":""}"#.to_vec(),
        None,
    )
    .unwrap();
    state.refresh().unwrap();
    assert!(!state.is_upgrading());
    syncer.close();
    assert!(!etcd.Snapshot().contains_key(key));
    domain.close();
}
