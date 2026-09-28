// Copyright 2026 AsterSQL.

use super::*;
use std::sync::{Arc, mpsc};
use std::time::Duration;

struct FailingDatabase;

impl Database for FailingDatabase {
    fn execute(&self, _: &str) -> Result<(), ImporterError> {
        unreachable!("job processing only executes statements inside a transaction")
    }

    fn begin(&self) -> Result<Box<dyn DatabaseTransaction>, ImporterError> {
        Err(ImporterError::Database("begin failed".to_owned()))
    }

    fn close(&self) -> Result<(), ImporterError> {
        Ok(())
    }
}

#[test]
fn worker_error_does_not_deadlock_a_full_job_channel() {
    let table = Arc::new(Table::new());
    let database: Arc<dyn Database> = Arc::new(FailingDatabase);
    let (result_sender, result_receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let result = process_jobs(table, &[database], 64, 1, 1);
        result_sender.send(result).unwrap();
    });

    let result = result_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("worker failure must cancel the producer instead of deadlocking");
    assert_eq!(
        result,
        Err(ImporterError::Database("begin failed".to_owned()))
    );
}
