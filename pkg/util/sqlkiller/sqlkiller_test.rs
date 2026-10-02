// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::{Arc, Mutex};

use crate::logutil::log::{BgLogger, LogField};

use super::{KilledByMemArbitrator, QueryInterrupted, SQLKiller, UnspecifiedKillSignal};

#[test]
fn reset_after_successful_kill_signal_cas() {
    let killer = Arc::new(SQLKiller::new());
    let entries_before = BgLogger().entries().len();
    let callback_killer = Arc::clone(&killer);
    fail::cfg_callback("go_merge_34_before_log_kill_signal", move || {
        callback_killer.Reset();
    })
    .expect("install before-log failpoint");

    killer.SendKillSignal(QueryInterrupted);
    fail::remove("go_merge_34_before_log_kill_signal");

    let expected_reason = killer
        .getKillError(QueryInterrupted, "")
        .expect("query interruption must map to an error")
        .to_string();
    let entries = BgLogger().entries();
    let expected_reason_field = LogField::String("reason".to_owned(), expected_reason);
    let initiated = entries[entries_before..]
        .iter()
        .filter(|entry| {
            entry.message == "kill initiated" && entry.fields.contains(&expected_reason_field)
        })
        .collect::<Vec<_>>();
    assert_eq!(initiated.len(), 1, "the successful CAS must log once");

    assert_eq!(killer.GetKillSignal(), UnspecifiedKillSignal);
    assert!(killer.HandleSignal().is_ok());
    let event = killer.killEvent.lock().expect("killEvent mutex poisoned");
    assert!(!event.triggered);
    assert!(event.desc.is_empty());
    drop(event);
    assert!(!killer.GetKillEventChan().is_closed());
}

#[test]
fn kill_signal_after_reset_clear() {
    let killer = Arc::new(SQLKiller::new());
    let callback_killer = Arc::clone(&killer);
    let pending = Arc::new(Mutex::new(None));
    let callback_pending = Arc::clone(&pending);
    fail::cfg_callback("go_merge_34_after_reset_signal_swap", move || {
        assert!(
            callback_killer.killEvent.try_lock().is_err(),
            "Reset must hold the state mutex after clearing Signal"
        );
        let sending_killer = Arc::clone(&callback_killer);
        *callback_pending.lock().expect("pending mutex poisoned") =
            Some(std::thread::spawn(move || {
                sending_killer.SendKillSignalWithKillEventReason(
                    KilledByMemArbitrator,
                    "memory usage exceeds the instance limit".to_owned(),
                );
            }));
    })
    .expect("install after-swap failpoint");

    killer.Reset();
    fail::remove("go_merge_34_after_reset_signal_swap");
    pending
        .lock()
        .expect("pending mutex poisoned")
        .take()
        .expect("failpoint must start the sender")
        .join()
        .expect("sender must finish");

    assert_eq!(killer.GetKillSignal(), KilledByMemArbitrator);
    assert!(
        killer
            .HandleSignal()
            .expect_err("memory arbitrator signal must return an error")
            .to_string()
            .contains("memory usage exceeds the instance limit")
    );
    let event = killer.killEvent.lock().expect("killEvent mutex poisoned");
    assert!(event.triggered);
    assert_eq!(event.desc, "memory usage exceeds the instance limit");
    drop(event);
    assert!(killer.GetKillEventChan().is_closed());
}
