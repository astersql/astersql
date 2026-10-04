// Copyright 2026 AsterSQL.

use super::runaway::{
    ResourceControllerConfig, ResourceGroupController, ResourceGroupRuntime, RunawayManager,
};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Controller {
    started: AtomicBool,
    config: ResourceControllerConfig,
}

impl ResourceGroupController for Controller {
    fn start(&self) {
        self.started.store(true, Ordering::SeqCst);
    }

    fn config(&self) -> &ResourceControllerConfig {
        &self.config
    }
}

struct Manager;

impl RunawayManager for Manager {}

#[test]
fn initialize_starts_only_the_resource_controller_like_go() {
    let controller = Arc::new(Controller {
        started: AtomicBool::new(false),
        config: ResourceControllerConfig {
            server_id: 42,
            advertised_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 4000,
            request_unit_mode: true,
        },
    });
    let runtime = ResourceGroupRuntime {
        controller: controller.clone(),
        runaway_manager: Arc::new(Manager),
    };

    let initialized = runtime.initialize();

    assert!(controller.started.load(Ordering::SeqCst));
    assert_eq!(initialized.controller.config().server_id, 42);
}

use super::runaway::{ControllerResourceGroupCatalog, LookupError, ResourceGroupProvider};
use astersql_resourcegroup_runaway as runaway;
use std::sync::{Mutex, atomic::AtomicUsize};
use tikv_client::{
    proto::{meta_storagepb as meta, resource_manager as rm},
    resource_group_lookup::{RuVersionPolicy, ServerConfig, TokenRpcParams},
};
struct ProviderStub {
    base: Arc<dyn ResourceGroupProvider>,
    config: ServerConfig,
    response: Mutex<Result<Option<rm::ResourceGroup>, LookupError>>,
    calls: AtomicUsize,
}
impl ResourceGroupProvider for ProviderStub {
    fn get(&self, key: &[u8]) -> Result<meta::GetResponse, LookupError> {
        Ok(meta::GetResponse {
            header: Some(meta::ResponseHeader::default()),
            kvs: vec![meta::KeyValue {
                key: key.into(),
                value: serde_json::to_vec(&self.config).unwrap(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }
    fn put(&self, key: &[u8], value: &[u8]) -> Result<meta::PutResponse, LookupError> {
        self.base.put(key, value)
    }
    fn get_resource_group(&self, _name: &str) -> Result<Option<rm::ResourceGroup>, LookupError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.response.lock().unwrap().clone()
    }
}
fn transient_provider() -> Arc<ProviderStub> {
    let failure = LookupError::GetResourceGroup {
        name: "test-group".into(),
        cause: Box::new(LookupError::Rpc(tonic::Status::unavailable(
            "resource manager unavailable",
        ))),
    };
    Arc::new(ProviderStub {
        base: astersql_domain_infosync::NewMockResourceGroupProvider(0),
        config: ServerConfig {
            token_rpc_params: TokenRpcParams {
                wait_retry_interval: "250ms".into(),
                wait_retry_times: 4,
            },
            ru_version_policy: Some(RuVersionPolicy {
                default: 2,
                ..Default::default()
            }),
            ..Default::default()
        },
        response: Mutex::new(Err(failure)),
        calls: AtomicUsize::new(0),
    })
}
fn lookup_domain() -> crate::Domain {
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    crate::Domain::new_mock(
        Arc::try_unwrap(storage).ok().unwrap(),
        Arc::new(crate::KvInfoSchemaLoader::default()),
    )
}
#[test]
fn starter_degraded_resource_group_recovers_without_caching() {
    let domain = lookup_domain();
    let provider = transient_provider();
    domain
        .init_resource_groups_controller(Some(provider.clone()), 0, true, true)
        .unwrap();
    let controller = domain.resource_group_lookup_controller().unwrap();
    assert_eq!(domain.ru_version(), 2);
    let group = controller.get_resource_group("test-group").unwrap();
    assert_eq!(group.name, "test-group");
    assert_eq!(group.mode, rm::GroupMode::RuMode as i32);
    assert_eq!(
        group.r_u_settings,
        Some(crate::resource_group_controller_options::new_default_degraded_ru_settings())
    );
    let mut real = group;
    real.r_u_settings
        .as_mut()
        .unwrap()
        .r_u
        .as_mut()
        .unwrap()
        .settings
        .as_mut()
        .unwrap()
        .fill_rate = 1;
    *provider.response.lock().unwrap() = Ok(Some(real.clone()));
    assert_eq!(controller.get_resource_group("test-group").unwrap(), real);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    domain.close();
    assert!(domain.resource_group_lookup_controller().is_none());
}
#[test]
fn resource_group_options_preserve_provider_settings_outside_enabled_starter() {
    for (is_starter, enabled) in [(true, true), (true, false), (false, true)] {
        let domain = lookup_domain();
        let provider = transient_provider();
        domain
            .init_resource_groups_controller(Some(provider), 0, is_starter, enabled)
            .unwrap();
        let controller = domain.resource_group_lookup_controller().unwrap();
        let config = controller.config();
        assert_eq!(config.max_wait_duration, std::time::Duration::from_secs(30));
        if is_starter && enabled {
            assert_eq!(
                config.wait_retry_interval,
                std::time::Duration::from_millis(100)
            );
            assert_eq!(config.wait_retry_times, 20);
            assert_eq!(
                config.degraded_mode_wait_duration,
                std::time::Duration::from_millis(1500)
            );
        } else {
            assert_eq!(
                config.wait_retry_interval,
                std::time::Duration::from_millis(250)
            );
            assert_eq!(config.wait_retry_times, 4);
            assert_eq!(
                config.degraded_mode_wait_duration,
                std::time::Duration::ZERO
            );
            assert!(controller.get_resource_group("test-group").is_err());
        }
        domain.close();
    }
}
#[test]
fn starter_runaway_switch_group_accepts_degraded_lookup() {
    let domain = lookup_domain();
    let provider = transient_provider();
    domain
        .init_resource_groups_controller(Some(provider.clone()), 0, true, true)
        .unwrap();
    let catalog = Arc::new(ControllerResourceGroupCatalog(
        domain.resource_group_lookup_controller().unwrap(),
    ));
    let manager = runaway::manager::Manager::NewRunawayManager(
        catalog,
        "127.0.0.1:4000",
        Arc::new(runaway::NoopExecutor),
        Arc::new(runaway::syncer::AllSystemTables),
    );
    domain.bind_runaway_manager(Some(Arc::new(manager.clone())));
    let checker = runaway::checker::Checker::NewChecker(
        manager.clone(),
        "source-group".into(),
        Some(runaway::RunawaySettings {
            action: runaway::RunawayAction::SwitchGroup,
            switch_group_name: "target-switch-group".into(),
            rule: runaway::RunawayRule {
                processed_keys: 1,
                ..Default::default()
            },
            ..Default::default()
        }),
        "SELECT 1".into(),
        "sql_digest".into(),
        "plan_digest".into(),
        runaway::nowMicros(),
    );
    assert!(checker.CheckThresholds(None, 10, None).is_none());
    let mut request = runaway::CopRequest::default();
    checker.BeforeCopRequest(&mut request).unwrap();
    assert_eq!(request.resource_group_name, "target-switch-group");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    manager.Stop();
    domain.close();
}
#[test]
fn controller_initialization_skips_non_pd_storage() {
    let domain = lookup_domain();
    domain
        .init_resource_groups_controller(None, 0, true, true)
        .unwrap();
    assert!(domain.resource_group_lookup_controller().is_none());
    domain.close();
}
