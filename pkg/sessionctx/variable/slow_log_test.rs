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

use crate::slow_log::{self, Threshold};

#[test]
fn rule_fields_match_the_go_accessor_registry() {
    assert!(slow_log::ParseSlowLogFieldValue("Plan_digest", "digest").is_err());
}

#[test]
fn rule_entry_searches_for_the_go_word_colon_pattern() {
    let rules = slow_log::ParseSessionSlowLogRules("ignored prefix DB:TeSt")
        .unwrap()
        .unwrap();
    assert_eq!(
        rules.rules[0].threshold("db"),
        Some(&Threshold::String("TeSt".to_owned()))
    );
}

#[test]
fn digest_setter_materializes_the_statement_digest_like_go() {
    let mut vars = crate::session::SessionVars::new();
    vars.StmtCtx.OriginalSQL = "select * from t".to_owned();
    let mut items = slow_log::SlowQueryLogItems::default();
    let accessor = slow_log::SlowLogRuleFieldAccessors
        .get(&slow_log::SlowLogDigestStr.to_ascii_lowercase())
        .unwrap();

    (accessor.Setter.as_ref().unwrap())(
        &slow_log::SlowLogExecContext::default(),
        Some(&vars),
        &mut items,
    );

    assert!(!items.Digest.is_empty());
    assert_eq!(items.Digest, vars.StmtCtx.SQLDigest().1.String());
}

#[test]
fn wait_ts_rule_reads_shared_wait_duration_in_seconds() {
    let vars = crate::session::SessionVars::new();
    let accessor = slow_log::SlowLogRuleFieldAccessors.get("wait_ts").unwrap();
    let items = slow_log::SlowQueryLogItems::default();
    *vars.DurationWaitTS.lock().unwrap() = std::time::Duration::from_millis(2);
    assert!((accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.002)
    ));
    assert!(!(accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.003)
    ));
    *vars.DurationWaitTS.lock().unwrap() = std::time::Duration::ZERO;
    assert!(!(accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.001)
    ));
}

#[test]
fn ia_scan_details_are_logged_independently_in_seconds() {
    use execdetails::execdetails::{ExecDetails, util::ScanDetail};
    let vars = crate::session::SessionVars::new();
    let mut items = slow_log::SlowQueryLogItems::default();
    items.SQL = "select * from t".into();
    assert!(
        !vars
            .SlowLogFormat(&items)
            .contains("IA_remote_read_segment")
    );
    let mut details = ExecDetails::default();
    details.CopExecDetails.ScanDetail = Some(ScanDetail {
        IaRemoteReadSegmentCount: 3,
        IaRemoteReadSegmentBytes: 4096,
        IaRemoteReadSegmentDuration: std::time::Duration::from_millis(5),
        ..Default::default()
    });
    items.ExecDetail = Some(Box::new(details));
    let log = vars.SlowLogFormat(&items);
    assert!(log.contains("# IA_remote_read_segment_count: 3\n"), "{log}");
    assert!(
        log.contains("# IA_remote_read_segment_size: 4096\n"),
        "{log}"
    );
    assert!(
        log.contains("# IA_remote_read_segment_wait_time: 0.005\n"),
        "{log}"
    );
    let scan = items
        .ExecDetail
        .as_mut()
        .unwrap()
        .CopExecDetails
        .ScanDetail
        .as_mut()
        .unwrap();
    scan.IaRemoteReadSegmentCount = 0;
    scan.IaRemoteReadSegmentBytes = 0;
    let log = vars.SlowLogFormat(&items);
    assert!(!log.contains("IA_remote_read_segment_count"));
    assert!(!log.contains("IA_remote_read_segment_size"));
    assert!(log.contains("IA_remote_read_segment_wait_time: 0.005"));
}

#[test]
fn read_pool_task_details_are_logged_as_one_complete_field() {
    use execdetails::execdetails::{ExecDetails, util::PoolTaskDetails};
    let vars = crate::session::SessionVars::new();
    let mut items = slow_log::SlowQueryLogItems::default();
    assert!(
        !vars
            .SlowLogFormat(&items)
            .contains("Read_pool_task_details")
    );
    let pool = PoolTaskDetails {
        TaskCount: 2,
        PollCount: 8,
        MaxPollCount: 4,
        MinPollCount: 4,
        DispatchCount: 4,
        MaxDispatchCount: 2,
        MinDispatchCount: 2,
        PollWallTime: std::time::Duration::from_millis(12),
        ..Default::default()
    };
    let mut details = ExecDetails::default();
    details.ReadPoolTaskDetails = Some(pool.clone());
    items.ExecDetail = Some(Box::new(details));
    let expected = format!("# Read_pool_task_details: {}\n", pool.String());
    let log = vars.SlowLogFormat(&items);
    assert!(log.contains(&expected), "{log}");
    items.ExecDetail.as_mut().unwrap().ReadPoolTaskDetails = Some(PoolTaskDetails::default());
    assert!(
        !vars
            .SlowLogFormat(&items)
            .contains("Read_pool_task_details")
    );
}

#[test]
fn storage_from_fields_are_logged_for_tikv_without_mpp() {
    let vars = crate::session::SessionVars::new();
    let items = slow_log::SlowQueryLogItems {
        SQL: "select * from t".into(),
        StorageKV: true,
        ..Default::default()
    };

    let log = vars.SlowLogFormat(&items);
    assert!(log.contains("# Storage_from_kv: true\n"), "{log}");
    assert!(log.contains("# Storage_from_mpp: false\n"), "{log}");
}

#[test]
fn ru_details_use_unified_read_write_fields() {
    use execdetails::execdetails::util::RUDetails;

    let vars = crate::session::SessionVars::new();
    let items = slow_log::SlowQueryLogItems {
        SQL: "select * from t".into(),
        RUDetails: Some(RUDetails {
            read_ru: 19.0,
            write_ru: 0.0,
            ru_wait_duration: std::time::Duration::from_millis(20),
            ..Default::default()
        }),
        ..Default::default()
    };

    let log = vars.SlowLogFormat(&items);
    assert!(log.contains("# Request_unit_read: 19\n"), "{log}");
    assert!(!log.contains("Request_unit_write"), "{log}");
    assert!(log.contains("# Time_queued_by_rc: 0.02\n"), "{log}");
    assert!(!log.contains("Request_unit_v2"), "{log}");
}
