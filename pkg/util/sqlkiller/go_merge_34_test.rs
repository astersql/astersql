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

use super::{KilledByMemArbitrator, QueryInterrupted, SQLKiller, UnspecifiedKillSignal};
use std::sync::{Arc, Mutex};

#[test]
fn go_merge_34_reset_serializes_signal_clear_with_event_reset() {
    let killer = Arc::new(SQLKiller::new());
    let in_hook = killer.clone();
    let pending = Arc::new(Mutex::new(None));
    let pending_in_hook = pending.clone();
    fail::cfg_callback("go_merge_34_after_reset_signal_swap", move || {
        assert!(
            in_hook.killEvent.try_lock().is_err(),
            "reset must hold the kill event mutex while clearing the signal"
        );
        let send = in_hook.clone();
        *pending_in_hook.lock().unwrap() = Some(std::thread::spawn(move || {
            send.SendKillSignalWithKillEventReason(
                KilledByMemArbitrator,
                "memory limit".to_owned(),
            );
        }));
    })
    .unwrap();
    killer.Reset();
    fail::remove("go_merge_34_after_reset_signal_swap");
    pending.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(killer.GetKillSignal(), KilledByMemArbitrator);
    assert!(
        killer
            .HandleSignal()
            .unwrap_err()
            .to_string()
            .contains("memory limit")
    );
    assert!(killer.GetKillEventChan().is_closed());
}

#[test]
fn go_merge_34_reset_between_signal_and_logging_keeps_state_consistent() {
    let killer = Arc::new(SQLKiller::new());
    let callback_killer = killer.clone();
    fail::cfg_callback("go_merge_34_before_log_kill_signal", move || {
        callback_killer.Reset()
    })
    .unwrap();
    killer.SendKillSignal(QueryInterrupted);
    fail::remove("go_merge_34_before_log_kill_signal");
    assert_eq!(killer.GetKillSignal(), UnspecifiedKillSignal);
    assert!(killer.HandleSignal().is_ok());
    assert!(!killer.GetKillEventChan().is_closed());
}
