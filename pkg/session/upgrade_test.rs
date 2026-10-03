// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::sync::Mutex;

static EXECUTED_UPGRADES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record_upgrade(name: &str) {
    EXECUTED_UPGRADES
        .lock()
        .expect("upgrade action recorder lock")
        .push(name.to_owned());
}

#[test]
fn upgrade_to_ver_functions_check() {
    crate::upgrade_def::installUpgradeAction(record_upgrade)
        .expect("upgrade action recorder should only be installed once");

    let mut last_version = 0;
    let mut first_version_after_reserved_range = None;
    for versioned_upgrade in crate::upgrade_def::upgradeToVerFunctions.iter() {
        assert!(
            versioned_upgrade.version > last_version,
            "upgradeToVerFunctions should be in ascending order: {} follows {last_version}",
            versioned_upgrade.version
        );
        last_version = versioned_upgrade.version;
        if last_version > 256 && first_version_after_reserved_range.is_none() {
            first_version_after_reserved_range = Some(last_version);
        }

        EXECUTED_UPGRADES
            .lock()
            .expect("upgrade action recorder lock")
            .clear();
        // The upgrade stubs do not inspect their placeholder session argument.
        // A dangling reference is valid here because that private type is a
        // zero-sized, inhabited struct and the pointer is non-null and aligned.
        let session = unsafe { &*std::ptr::NonNull::dangling().as_ptr() };
        (versioned_upgrade.function)(session, last_version);
        let expected_name = format!("upgradeToVer{}", versioned_upgrade.version);
        assert_eq!(
            EXECUTED_UPGRADES
                .lock()
                .expect("upgrade action recorder lock")
                .as_slice(),
            [expected_name],
            "function name should match upgradeToVer pattern"
        );
    }

    assert_eq!(first_version_after_reserved_range, Some(277));

    // SAFETY: tests do not mutate the bootstrap version; this mirrors Go's
    // final comparison with currentBootstrapVersion.
    assert_eq!(
        unsafe { crate::upgrade_def::currentBootstrapVersion },
        last_version
    );
}
