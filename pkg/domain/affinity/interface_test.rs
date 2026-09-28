// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use astersql_domain_affinity::interface::{
    create_groups_if_not_exists, delete_groups, delete_groups_with_retry, get_all_group_states,
    get_groups, init_manager,
};
use astersql_domain_affinity::manager::{
    AffinityError, AffinityGroupKeyRange, AffinityGroupState, Context, PdClient,
};

pub(crate) static PACKAGE_STATE_TEST_LOCK: Mutex<()> = Mutex::new(());
static LOG_RECORDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static INSTALL_LOGGER: Once = Once::new();

struct RecordingLogger;

impl log::Log for RecordingLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            LOG_RECORDS
                .lock()
                .unwrap()
                .push((record.target().to_owned(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

static RECORDING_LOGGER: RecordingLogger = RecordingLogger;

pub(crate) fn lock_package_state() -> MutexGuard<'static, ()> {
    PACKAGE_STATE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn reset_log_records() {
    INSTALL_LOGGER.call_once(|| {
        log::set_logger(&RECORDING_LOGGER).expect("test logger must install once");
        log::set_max_level(log::LevelFilter::Error);
    });
    LOG_RECORDS.lock().unwrap().clear();
}

struct TestContext {
    cancelled: bool,
    deadline: Option<Instant>,
}

impl Context for TestContext {
    fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[derive(Default)]
struct ContextRecordingClient {
    observations: Mutex<Vec<(&'static str, bool, Option<Instant>)>>,
}

impl ContextRecordingClient {
    fn record(&self, operation: &'static str, ctx: &dyn Context) {
        self.observations
            .lock()
            .unwrap()
            .push((operation, ctx.is_cancelled(), ctx.deadline()));
    }
}

impl PdClient for ContextRecordingClient {
    fn create_affinity_groups(
        &self,
        ctx: &dyn Context,
        _groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
        _skip_exist_check: bool,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        self.record("create", ctx);
        Ok(HashMap::new())
    }

    fn batch_delete_affinity_groups(
        &self,
        ctx: &dyn Context,
        _ids: &[String],
        _force: bool,
    ) -> Result<(), AffinityError> {
        self.record("delete", ctx);
        Ok(())
    }

    fn get_affinity_groups(
        &self,
        ctx: &dyn Context,
        _ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        self.record("get", ctx);
        Ok(HashMap::new())
    }

    fn get_all_affinity_groups(
        &self,
        ctx: &dyn Context,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        self.record("get_all", ctx);
        Ok(HashMap::new())
    }
}

#[derive(Default)]
struct AlwaysFailDeleteClient {
    delete_calls: AtomicUsize,
}

impl PdClient for AlwaysFailDeleteClient {
    fn create_affinity_groups(
        &self,
        _ctx: &dyn Context,
        _groups: &HashMap<String, Vec<AffinityGroupKeyRange>>,
        _skip_exist_check: bool,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        Ok(HashMap::new())
    }

    fn batch_delete_affinity_groups(
        &self,
        _ctx: &dyn Context,
        _ids: &[String],
        _force: bool,
    ) -> Result<(), AffinityError> {
        let attempt = self.delete_calls.fetch_add(1, Ordering::SeqCst) + 1;
        Err(AffinityError::new(format!("attempt-{attempt}")))
    }

    fn get_affinity_groups(
        &self,
        _ctx: &dyn Context,
        _ids: &[String],
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        Ok(HashMap::new())
    }

    fn get_all_affinity_groups(
        &self,
        _ctx: &dyn Context,
    ) -> Result<HashMap<String, AffinityGroupState>, AffinityError> {
        Ok(HashMap::new())
    }
}

#[test]
fn package_api_forwards_context_to_every_pd_operation() {
    let _guard = lock_package_state();
    let client = Arc::new(ContextRecordingClient::default());
    init_manager(Some(client.clone()));
    let deadline = Instant::now() + Duration::from_secs(30);
    let ctx = TestContext {
        cancelled: true,
        deadline: Some(deadline),
    };
    let groups = HashMap::from([(
        "g1".to_owned(),
        vec![AffinityGroupKeyRange::new(b"a", b"b")],
    )]);
    let ids = vec!["g1".to_owned()];

    create_groups_if_not_exists(&ctx, &groups).unwrap();
    delete_groups(&ctx, &ids).unwrap();
    get_groups(&ctx, &ids).unwrap();
    get_all_group_states(&ctx).unwrap();

    assert_eq!(
        *client.observations.lock().unwrap(),
        vec![
            ("create", true, Some(deadline)),
            ("delete", true, Some(deadline)),
            ("get", true, Some(deadline)),
            ("get_all", true, Some(deadline)),
        ]
    );
    init_manager(None);
}

#[test]
fn final_delete_failure_is_logged_once_with_error_and_group_ids() {
    let _guard = lock_package_state();
    reset_log_records();
    let client = Arc::new(AlwaysFailDeleteClient::default());
    init_manager(Some(client.clone()));
    let ids = vec!["g1".to_owned(), "g2".to_owned()];

    let error = delete_groups_with_retry(
        &TestContext {
            cancelled: false,
            deadline: None,
        },
        &ids,
    )
    .unwrap_err();

    init_manager(None);
    assert_eq!(error.to_string(), "attempt-4");
    assert_eq!(client.delete_calls.load(Ordering::SeqCst), 4);
    let records = LOG_RECORDS.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, "astersql_domain_affinity");
    assert!(
        records[0]
            .1
            .contains("Failed to delete affinity groups after retries")
    );
    assert!(records[0].1.contains("error=attempt-4"));
    assert!(records[0].1.contains("groupIDs=[\"g1\", \"g2\"]"));
}
