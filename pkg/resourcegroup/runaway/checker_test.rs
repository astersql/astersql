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

// Checker 单元测试：活跃计数、阈值与动作优先级、watch/Cop 改写、错误传播、
// quarantine 副作用、convict 标识和 CAS 并发标记。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::checker::Checker;
use crate::manager::Manager;
use crate::record::QuarantineRecord;
use crate::syncer::AllSystemTables;
use crate::{
    CopRequest, Error, NoopExecutor, RUDetails, ResourceGroup, ResourceGroupCatalog, Result,
    RunawayAction, RunawayRule, RunawaySettings, RunawayWatch, RunawayWatchType, nowMicros,
};

/// 测试用资源组目录：按名称存放内存中的 ResourceGroup。
#[derive(Default)]
struct Catalog {
    groups: std::collections::HashMap<String, ResourceGroup>,
}

impl ResourceGroupCatalog for Catalog {
    fn GetResourceGroup(&self, name: &str) -> Result<Option<ResourceGroup>> {
        Ok(self.groups.get(name).cloned())
    }
}

/// 用给定资源组列表构造 Manager（NoopExecutor + AllSystemTables）。
fn manager_with(groups: Vec<ResourceGroup>) -> Manager {
    let catalog = Catalog {
        groups: groups.into_iter().map(|g| (g.name.clone(), g)).collect(),
    };
    Manager::NewRunawayManager(
        Arc::new(catalog),
        "server-1",
        Arc::new(NoopExecutor),
        Arc::new(AllSystemTables),
    )
}

/// 构造带 Similar watch 与固定阈值的 runaway 设置。
fn settings(action: RunawayAction) -> RunawaySettings {
    RunawaySettings {
        rule: RunawayRule {
            exec_elapsed_time_ms: 1_000,
            request_unit: 100,
            processed_keys: 10,
        },
        action,
        switch_group_name: String::new(),
        watch: Some(RunawayWatch {
            kind: RunawayWatchType::Similar,
            lasting_duration_ms: 60_000,
        }),
    }
}

