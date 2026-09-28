// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::Duration;

use crate::upgrade_run::{UpgradeRuntime, VersionedUpgrade};

struct MetadataLockRuntime {
    setting: Result<Option<bool>, &'static str>,
    enabled: Option<bool>,
}

impl UpgradeRuntime for MetadataLockRuntime {
    type Error = &'static str;

    fn metadata_lock(&mut self) -> Result<Option<bool>, Self::Error> {
        self.setting
    }

    fn set_metadata_lock_enabled(&mut self, enabled: bool) {
        self.enabled = Some(enabled);
    }

    fn bootstrap_version(&mut self) -> Result<i64, Self::Error> {
        unreachable!()
    }

    fn current_bootstrap_version(&mut self) -> i64 {
        unreachable!()
    }

    fn support_upgrade_http_version(&mut self) -> i64 {
        unreachable!()
    }

    fn internal_sql_timeout(&mut self) -> Duration {
        unreachable!()
    }

    fn check_cluster_state(&mut self, _: i64, _: i64, _: Duration) {
        unreachable!()
    }

    fn upgrade_functions(&mut self) -> Vec<VersionedUpgrade> {
        unreachable!()
    }

    fn execute_upgrade(&mut self, _: VersionedUpgrade, _: i64) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn upgrade_version_99_before(&mut self) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn upgrade_version_99_after(&mut self) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn update_bootstrap_version(&mut self) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn commit(&mut self) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn sleep(&mut self, _: Duration) {
        unreachable!()
    }
}

#[test]
fn mdl_is_disabled_when_reading_persisted_setting_fails() {
    let mut runtime = MetadataLockRuntime {
        setting: Err("metadata read failed"),
        enabled: None,
    };

    assert_eq!(
        crate::upgrade_run::InitMDLVariableForUpgrade(&mut runtime),
        Err("metadata read failed")
    );
    assert_eq!(runtime.enabled, Some(false));
}

#[test]
fn mdl_setting_and_null_marker_match_go_semantics() {
    for (setting, expected_enabled, expected_was_null) in [
        (Some(true), true, false),
        (Some(false), false, false),
        (None, false, true),
    ] {
        let mut runtime = MetadataLockRuntime {
            setting: Ok(setting),
            enabled: None,
        };
        assert_eq!(
            crate::upgrade_run::InitMDLVariableForUpgrade(&mut runtime),
            Ok(expected_was_null)
        );
        assert_eq!(runtime.enabled, Some(expected_enabled));
    }
}
