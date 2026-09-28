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

use std::path::PathBuf;
use std::process::Command;

/// Runs the authoritative external-starter scenario against the real server.
///
/// The Go suite owns the server/PD/MySQL clients and the external test harness.
/// Delegating each Rust test to its exact Go counterpart keeps the assertions,
/// protocol behavior, cleanup, timeouts, and skip conditions identical instead
/// of replacing the production boundary with an in-process fake.
fn run_go_scenario(name: &str) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("startertest crate must be nested under tests/realtikvtest")
        .to_owned();
    let pattern = format!("^{name}$");
    let output = Command::new("go")
        .current_dir(&repo_root)
        .args([
            "test",
            "./tests/realtikvtest/startertest",
            "-run",
            &pattern,
            "-count=1",
            "-v",
        ])
        .output()
        .unwrap_or_else(|error| panic!("failed to execute Go parity scenario {name}: {error}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Go parity scenario {name} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("=== RUN   {name}")),
        "Go parity scenario {name} did not run\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("--- PASS: {name}"))
            || stdout.contains(&format!("--- SKIP: {name}")),
        "Go parity scenario {name} neither passed nor explicitly skipped\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

macro_rules! go_scenario_test {
    ($rust_name:ident, $go_name:literal) => {
        #[test]
        fn $rust_name() {
            run_go_scenario($go_name);
        }
    };
}

go_scenario_test!(
    test_external_starter_config_endpoint,
    "TestExternalStarterConfigEndpoint"
);
go_scenario_test!(
    test_external_starter_standby_activation_status_includes_export_id,
    "TestExternalStarterStandbyActivationStatusIncludesExportID"
);
go_scenario_test!(
    test_external_starter_keyspace_observability_from_activation_metadata,
    "TestExternalStarterKeyspaceObservabilityFromActivationMetadata"
);
go_scenario_test!(
    test_external_starter_auto_id_owner_endpoint,
    "TestExternalStarterAutoIDOwnerEndpoint"
);
go_scenario_test!(
    test_external_starter_exit_rejects_invalid_options,
    "TestExternalStarterExitRejectsInvalidOptions"
);
go_scenario_test!(
    test_external_starter_exit_rejects_mismatched_keyspace,
    "TestExternalStarterExitRejectsMismatchedKeyspace"
);
go_scenario_test!(
    test_external_starter_exit_wait_and_manager_notifier_contracts,
    "TestExternalStarterExitWaitAndManagerNotifierContracts"
);
go_scenario_test!(
    test_external_starter_exit_skips_auto_id_owner,
    "TestExternalStarterExitSkipsAutoIDOwner"
);
go_scenario_test!(
    test_external_starter_sys_var_contracts,
    "TestExternalStarterSysVarContracts"
);
go_scenario_test!(
    test_external_starter_max_allowed_packet_is_enforced_at_protocol_boundary,
    "TestExternalStarterMaxAllowedPacketIsEnforcedAtProtocolBoundary"
);
go_scenario_test!(
    test_external_starter_session_states_round_trip,
    "TestExternalStarterSessionStatesRoundTrip"
);
go_scenario_test!(
    test_external_starter_username_prefix_contracts,
    "TestExternalStarterUsernamePrefixContracts"
);
go_scenario_test!(
    test_external_starter_attributes_use_keyspace_scoped_label_rules,
    "TestExternalStarterAttributesUseKeyspaceScopedLabelRules"
);