/// 验证活跃计数加减顺序与并发后最终为 0（对齐 Go TestActiveGroupCounterOrdering）。
#[test]
fn active_group_counter_ordering_and_concurrency_match_go() {
    let manager = manager_with(Vec::new());
    let normal = manager.loadOrStoreActiveCounter("normal").unwrap().0;
    normal.fetch_add(1, Ordering::AcqRel);
    normal.fetch_sub(1, Ordering::AcqRel);
    assert_eq!(manager.getActiveWatchCount("normal"), 0);

    let reversed = manager.loadOrStoreActiveCounter("reversed").unwrap().0;
    reversed.fetch_sub(1, Ordering::AcqRel);
    reversed.fetch_add(1, Ordering::AcqRel);
    assert_eq!(manager.getActiveWatchCount("reversed"), 0);
    assert_eq!(manager.getActiveWatchCount("missing"), 0);

    // 并发 +1/-1 各半，最终活跃计数应归零。
    let mut workers = Vec::new();
    for delta in [1_i64, -1].into_iter().cycle().take(2_000) {
        let manager = manager.clone();
        workers.push(std::thread::spawn(move || {
            manager
                .loadOrStoreActiveCounter("concurrent")
                .unwrap()
                .0
                .fetch_add(delta, Ordering::AcqRel);
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(manager.getActiveWatchCount("concurrent"), 0);
}

/// 验证耗时/RU/processed keys 优先级与累计超限后 Kill 标记。
#[test]
fn thresholds_preserve_go_priority_and_accumulation() {
    let manager = manager_with(Vec::new());
    let checker = Checker::NewChecker(
        manager.clone(),
        "rg".into(),
        Some(settings(RunawayAction::Kill)),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        1_000_000,
    );
    assert!(
        checker
            .exceedsThresholds(2_000_000, None, 10)
            .contains("ElapsedTime")
    );
    assert!(
        checker
            .exceedsThresholds(
                0,
                Some(&RUDetails {
                    read_ru: 75.0,
                    write_ru: 25.0
                }),
                10
            )
            .contains("RequestUnit")
    );
    assert!(
        checker
            .exceedsThresholds(0, None, 10)
            .contains("ProcessedKeys")
    );

    assert!(checker.CheckThresholds(None, 5, None).is_none());
    assert!(checker.CheckThresholds(None, 5, None).is_some());
    assert!(checker.isMarkedByIdentifyInRunawaySettings());
    assert_eq!(checker.CheckAction(), RunawayAction::Kill);
    assert_eq!(manager.drainRunawayRecords().len(), 1);
    checker.ResetTotalProcessedKeys();
}

/// 验证 Exact/Similar/Plan/None 对应不同 convict 标识。
#[test]
fn setting_convict_identifier_matches_watch_type() {
    for (kind, expected) in [
        (RunawayWatchType::Exact, "select 1"),
        (RunawayWatchType::Similar, "sql"),
        (RunawayWatchType::Plan, "plan"),
        (RunawayWatchType::None, ""),
    ] {
        let mut value = settings(RunawayAction::DryRun);
        value.watch.as_mut().unwrap().kind = kind;
        let checker = Checker::NewChecker(
            manager_with(Vec::new()),
            "rg".into(),
            Some(value),
            "select 1".into(),
            "sql".into(),
            "plan".into(),
            0,
        );
        assert_eq!(checker.getSettingConvictIdentifier(), expected);
    }
}

/// 对齐 Go TestNewChecker，并覆盖 DeriveChecker 的全部提前返回条件。
#[test]
fn new_and_derived_checker_match_go_guards() {
    let start = 1_000_000;
    let checker = Checker::NewChecker(
        manager_with(Vec::new()),
        "rg".into(),
        None,
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        start,
    );
    assert_eq!(checker.exceedsThresholds(i64::MAX, None, i64::MAX), "");

    let checker = Checker::NewChecker(
        manager_with(Vec::new()),
        "rg".into(),
        Some(settings(RunawayAction::Kill)),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        start,
    );
    assert_eq!(checker.exceedsThresholds(1_999_999, None, 9), "");
    assert!(
        checker
            .exceedsThresholds(2_000_000, None, 0)
            .contains("ElapsedTime")
    );
    assert!(
        checker
            .exceedsThresholds(
                0,
                Some(&RUDetails {
                    read_ru: 40.0,
                    write_ru: 60.0,
                }),
                0,
            )
            .contains("RequestUnit")
    );
    assert!(
        checker
            .exceedsThresholds(0, None, 10)
            .contains("ProcessedKeys")
    );

    let bare_group = ResourceGroup {
        name: "bare".into(),
        runaway_settings: None,
    };
    let configured_group = ResourceGroup {
        name: "configured".into(),
        runaway_settings: Some(settings(RunawayAction::DryRun)),
    };
    let manager = manager_with(vec![bare_group, configured_group]);
    assert!(
        manager
            .DeriveChecker(
                "missing",
                "sql".into(),
                "digest".into(),
                "plan".into(),
                start
            )
            .is_none()
    );
    assert!(
        manager
            .DeriveChecker(
                "configured",
                "sql".into(),
                "digest".into(),
                String::new(),
                start,
            )
            .is_none()
    );
    assert!(
        manager
            .DeriveChecker("bare", "sql".into(), "digest".into(), "plan".into(), start)
            .is_none()
    );
    assert!(
        manager
            .DeriveChecker(
                "configured",
                "sql".into(),
                "digest".into(),
                "plan".into(),
                start,
            )
            .is_some()
    );
}

fn watch_record(
    group: &str,
    text: &str,
    action: RunawayAction,
    switch_group: &str,
) -> QuarantineRecord {
    QuarantineRecord {
        ResourceGroupName: group.into(),
        WatchText: text.into(),
        Watch: RunawayWatchType::Exact,
        Action: action,
        SwitchGroupName: switch_group.into(),
        ..Default::default()
    }
}

/// 覆盖 Go BeforeExecutor 的 fallback、Kill、CoolDown 与 SwitchGroup 分支。
#[test]
fn watch_actions_and_switch_group_validation_match_go() {
    let source = ResourceGroup {
        name: "source".into(),
        runaway_settings: Some(settings(RunawayAction::Kill)),
    };
    let target = ResourceGroup {
        name: "target".into(),
        runaway_settings: None,
    };

    let kill_manager = manager_with(vec![source.clone(), target.clone()]);
    kill_manager.AddWatch(watch_record(
        "source",
        "select kill",
        RunawayAction::NoneAction,
        "",
    ));
    let mut kill = Checker::NewChecker(
        kill_manager.clone(),
        "source".into(),
        source.runaway_settings.clone(),
        "select kill".into(),
        "sql".into(),
        "plan".into(),
        nowMicros(),
    );
    assert_eq!(kill.BeforeExecutor(), Err(Error::Quarantined));
    assert_eq!(kill.CheckAction(), RunawayAction::Kill);
    let records = kill_manager.drainRunawayRecords();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].Match, "watch");
    assert_eq!(records[0].Action, "kill");

    let cooldown_manager = manager_with(vec![source.clone(), target.clone()]);
    cooldown_manager.AddWatch(watch_record(
        "source",
        "select cool",
        RunawayAction::CoolDown,
        "",
    ));
    let mut cooldown = Checker::NewChecker(
        cooldown_manager,
        "source".into(),
        source.runaway_settings.clone(),
        "select cool".into(),
        "sql".into(),
        "plan".into(),
        nowMicros(),
    );
    assert_eq!(cooldown.BeforeExecutor().unwrap(), "");
    let mut request = CopRequest::default();
    cooldown.BeforeCopRequest(&mut request).unwrap();
    assert_eq!(request.override_priority, Some(1));

    for (switch_group, expected) in [("target", "target"), ("missing", "")] {
        let manager = manager_with(vec![source.clone(), target.clone()]);
        manager.AddWatch(watch_record(
            "source",
            "select switch",
            RunawayAction::SwitchGroup,
            switch_group,
        ));
        let mut checker = Checker::NewChecker(
            manager,
            "source".into(),
            source.runaway_settings.clone(),
            "select switch".into(),
            "sql".into(),
            "plan".into(),
            nowMicros(),
        );
        assert_eq!(checker.BeforeExecutor().unwrap(), expected);
        assert_eq!(checker.CheckAction(), RunawayAction::SwitchGroup);
    }
}

/// 覆盖 Go BeforeCopRequest 与 CheckThresholds 的动作、错误和副作用契约。
#[test]
fn rule_actions_errors_and_quarantine_match_go() {
    let target = ResourceGroup {
        name: "target".into(),
        runaway_settings: None,
    };
    let mut switch_settings = settings(RunawayAction::SwitchGroup);
    switch_settings.switch_group_name = "target".into();
    let source = ResourceGroup {
        name: "source".into(),
        runaway_settings: Some(switch_settings.clone()),
    };
    let manager = manager_with(vec![source, target]);
    let switch = Checker::NewChecker(
        manager,
        "source".into(),
        Some(switch_settings),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        0,
    );
    let mut request = CopRequest::default();
    switch.BeforeCopRequest(&mut request).unwrap();
    assert_eq!(request.resource_group_name, "target");

    let mut kill_settings = settings(RunawayAction::Kill);
    kill_settings.rule.exec_elapsed_time_ms = 60_000;
    kill_settings.rule.processed_keys = 10;
    let group = ResourceGroup {
        name: "kill".into(),
        runaway_settings: Some(kill_settings.clone()),
    };
    let manager = manager_with(vec![group]);
    let kill = Checker::NewChecker(
        manager.clone(),
        "kill".into(),
        Some(kill_settings),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        nowMicros() - 1_000,
    );
    let mut request = CopRequest::default();
    kill.BeforeCopRequest(&mut request).unwrap();
    assert!(request.max_execution_duration_ms > 0);
    assert!(request.max_execution_duration_ms <= 60_000);

    let original = Error::Storage("cop failed".into());
    assert_eq!(
        kill.CheckThresholds(None, 5, Some(original.clone())),
        Some(original)
    );
    let interrupted = kill.CheckThresholds(None, 5, None).unwrap();
    assert!(
        matches!(interrupted, Error::QueryInterrupted(ref cause) if cause.contains("ProcessedKeys"))
    );
    assert!(kill.isMarkedByIdentifyInRunawaySettings());
    assert_eq!(kill.CheckAction(), RunawayAction::Kill);
    assert_eq!(manager.drainRunawayRecords().len(), 1);
    let quarantines = manager.drainQuarantineRecords();
    assert_eq!(quarantines.len(), 1);
    assert_eq!(quarantines[0].WatchText, "sql");
    assert!(quarantines[0].ExceedCause.contains("ProcessedKeys"));
}

/// 对齐 Go TestCheckRuleKillAction 与 TestConcurrentResetAndCheckThresholds。
#[test]
fn kill_polling_and_concurrent_reset_match_go() {
    let mut kill_settings = settings(RunawayAction::Kill);
    kill_settings.rule.exec_elapsed_time_ms = 1;
    let checker = Arc::new(Checker::NewChecker(
        manager_with(Vec::new()),
        "rg".into(),
        Some(kill_settings),
        "select 1".into(),
        "sql".into(),
        "plan".into(),
        nowMicros() - 2_000,
    ));
    let (cause, should_kill) = checker.CheckRuleKillAction();
    assert!(cause.contains("ElapsedTime"));
    assert!(should_kill);
    assert_eq!(checker.CheckRuleKillAction(), (String::new(), false));

    let no_settings = Arc::new(Checker::NewChecker(
        manager_with(Vec::new()),
        "rg".into(),
        None,
        String::new(),
        String::new(),
        String::new(),
        0,
    ));
    let mut workers = Vec::new();
    for _ in 0..5 {
        let checker = no_settings.clone();
        workers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                assert!(checker.CheckThresholds(None, 10, None).is_none());
            }
        }));
        let checker = no_settings.clone();
        workers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                checker.ResetTotalProcessedKeys();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
}
