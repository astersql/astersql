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

use crate::restore::RestoreConfig;
use crate::stream::restoreStreamWithTiKVConfigControl;
use crate::stream_test::config_control;

#[test]
fn retain_latest_mvcc_controls_rocksdb_background_jobs() {
    for retain_latest_mvcc in [false, true] {
        let (control, state, _) = config_control(false);
        let mut cfg = RestoreConfig {
            RetainLatestMVCCVersion: retain_latest_mvcc,
            ..Default::default()
        };

        restoreStreamWithTiKVConfigControl(&mut cfg, &control, |_, _| {
            assert_eq!(
                state.lock().unwrap().values["jobs"],
                if retain_latest_mvcc { "1" } else { "8" }
            );
            Ok(())
        })
        .unwrap();

        let state = state.lock().unwrap();
        assert_eq!(state.values["jobs"], "8");
        let jobs_calls = state
            .calls
            .iter()
            .filter(|call| call.contains(":jobs:"))
            .count();
        assert_eq!(jobs_calls, if retain_latest_mvcc { 3 } else { 0 });
    }
}
