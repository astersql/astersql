// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use astersql_extworkload::{Manager, ManagerError, context, keyspacepb};

use super::CreateAnalyzeSession;

struct RecordingManager(Arc<Mutex<Vec<bool>>>);

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
    fn InitializeGCV2(&mut self, _: &context::Context) -> Result<(), ManagerError> {
        Ok(())
    }
    fn AbortGCV2(&mut self, _: &context::Context) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RegisterGCV2(&mut self, _: &context::Context, _: u64, _: i64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn RecycleGCV2(&mut self, _: &context::Context, _: u64) -> Result<(), ManagerError> {
        Ok(())
    }
    fn UpdateGCLifeTime(&mut self, _: &context::Context, _: i64) -> Result<(), ManagerError> {
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
fn go_merge_43_set_global_ttl_enable_forwards_to_master_controller() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    domain.set_external_workload_manager(Some(Box::new(RecordingManager(Arc::clone(&changes)))));
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
}
