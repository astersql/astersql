// Copyright 2026 AsterSQL.
//! Domain resources for CREATE TABLE; auto-ID writes commit independently.
use super::kv;
use astersql_domain::Domain;
use astersql_meta_autoid as autoid;
use astersql_meta_model::TableInfo;
use std::sync::Arc;
fn storage_error(e: impl ToString) -> autoid::AutoIdError {
    autoid::AutoIdError::Storage(e.to_string())
}
struct Store(Arc<astersql_domain::canonical_domain::StorageHandle>);
struct Txn<'a>(&'a mut dyn kv::Transaction);
fn key(k: autoid::AutoIdKey) -> kv::Key {
    let prefix = match k.kind {
        autoid::AutoIdKeyKind::RowId => "TID",
        autoid::AutoIdKeyKind::IncrementId(v) if v < 5 => "TID",
        autoid::AutoIdKeyKind::IncrementId(_) => "IID",
        autoid::AutoIdKeyKind::RandomId => "TARID",
        autoid::AutoIdKeyKind::SequenceValue => "SID",
        autoid::AutoIdKeyKind::SequenceCycle => "SequenceCycle",
    };
    astersql_meta::transaction_meta_hash_key(
        format!("DB:{}", k.database_id).as_bytes(),
        format!("{prefix}:{}", k.table_id).as_bytes(),
    )
}
impl autoid::IdTransaction for Txn<'_> {
    fn get(&self, k: autoid::AutoIdKey) -> autoid::Result<i64> {
        match self.0.Get(&kv::Context::default(), key(k), &[]) {
            Ok(v) => std::str::from_utf8(&v.Value)
                .map_err(storage_error)?
                .parse()
                .map_err(storage_error),
            Err(e) if kv::IsErrNotFound(&e) => Ok(0),
            Err(e) => Err(storage_error(e)),
        }
    }
    fn put(&mut self, k: autoid::AutoIdKey, v: i64) -> autoid::Result<()> {
        self.0
            .Set(key(k), v.to_string().into_bytes())
            .map_err(storage_error)
    }
    fn inc(&mut self, k: autoid::AutoIdKey, v: i64) -> autoid::Result<i64> {
        let n = self.get(k)?.wrapping_add(v);
        self.put(k, n)?;
        Ok(n)
    }
    fn copy_to(&mut self, a: autoid::AutoIdKey, b: autoid::AutoIdKey) -> autoid::Result<()> {
        let v = self.get(a)?;
        self.put(b, v)
    }
}
impl autoid::IdStore for Store {
    fn run_in_transaction(
        &self,
        op: &mut dyn FnMut(&mut dyn autoid::IdTransaction) -> autoid::Result<()>,
    ) -> autoid::Result<()> {
        self.0
            .with_storage(|s| {
                kv::RunInNewTxn(&kv::Context::default(), s, true, |_, t| {
                    op(&mut Txn(t)).map_err(|e| kv::errors::New(e.to_string()))
                })
            })
            .map_err(storage_error)
    }
}
impl autoid::Requirement for Store {
    fn store(&self) -> Arc<dyn autoid::IdStore> {
        Arc::new(Store(self.0.clone()))
    }
}
struct Requirement {
    store: Store,
    discover: Arc<autoid::ClientDiscover>,
    keyspace: u32,
}
impl autoid::Requirement for Requirement {
    fn store(&self) -> Arc<dyn autoid::IdStore> {
        Arc::new(Store(self.store.0.clone()))
    }
    fn single_point_allocator(
        &self,
        db: i64,
        table: i64,
        unsigned: bool,
    ) -> Option<Arc<dyn autoid::Allocator>> {
        Some(Arc::new(autoid::SinglePointAllocator::new(
            db,
            table,
            unsigned,
            self.keyspace,
            self.discover.clone(),
        )))
    }
}
struct MockStore {
    store: Store,
    uuid: String,
}
impl autoid::IdStore for MockStore {
    fn run_in_transaction(
        &self,
        op: &mut dyn FnMut(&mut dyn autoid::IdTransaction) -> autoid::Result<()>,
    ) -> autoid::Result<()> {
        self.store.run_in_transaction(op)
    }
}
impl astersql_autoid_service::AutoIdStorage for MockStore {
    fn uuid(&self) -> &str {
        &self.uuid
    }
    fn keyspace_id(&self) -> u32 {
        autoid::NULLSPACE_ID
    }
    fn etcd_namespace(&self) -> String {
        String::new()
    }
}
struct UnusedDiscovery;
impl autoid::LeaderDiscovery for UnusedDiscovery {
    fn leader(&self, _: &autoid::Context, _: &str) -> autoid::Result<Option<String>> {
        Err(storage_error("mockstore client must be seeded"))
    }
}
struct UnusedConnector;
impl autoid::AutoIdClientConnector for UnusedConnector {
    fn connect(
        &self,
        _: &str,
    ) -> autoid::Result<(
        Arc<dyn autoid::AutoIdClient>,
        Arc<dyn autoid::ClientConnection>,
    )> {
        Err(storage_error("mockstore client must be seeded"))
    }
}
fn requirement(domain: &Domain) -> Result<Requirement, String> {
    let storage = domain.storage_handle();
    let (name, uuid, keyspace, endpoints) =
        storage.with_storage(|s| (s.Name(), s.UUID(), s.DDLKeyspaceID(), s.DDLPDEndpoints()));
    let keyspace = keyspace.map_err(|e| e.to_string())?;
    let discover = if name == "mock-storage" {
        let d = Arc::new(autoid::ClientDiscover::new(
            Arc::new(UnusedDiscovery),
            Arc::new(UnusedConnector),
        ));
        d.seed_client_for_test(Arc::new(astersql_autoid_service::mock_for_test(Arc::new(
            MockStore {
                store: Store(storage.clone()),
                uuid,
            },
        ))));
        d
    } else {
        let security = &astersql_config::get_global_config().security;
        let tls = if security.cluster_ssl_ca.is_empty() {
            None
        } else {
            Some(astersql_autoid_service::client::ClientTls {
                ca: std::fs::read(&security.cluster_ssl_ca).map_err(|e| e.to_string())?,
                cert: read_optional_tls(&security.cluster_ssl_cert)?,
                key: read_optional_tls(&security.cluster_ssl_key)?,
            })
        };
        let namespace = if keyspace == autoid::NULLSPACE_ID {
            String::new()
        } else {
            format!("/keyspaces/tidb/{keyspace}")
        };
        astersql_autoid_service::client::client_discover(
            endpoints.map_err(|e| e.to_string())?,
            tls,
            namespace,
        )
        .map_err(|e| e.to_string())?
    };
    Ok(Requirement {
        store: Store(storage),
        discover,
        keyspace,
    })
}
pub(super) fn rebase_ids(domain: &Domain, schema: i64, t: &TableInfo) -> Result<(), String> {
    let info = autoid::TableInfo {
        id: t.ID,
        version: t.Version,
        pk_is_handle: t.PKIsHandle,
        is_common_handle: t.IsCommonHandle,
        has_auto_increment_column: t.GetAutoIncrementColInfo().is_some(),
        auto_increment_unsigned: t.IsAutoIncColUnsigned(),
        auto_id_cache: t.AutoIDCache,
        separate_auto_increment: t.SepAutoInc(),
        auto_random_bits: t.AutoRandomBits,
        auto_random_unsigned: t.IsAutoRandomBitColUnsigned(),
        ..Default::default()
    };
    let single_point = info.auto_id_cache == 1
        && info.version >= 5
        && info.has_auto_increment_column
        && info.separate_auto_increment;
    let remote;
    let local = Store(domain.storage_handle());
    let req: &dyn autoid::Requirement = if single_point {
        remote = requirement(domain)?;
        &remote
    } else {
        &local
    };
    let allocs = autoid::new_allocators_from_table_info(req, schema, &info);
    let mut bases = Vec::new();
    if t.AutoIncID > 1 {
        bases.push((
            if t.SepAutoInc() {
                autoid::AllocatorType::AutoIncrement
            } else {
                autoid::AllocatorType::RowId
            },
            t.AutoIncID - 1,
        ));
    }
    if t.AutoIncIDExtra != 0 {
        bases.push((autoid::AllocatorType::RowId, t.AutoIncIDExtra - 1));
    }
    if t.AutoRandID > 1 {
        bases.push((autoid::AllocatorType::AutoRandom, t.AutoRandID - 1));
    }
    for (tp, base) in bases {
        if let Some(a) = allocs.get(tp) {
            a.rebase(&autoid::Context::default(), base, false)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
pub(super) fn check_columnar(domain: &Domain, t: &TableInfo) -> Result<(), String> {
    astersql_planner_core::InstallPlannerExpressionFactory().map_err(|e| e.to_string())?;
    if !t.TiFlashReplica.as_ref().is_some_and(|r| r.Count > 0)
        || !astersql_config::get_global_config()
            .cse
            .is_columnar_store_enabled()
    {
        return Ok(());
    }
    let ks = domain.storage_handle().with_storage(|s| s.GetKeyspace());
    let v = domain
        .global_system_variable("tidb_columnar_storage_enabled")
        .ok_or_else(|| {
            astersql_util_dbterror::ErrTiFlashColumnarStorageCheckFailed
                .GenWithStackByArgs(&[ks.clone().into()])
                .to_string()
        })?;
    if v.eq_ignore_ascii_case("ON") || v == "1" {
        return Ok(());
    }
    if t.Indices.iter().any(|i| i.IsColumnarIndex()) {
        return Err(astersql_util_dbterror::ErrUnsupportedAddColumnarIndex
            .GenWithStackByArgs(&["Columnar Storage is not enabled".into()])
            .to_string());
    }
    Err(astersql_util_dbterror::ErrTiFlashColumnarStorageNotEnabled
        .GenWithStackByArgs(&[ks.into(), v.into()])
        .to_string())
}
pub(super) fn create_affinity(domain: &Domain, t: &TableInfo) -> Result<(), String> {
    let Some(a) = &t.Affinity else {
        return Ok(());
    };
    let physical: Vec<(String, i64)> = match a.Level.as_str() {
        "table" => vec![(format!("_tidb_t_{}", t.ID), t.ID)],
        "partition" => t
            .GetPartitionInfo()
            .map(|p| &p.Definitions)
            .filter(|d| !d.is_empty())
            .ok_or_else(|| {
                format!(
                    "partition affinity requires partition definitions for table {} (ID: {})",
                    t.Name.O, t.ID
                )
            })?
            .iter()
            .map(|d| (format!("_tidb_pt_{}_p{}", t.ID, d.ID), d.ID))
            .collect(),
        other => return Err(format!("invalid affinity level: {other}")),
    };
    let mut groups = std::collections::HashMap::new();
    for (id, p) in physical {
        let start = astersql_tablecodec::GenTablePrefix(p);
        let end = astersql_tablecodec::GenTablePrefix(p.wrapping_add(1));
        let (start, end) = domain
            .storage_handle()
            .with_storage(|s| s.EncodeDDLRegionRange(&start.0, &end.0))
            .map_err(|e| e.to_string())?;
        groups.insert(
            id,
            vec![astersql_domain_affinity::AffinityGroupKeyRange::new(
                start, end,
            )],
        );
    }
    if domain.storage_handle().with_storage(|s| s.Name()) == "mock-storage" {
        astersql_domain_affinity::create_groups_if_not_exists(
            &astersql_domain_affinity::BackgroundContext,
            &groups,
        )
        .map_err(|e| e.to_string())
    } else {
        let client = pd_client(domain)?;
        astersql_domain_affinity::new_pd_manager(Arc::new(client))
            .create_affinity_groups_if_not_exists(
                &astersql_domain_affinity::BackgroundContext,
                &groups,
            )
            .map_err(|e| e.to_string())
    }
}

fn read_optional_tls(path: &str) -> Result<Vec<u8>, String> {
    if path.is_empty() {
        Ok(Vec::new())
    } else {
        std::fs::read(path).map_err(|e| e.to_string())
    }
}
fn pd_client(domain: &Domain) -> Result<astersql_domain_affinity::http_client::HttpClient, String> {
    let endpoints = domain
        .storage_handle()
        .with_storage(|s| s.DDLPDEndpoints())
        .map_err(|e| e.to_string())?;
    let c = astersql_config::get_global_config();
    let tls = if c.security.cluster_ssl_ca.is_empty() {
        None
    } else {
        Some((
            std::fs::read(&c.security.cluster_ssl_ca).map_err(|e| e.to_string())?,
            read_optional_tls(&c.security.cluster_ssl_cert)?,
            read_optional_tls(&c.security.cluster_ssl_key)?,
        ))
    };
    astersql_domain_affinity::http_client::HttpClient::new(
        endpoints,
        tls.as_ref()
            .map(|(a, b, c)| (a.as_slice(), b.as_slice(), c.as_slice())),
    )
    .map_err(|e| e.to_string())
}
pub(super) fn put_bundles(
    domain: &Domain,
    bundles: &[astersql_ddl_placement::Bundle],
) -> Result<(), String> {
    if bundles.is_empty() {
        return Ok(());
    }
    if domain.storage_handle().with_storage(|s| s.Name()) == "mock-storage" {
        return astersql_domain_infosync::PutRuleBundlesWithDefaultRetry(bundles)
            .map_err(|e| e.to_string());
    }
    let pd = pd_client(domain)?;
    let value = serde_json::to_value(bundles).map_err(|e| e.to_string())?;
    let mut last = String::new();
    for attempt in 0..=astersql_domain_infosync::RequestPDMaxRetry {
        match pd.request_json(
            &astersql_domain_affinity::BackgroundContext,
            reqwest::Method::POST,
            "/pd/api/v1/config/placement-rule?partial=true",
            Some(value.clone()),
        ) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        if attempt < astersql_domain_infosync::RequestPDMaxRetry {
            std::thread::sleep(astersql_domain_infosync::RequestRetryInterval);
        }
    }
    Err(last)
}
fn replica_rule(
    domain: &Domain,
    id: i64,
    replica: &astersql_meta_model::TiFlashReplicaInfo,
) -> Result<serde_json::Value, String> {
    let start = astersql_tablecodec::GenTableRecordPrefix(id);
    let end = astersql_tablecodec::GenTablePrefix(id.wrapping_add(1));
    let (start, end) = domain
        .storage_handle()
        .with_storage(|s| s.EncodeDDLRegionRange(&start.0, &end.0))
        .map_err(|e| e.to_string())?;
    let keyspace = domain
        .storage_handle()
        .with_storage(|s| s.DDLKeyspaceID())
        .map_err(|e| e.to_string())?;
    let id = if keyspace == u32::MAX {
        format!("table-{id}-r")
    } else {
        format!("keyspace-{keyspace}-table-{id}-r")
    };
    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(
        serde_json::json!({"group_id":"tiflash","id":id,"index":120,"start_key":hex(&start),"end_key":hex(&end),"role":"learner","count":replica.Count,"label_constraints":[{"key":"engine","op":"in","values":["tiflash"]}],"location_labels":replica.LocationLabels}),
    )
}
pub(super) fn configure_replica(domain: &Domain, t: &TableInfo) -> Result<(), String> {
    if !astersql_config::get_global_config()
        .cse
        .is_tiflash_enabled()
    {
        return Ok(());
    }
    let Some(r) = &t.TiFlashReplica else {
        return Ok(());
    };
    if domain.storage_handle().with_storage(|s| s.Name()) == "mock-storage" {
        return if let Some(p) = t.GetPartitionInfo() {
            astersql_domain_infosync::ConfigureTiFlashPDForPartitions(
                false,
                &p.Definitions,
                r.Count,
                &r.LocationLabels,
                t.ID,
            )
            .and_then(|_| {
                astersql_domain_infosync::ConfigureTiFlashPDForPartitions(
                    true,
                    &p.AddingDefinitions,
                    r.Count,
                    &r.LocationLabels,
                    t.ID,
                )
            })
        } else {
            astersql_domain_infosync::ConfigureTiFlashPDForTable(t.ID, r.Count, &r.LocationLabels)
        }
        .map_err(|e| e.to_string());
    }
    let pd = pd_client(domain)?;
    let ctx = astersql_domain_affinity::BackgroundContext;
    let group = pd
        .request_json(
            &ctx,
            reqwest::Method::GET,
            "/pd/api/v1/config/rule_group/tiflash",
            None,
        )
        .map_err(|e| e.to_string())?;
    if group["index"] != astersql_ddl_placement::RuleIndexTiFlash || group["override"] != false {
        pd.request_json(&ctx,reqwest::Method::POST,"/pd/api/v1/config/rule_group",Some(serde_json::json!({"id":"tiflash","index":astersql_ddl_placement::RuleIndexTiFlash,"override":false}))).map_err(|e|e.to_string())?;
    }
    if let Some(p) = t.GetPartitionInfo() {
        for (accel, defs) in [(false, &p.Definitions), (true, &p.AddingDefinitions)] {
            let mut ops = Vec::new();
            let mut ranges = Vec::new();
            for d in defs {
                let mut rule = replica_rule(domain, d.ID, r)?;
                rule["action"] = serde_json::json!(if r.Count == 0 { "del" } else { "add" });
                ops.push(rule);
                let start = astersql_tablecodec::GenTableRecordPrefix(d.ID);
                let end = astersql_tablecodec::GenTablePrefix(d.ID.wrapping_add(1));
                let (start, end) = domain
                    .storage_handle()
                    .with_storage(|s| s.EncodeDDLRegionRange(&start.0, &end.0))
                    .map_err(|e| e.to_string())?;
                let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
                ranges.push(serde_json::json!({"start_key":hex(&start),"end_key":hex(&end)}));
            }
            pd.request_json(
                &ctx,
                reqwest::Method::POST,
                "/pd/api/v1/config/rules/batch",
                Some(serde_json::json!(ops)),
            )
            .map_err(|e| e.to_string())?;
            if accel && !ranges.is_empty() {
                pd.request_json(
                    &ctx,
                    reqwest::Method::POST,
                    "/pd/api/v1/regions/accelerate-schedule/batch",
                    Some(serde_json::json!(ranges)),
                )
                .map_err(|e| e.to_string())?;
            }
        }
    } else {
        let rule = replica_rule(domain, t.ID, r)?;
        if r.Count == 0 {
            pd.request_json(
                &ctx,
                reqwest::Method::DELETE,
                &format!(
                    "/pd/api/v1/config/rule/tiflash/{}",
                    rule["id"].as_str().unwrap()
                ),
                None,
            )
        } else {
            pd.request_json(
                &ctx,
                reqwest::Method::POST,
                "/pd/api/v1/config/rule",
                Some(rule),
            )
        }
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
