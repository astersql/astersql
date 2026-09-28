// Copyright 2026 AsterSQL.

use std::sync::atomic::AtomicU32;

use super::{BackoffConfig, Context, Jitter, NewBackofferWithVars, driver_error, errors, kv};

fn retry_error(message: &str) -> errors::SharedError {
    errors::SharedError::new(driver_error::TiKvError::Other(message.to_owned()))
}

#[test]
fn txn_lock_fast_config_name_matches_case_insensitively() {
    let killed = AtomicU32::new(0);
    let vars = kv::Variables {
        BackoffLockFast: 7,
        BackOffWeight: 1,
        Killed: &killed,
    };
    let mut backoffer = NewBackofferWithVars(Context::new(), 100, Some(&vars));
    let config = BackoffConfig::new("TXNLOCKFAST", 2, 100, Jitter::NoJitter);

    assert!(backoffer.Backoff(&config, retry_error("locked")).is_ok());
    assert_eq!(backoffer.GetBackoffSleepMS().get("TXNLOCKFAST"), Some(&7));
}
