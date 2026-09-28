// Copyright 2026 AsterSQL.

use super::rule_build_key_info::BuildKeySolver;
use super::rule_init::LogicalRule;

#[test]
fn build_key_solver_uses_go_rule_name() {
    assert_eq!(BuildKeySolver.name(), "build_keys");
}
