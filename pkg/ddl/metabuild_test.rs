// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use super::metabuild::{
    BuildOption, ClusteredIndexMode, SessionBuildContext, SessionVariables,
    new_meta_build_context_with_session,
};

fn session() -> SessionBuildContext {
    SessionBuildContext {
        expression_context: "strict-all-tables,no-zero-date;utf8mb4_bin".to_owned(),
        latest_info_schema: "latest-info-schema".to_owned(),
        variables: SessionVariables {
            enable_auto_increment_in_generated: true,
            primary_key_required: true,
            in_restricted_sql: false,
            clustered_index_mode: ClusteredIndexMode::IntOnly,
            shard_row_id_bits: 0,
            pre_split_regions: 0,
        },
    }
}

#[test]
fn copies_every_session_derived_field() {
    let mut session = session();
    session.variables.enable_auto_increment_in_generated = false;
    session.variables.clustered_index_mode = ClusteredIndexMode::On;
    session.variables.shard_row_id_bits = 6;
    session.variables.pre_split_regions = 123;

    let context = new_meta_build_context_with_session(&session, []);

    assert_eq!(context.expression_context, session.expression_context);
    assert!(!context.enable_auto_increment_in_generated);
    assert!(context.primary_key_required);
    assert_eq!(context.clustered_index_mode, ClusteredIndexMode::On);
    assert_eq!(context.shard_row_id_bits, 6);
    assert_eq!(context.pre_split_regions, 123);
    assert_eq!(context.info_schema, session.latest_info_schema);
    assert!(!context.suppress_too_long_index_error);
    assert!(context.warnings.is_empty());
}

#[test]
fn restricted_sql_disables_primary_key_requirement() {
    let mut session = session();
    session.variables.in_restricted_sql = true;
    let context = new_meta_build_context_with_session(&session, []);
    assert!(!context.primary_key_required);
}

#[test]
fn caller_options_follow_session_defaults_and_later_options_win() {
    let context = new_meta_build_context_with_session(
        &session(),
        [
            BuildOption::EnableAutoIncrementInGenerated(false),
            BuildOption::PrimaryKeyRequired(false),
            BuildOption::ClusteredIndexMode(ClusteredIndexMode::Off),
            BuildOption::ShardRowIdBits(6),
            BuildOption::PreSplitRegions(123),
            BuildOption::InfoSchema("overridden-info-schema".to_owned()),
            BuildOption::SuppressTooLongIndexError(true),
            BuildOption::ShardRowIdBits(9),
        ],
    );

    assert!(!context.enable_auto_increment_in_generated);
    assert!(!context.primary_key_required);
    assert_eq!(context.clustered_index_mode, ClusteredIndexMode::Off);
    assert_eq!(context.shard_row_id_bits, 9);
    assert_eq!(context.pre_split_regions, 123);
    assert_eq!(context.info_schema, "overridden-info-schema");
    assert!(context.suppress_too_long_index_error);
}

#[test]
fn clustered_index_modes_round_trip() {
    for mode in [
        ClusteredIndexMode::IntOnly,
        ClusteredIndexMode::Off,
        ClusteredIndexMode::On,
    ] {
        let mut session = session();
        session.variables.clustered_index_mode = mode;
        assert_eq!(
            new_meta_build_context_with_session(&session, []).clustered_index_mode,
            mode
        );
    }
}

#[test]
#[should_panic(expected = "session expression context must not be empty")]
fn rejects_missing_expression_context() {
    let mut session = session();
    session.expression_context.clear();
    let _ = new_meta_build_context_with_session(&session, []);
}
