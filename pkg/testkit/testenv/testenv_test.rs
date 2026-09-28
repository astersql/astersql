// Copyright 2026 AsterSQL.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use astersql_config as config;

use crate::{MaxProcsForTest, SetGOMAXPROCSForTest, TestContext, UpdateConfigForNextgen};

static GLOBAL_CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct RecordingContext {
    helper_called: AtomicBool,
    cleanups: Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>,
}

impl RecordingContext {
    fn run_cleanups(&self) {
        while let Some(cleanup) = self.cleanups.lock().unwrap().pop() {
            cleanup();
        }
    }
}

impl TestContext for RecordingContext {
    fn helper(&self) {
        self.helper_called.store(true, Ordering::SeqCst);
    }

    fn cleanup(&self, cleanup: Box<dyn FnOnce() + Send + 'static>) {
        self.cleanups.lock().unwrap().push(cleanup);
    }
}

#[test]
fn test_parallelism_cap_is_positive_and_at_most_sixteen() {
    SetGOMAXPROCSForTest();
    assert!((1..=16).contains(&MaxProcsForTest()));
}

#[test]
fn update_config_for_nextgen_updates_and_restores_the_go_config_fields() {
    let _guard = GLOBAL_CONFIG_LOCK.lock().unwrap();
    let original = config::get_global_config().as_ref().clone();
    let mut initial = original.clone();
    initial.keyspace_name = "before-keyspace".to_owned();
    initial.instance.tidb_service_scope = "before-scope".to_owned();
    config::store_global_config(initial.clone());

    let context = RecordingContext::default();
    UpdateConfigForNextgen(&context);

    assert!(context.helper_called.load(Ordering::SeqCst));
    let updated = config::get_global_config();
    assert_eq!(updated.keyspace_name, "SYSTEM");
    assert_eq!(updated.instance.tidb_service_scope, "dxf_service");

    context.run_cleanups();
    let restored = config::get_global_config();
    assert_eq!(restored.keyspace_name, initial.keyspace_name);
    assert_eq!(
        restored.instance.tidb_service_scope,
        initial.instance.tidb_service_scope
    );
    config::store_global_config(original);
}
