// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use astersql_extworkload::{Manager, ManagerError, context, keyspacepb};

use super::CreateAnalyzeSession;

struct RecordingManager(Arc<Mutex<Vec<bool>>>, bool);

impl Manager for RecordingManager {
    fn Close(&mut self) -> Result<(), ManagerError> {
        Ok(())
    }
    fn Role(&self) -> String {
        astersql_config::RoleMaster.into()
    }
    fn Meta(&self) -> Option<&keyspacepb::KeyspaceMeta> {
        None
    }
    fn InitializeGCV2(
        &mut self,
        _: &context::Context,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        Ok(())
    }
    fn AbortGCV2(&mut self, _: &context::Context) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RegisterGCV2(
        &mut self,
        _: &context::Context,
        _: u64,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleGCV2(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn UpdateGCLifeTime(
        &mut self,
        _: &context::Context,
        _: std::time::Duration,
    ) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RegisterTTLTask(
        &mut self,
        _: &context::Context,
        _: i64,
        _: bool,
    ) -> Result<(), ManagerError> {
        Ok(())
    }
    fn DeleteTTLTableInfo(&mut self, _: &context::Context, _: i64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleTTLTask(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn UpdateTTLJobEnable(
        &mut self,
        _: &context::Context,
        enabled: bool,
    ) -> Result<(), ManagerError> {
        self.0.lock().unwrap().push(enabled);
        if self.1 {
            Err(std::io::Error::other("TTL controller unavailable").into())
        } else {
            Ok(())
        }
    }
    fn RegisterAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
}

#[test]
fn go_merge_43_set_global_ttl_enable_forwards_to_master_controller() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    domain.set_external_workload_manager(Some(Box::new(RecordingManager(
        Arc::clone(&changes),
        false,
    ))));
    session
        .execute("SET GLOBAL tidb_ttl_job_enable = OFF")
        .unwrap();
    assert_eq!(*changes.lock().unwrap(), [false]);
    assert!(!astersql_sessionctx_vardef::EnableTTLJob.Load());
    session
        .execute("SET GLOBAL tidb_ttl_job_enable = ON")
        .unwrap();
    assert_eq!(*changes.lock().unwrap(), [false, true]);
    assert!(astersql_sessionctx_vardef::EnableTTLJob.Load());
    session
        .execute("SET GLOBAL tidb_ttl_job_enable = OFF")
        .unwrap();
    domain.notify_update_sysvar_cache(true);
    assert_eq!(*changes.lock().unwrap(), [false, true, false]);
    assert!(!astersql_sessionctx_vardef::EnableTTLJob.Load());
    domain.set_external_workload_manager(Some(Box::new(RecordingManager(
        Arc::clone(&changes),
        true,
    ))));
    let error = session
        .execute("SET GLOBAL tidb_ttl_job_enable = ON")
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("TTL controller unavailable"),
        "{error}"
    );
    assert_eq!(*changes.lock().unwrap(), [false, true, false, true]);
    assert!(!astersql_sessionctx_vardef::EnableTTLJob.Load());
    astersql_sessionctx_vardef::EnableTTLJob.Store(true);
}

struct Gcv2Manager {
    role: String,
    meta: keyspacepb::KeyspaceMeta,
    calls: Arc<Mutex<Vec<std::time::Duration>>>,
    events: Arc<Mutex<Vec<String>>>,
    fail_action: Option<&'static str>,
}
impl Manager for Gcv2Manager {
    fn Close(&mut self) -> Result<(), ManagerError> {
        self.events.lock().unwrap().push("close".into());
        Ok(())
    }
    fn Role(&self) -> String {
        self.role.clone()
    }
    fn Meta(&self) -> Option<&keyspacepb::KeyspaceMeta> {
        Some(&self.meta)
    }
    fn InitializeGCV2(
        &mut self,
        _: &context::Context,
        life: std::time::Duration,
    ) -> Result<(), ManagerError> {
        self.calls.lock().unwrap().push(life);
        self.events.lock().unwrap().push("init".into());
        if self.fail_action == Some("init") {
            Err(std::io::Error::other("init failed").into())
        } else {
            Ok(())
        }
    }
    fn AbortGCV2(&mut self, _: &context::Context) -> Result<(), ManagerError> {
        self.events.lock().unwrap().push("abort".into());
        if self.fail_action == Some("abort") {
            Err(std::io::Error::other("abort failed").into())
        } else {
            Ok(())
        }
    }
    fn RegisterGCV2(
        &mut self,
        _: &context::Context,
        _: u64,
        life: std::time::Duration,
    ) -> Result<(), ManagerError> {
        self.calls.lock().unwrap().push(life);
        Ok(())
    }
    fn RecycleGCV2(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn UpdateGCLifeTime(
        &mut self,
        _: &context::Context,
        life: std::time::Duration,
    ) -> Result<(), ManagerError> {
        self.calls.lock().unwrap().push(life);
        self.events.lock().unwrap().push("update".into());
        if self.fail_action == Some("update") {
            Err(std::io::Error::other("update failed").into())
        } else {
            Ok(())
        }
    }
    fn RegisterTTLTask(
        &mut self,
        _: &context::Context,
        _: i64,
        _: bool,
    ) -> Result<(), ManagerError> {
        Ok(())
    }
    fn DeleteTTLTableInfo(&mut self, _: &context::Context, _: i64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleTTLTask(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn UpdateTTLJobEnable(
        &mut self,
        _: &context::Context,
        enabled: bool,
    ) -> Result<(), ManagerError> {
        let _ = enabled;
        Ok(())
    }
    fn RegisterAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleAutoAnalyze(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
}

#[test]
fn set_global_gc_lifetime_notifies_effective_value() {
    for (role, value, expected, keyspace_level) in [
        ("master", "24h", 86400, true),
        ("gcv2", "24h", 86400, true),
        ("ttl", "24h", 86400, true),
        ("auto-analyze", "24h", 86400, true),
        ("master", "1m", 600, true),
        ("master", "24h", 86400, false),
    ] {
        let (domain, session) = CreateAnalyzeSession().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        domain.set_external_workload_manager(Some(Box::new(Gcv2Manager {
            role: role.into(),
            meta: keyspacepb::KeyspaceMeta {
                config: [(
                    "gc_management_type".into(),
                    if keyspace_level {
                        "keyspace_level"
                    } else {
                        "unified"
                    }
                    .into(),
                )]
                .into(),
                ..Default::default()
            },
            calls: calls.clone(),
            events: Default::default(),
            fail_action: None,
        })));
        session
            .execute(&format!("SET GLOBAL tidb_gc_life_time = '{value}'"))
            .unwrap();
        assert_eq!(
            if keyspace_level {
                vec![std::time::Duration::from_secs(expected)]
            } else {
                vec![]
            },
            *calls.lock().unwrap(),
            "{role}/{value}/{keyspace_level}"
        );
        domain.close();
    }
}

fn install_gcv2_manager(
    domain: &Arc<astersql_domain::Domain>,
    role: &str,
    keyspace_level: bool,
    fail_action: Option<&'static str>,
) -> (
    Arc<Mutex<Vec<std::time::Duration>>>,
    Arc<Mutex<Vec<String>>>,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    domain.set_external_workload_manager(Some(Box::new(Gcv2Manager {
        role: role.into(),
        meta: keyspacepb::KeyspaceMeta {
            config: [(
                "gc_management_type".into(),
                if keyspace_level {
                    "keyspace_level"
                } else {
                    "unified"
                }
                .into(),
            )]
            .into(),
            ..Default::default()
        },
        calls: calls.clone(),
        events: events.clone(),
        fail_action,
    })));
    (calls, events)
}

#[test]
fn initialize_gcv2_reads_effective_sql_lifetime_and_cleans_failed_manager() {
    for (role, keyspace_level, fail_action, invalid, expected_calls, removed) in [
        ("master", true, None, false, 1, false),
        ("gcv2", true, None, false, 0, false),
        ("master", false, None, false, 0, false),
        ("master", true, Some("init"), false, 1, true),
        ("master", true, None, true, 0, true),
    ] {
        let (domain, session) = CreateAnalyzeSession().unwrap();
        session
            .execute("SET GLOBAL tidb_gc_life_time = '24h'")
            .unwrap();
        if invalid {
            session.execute("UPDATE mysql.tidb SET VARIABLE_VALUE='invalid' WHERE VARIABLE_NAME='tikv_gc_life_time'").unwrap();
        }
        let (calls, events) = install_gcv2_manager(&domain, role, keyspace_level, fail_action);
        super::session::initialize_external_workload_gcv2(&domain);
        assert_eq!(expected_calls, calls.lock().unwrap().len());
        if expected_calls > 0 {
            assert_eq!(
                calls.lock().unwrap()[0],
                std::time::Duration::from_secs(86400)
            );
        }
        assert_eq!(removed, domain.external_workload_manager().is_none());
        assert_eq!(
            usize::from(removed),
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|v| *v == "close")
                .count()
        );
        domain.close();
    }
}

#[test]
fn gc_lifetime_notification_failure_keeps_successful_sql_setting() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let (calls, _) = install_gcv2_manager(&domain, "master", true, Some("update"));
    session
        .execute("SET GLOBAL tidb_gc_life_time = '1m'")
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec![std::time::Duration::from_secs(600)]
    );
    let mut results = session
        .execute("SELECT @@global.tidb_gc_life_time")
        .unwrap();
    assert_eq!(
        results[0].next_row().unwrap().unwrap(),
        vec!["10m0s".to_string()]
    );
    assert!(
        session
            .execute("SET GLOBAL tidb_gc_life_time = 'invalid'")
            .is_err()
    );
    assert!(
        session
            .execute("SET SESSION tidb_gc_life_time = '24h'")
            .is_err()
    );
    assert_eq!(1, calls.lock().unwrap().len());
    domain.close();
}

#[cfg(feature = "nextgen")]
struct RestoreDeployMode(astersql_config_deploymode::Mode);
#[cfg(feature = "nextgen")]
impl Drop for RestoreDeployMode {
    fn drop(&mut self) {
        astersql_config_deploymode::Set(self.0).unwrap();
    }
}

#[cfg(feature = "nextgen")]
#[test]
fn upgrade_gcv2_abort_uses_post_lock_version() {
    let _restore = RestoreDeployMode(astersql_config_deploymode::Get());
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let current = unsafe { crate::upgrade_def::currentBootstrapVersion };
    session
        .execute(&format!(
            "UPDATE mysql.tidb SET VARIABLE_VALUE='{}' WHERE VARIABLE_NAME='tidb_server_version'",
            current - 1
        ))
        .unwrap();
    let (_, events) = install_gcv2_manager(&domain, "gcv2", true, None);
    let lock = super::session::acquire_bootstrap_upgrade_lock(&domain, || {
        // Another node completes upgrade while this node waits for the external owner lock.
        session.execute(&format!("UPDATE mysql.tidb SET VARIABLE_VALUE='{current}' WHERE VARIABLE_NAME='tidb_server_version'")).unwrap();
        Ok(())
    }).unwrap();
    assert!(lock.is_some());
    super::BootstrapCanonicalDomain(domain.clone()).unwrap();
    assert!(!events.lock().unwrap().iter().any(|v| v == "abort"));
    domain.close();
}

#[cfg(feature = "nextgen")]
#[test]
fn upgrade_gcv2_aborts_only_when_upgrade_is_still_required() {
    let _restore = RestoreDeployMode(astersql_config_deploymode::Get());
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    for fail in [None, Some("abort")] {
        let (domain, session) = CreateAnalyzeSession().unwrap();
        let old = unsafe { crate::upgrade_def::currentBootstrapVersion } - 1;
        session.execute(&format!("UPDATE mysql.tidb SET VARIABLE_VALUE='{old}' WHERE VARIABLE_NAME='tidb_server_version'")).unwrap();
        let (_, events) = install_gcv2_manager(&domain, "gcv2", true, fail);
        let lock = super::session::acquire_bootstrap_upgrade_lock(&domain, || Ok(())).unwrap();
        assert!(lock.is_some());
        let result = super::BootstrapCanonicalDomain(domain.clone());
        assert_eq!(fail.is_some(), result.is_err());
        assert_eq!(
            1,
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|v| *v == "abort")
                .count()
        );
        let mut rows = session
            .execute(
                "SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME='tidb_server_version'",
            )
            .unwrap();
        assert_eq!(rows[0].next_row().unwrap().unwrap(), vec![old.to_string()]);
        domain.close();
    }
}

#[test]
fn external_controller_startup_failures_are_fatal_only_for_dedicated_gcv2() {
    for (role, missing_meta, keyspace_level, controller_failure, fatal, expected_create) in [
        ("gcv2", true, true, false, true, 0),
        ("master", true, true, false, false, 0),
        ("ttl", true, true, false, false, 0),
        ("gcv2", false, false, false, true, 0),
        ("gcv2", false, true, true, true, 1),
        ("master", false, true, true, false, 1),
        ("ttl", false, true, true, false, 1),
        ("master", false, false, false, false, 1),
        ("gcv2", false, true, false, false, 1),
        ("ttl", false, true, false, false, 1),
    ] {
        let (domain, _) = CreateAnalyzeSession().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let meta = if missing_meta {
            Err(super::SessionError::new("keyspace metadata unavailable"))
        } else {
            Ok(keyspacepb::KeyspaceMeta {
                config: [(
                    "gc_management_type".into(),
                    if keyspace_level {
                        "keyspace_level"
                    } else {
                        "unified"
                    }
                    .into(),
                )]
                .into(),
                ..Default::default()
            })
        };
        let mut create_count = 0;
        let result =
            super::session::install_external_workload_manager(&domain, role, meta, |meta| {
                create_count += 1;
                if controller_failure {
                    return Err(super::SessionError::new("controller unavailable"));
                }
                Ok(Some(Box::new(Gcv2Manager {
                    role: role.into(),
                    meta: meta.clone(),
                    calls: calls.clone(),
                    events: events.clone(),
                    fail_action: None,
                })))
            });
        assert_eq!(fatal, result.is_err());
        assert_eq!(expected_create, create_count);
        assert_eq!(
            !fatal && !missing_meta && !controller_failure,
            domain.external_workload_manager().is_some()
        );
        domain.close();
    }
}
