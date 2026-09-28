// Copyright 2026 AsterSQL.

use crate::analyze_worker::{
    AnalyzeSaveError, AnalyzeSaveResult, analyzeContext, analyzeJob, analyzeResults,
    analyzeSaveStatsRuntime, newAnalyzeSaveStatsWorker, statsMetaHistorySource,
};
use std::sync::{Arc, Mutex, mpsc};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    KillCheck,
    Save(i64, bool, statsMetaHistorySource),
    Finish(u64, Option<String>),
    Destroy(i64),
    SaveWarning(String),
    WorkerPanic(String),
    ClosedErrorChannel(String),
}

#[derive(Default)]
struct TestRuntime {
    events: Mutex<Vec<Event>>,
    kill_at_check: Option<usize>,
    save_error_table: Option<i64>,
    panic_on_save: bool,
}

impl TestRuntime {
    fn record(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl analyzeSaveStatsRuntime for TestRuntime {
    fn handle_kill_signal(&self) -> AnalyzeSaveResult {
        let check = {
            let mut events = self.events.lock().unwrap();
            let check = events
                .iter()
                .filter(|event| matches!(event, Event::KillCheck))
                .count()
                + 1;
            events.push(Event::KillCheck);
            check
        };
        if self.kill_at_check == Some(check) {
            Err(AnalyzeSaveError("query interrupted".into()))
        } else {
            Ok(())
        }
    }

    fn save_analyze_result_to_storage(
        &self,
        _: &analyzeContext,
        result: &mut analyzeResults,
        analyze_snapshot: bool,
        source: statsMetaHistorySource,
    ) -> AnalyzeSaveResult {
        if self.panic_on_save {
            panic!("save panic");
        }
        self.record(Event::Save(result.tableID, analyze_snapshot, source));
        if self.save_error_table == Some(result.tableID) {
            Err(AnalyzeSaveError("save failed".into()))
        } else {
            Ok(())
        }
    }

    fn finish_analyze_job(&self, job: Option<&analyzeJob>, error: Option<&AnalyzeSaveError>) {
        self.record(Event::Finish(
            job.map_or(0, |job| job.id),
            error.map(|error| error.0.clone()),
        ));
    }

    fn destroy_and_put_to_pool(&self, result: analyzeResults) {
        self.record(Event::Destroy(result.tableID));
    }

    fn log_save_warning(&self, _: &analyzeContext, error: &AnalyzeSaveError) {
        self.record(Event::SaveWarning(error.0.clone()));
    }

    fn log_worker_panic(&self, panic_message: &str) {
        self.record(Event::WorkerPanic(panic_message.into()));
    }

    fn analyze_panic_error(&self, panic_message: &str) -> AnalyzeSaveError {
        AnalyzeSaveError(format!("worker panic: {panic_message}"))
    }

    fn log_error_channel_closed(&self, error: &AnalyzeSaveError) {
        self.record(Event::ClosedErrorChannel(error.0.clone()));
    }
}

fn result(id: u64) -> analyzeResults {
    analyzeResults {
        job: Some(analyzeJob {
            id,
            ..Default::default()
        }),
        tableID: id as i64,
        ..Default::default()
    }
}

fn run_worker(
    runtime: Arc<TestRuntime>,
    results: Vec<analyzeResults>,
    snapshot: bool,
) -> Vec<AnalyzeSaveError> {
    let (results_tx, results_rx) = mpsc::channel();
    let (errors_tx, errors_rx) = mpsc::channel();
    for result in results {
        results_tx.send(result).unwrap();
    }
    drop(results_tx);
    newAnalyzeSaveStatsWorker(results_rx, errors_tx, runtime)
        .run(&analyzeContext { requestID: 7 }, snapshot);
    errors_rx.try_iter().collect()
}

#[test]
fn saves_and_finishes_each_result_before_returning_it_to_the_pool() {
    let runtime = Arc::new(TestRuntime::default());
    assert!(run_worker(runtime.clone(), vec![result(1), result(2)], true).is_empty());
    assert_eq!(
        runtime.events(),
        vec![
            Event::KillCheck,
            Event::Save(1, true, statsMetaHistorySource::Analyze),
            Event::Finish(1, None),
            Event::Destroy(1),
            Event::KillCheck,
            Event::Save(2, true, statsMetaHistorySource::Analyze),
            Event::Finish(2, None),
            Event::Destroy(2),
        ]
    );
}

#[test]
fn save_failure_is_reported_once_but_later_results_are_still_saved() {
    let runtime = Arc::new(TestRuntime {
        save_error_table: Some(1),
        ..Default::default()
    });
    assert_eq!(
        run_worker(runtime.clone(), vec![result(1), result(2)], false),
        vec![AnalyzeSaveError("save failed".into())]
    );
    assert_eq!(
        runtime.events(),
        vec![
            Event::KillCheck,
            Event::Save(1, false, statsMetaHistorySource::Analyze),
            Event::SaveWarning("save failed".into()),
            Event::Finish(1, Some("save failed".into())),
            Event::Destroy(1),
            Event::KillCheck,
            Event::Save(2, false, statsMetaHistorySource::Analyze),
            Event::Finish(2, None),
            Event::Destroy(2),
        ]
    );
}

#[test]
fn kill_switches_to_drain_mode_with_the_same_error() {
    let runtime = Arc::new(TestRuntime {
        kill_at_check: Some(2),
        ..Default::default()
    });
    assert_eq!(
        run_worker(
            runtime.clone(),
            vec![result(1), result(2), result(3)],
            false
        ),
        vec![AnalyzeSaveError("query interrupted".into())]
    );
    assert_eq!(
        runtime.events(),
        vec![
            Event::KillCheck,
            Event::Save(1, false, statsMetaHistorySource::Analyze),
            Event::Finish(1, None),
            Event::Destroy(1),
            Event::KillCheck,
            Event::Finish(2, Some("query interrupted".into())),
            Event::Destroy(2),
            Event::Finish(3, Some("query interrupted".into())),
            Event::Destroy(3),
        ]
    );
}

#[test]
fn panic_is_converted_to_the_worker_error() {
    let runtime = Arc::new(TestRuntime {
        panic_on_save: true,
        ..Default::default()
    });
    assert_eq!(
        run_worker(runtime.clone(), vec![result(1)], false),
        vec![AnalyzeSaveError("worker panic: save panic".into())]
    );
    assert_eq!(
        runtime.events(),
        vec![Event::KillCheck, Event::WorkerPanic("save panic".into()),]
    );
}
