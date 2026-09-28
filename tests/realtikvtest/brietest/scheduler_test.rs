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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `get_pd_scheduler_rules` 负责 读取 pd 调度器 rules。
// 中文总览：函数 `extract_key_ranges_from_rule` 负责 extract 键 范围集合 from rule。
// 中文总览：函数 `analyze_scheduler_rules` 负责 分析 调度器 rules。
// 中文总览：函数 `find_new_key_ranges` 负责 find 创建 key 范围集合。
// 中文总览：函数 `compare_key_ranges` 负责 compare 键 范围集合。

//! Go-equivalent tests for `scheduler_test.go`.
//!
//! Mapping:
//! - `TestLogRestoreFineGrainedSchedulerPausing` → [`test_log_restore_fine_grained_scheduler_pausing`]
//!
//! Mock/real: PD region-label rules HTTP is the local PD mock surface; restore
//! failpoint `log-restore-scheduler-paused` injects fine-grained pause ranges.

use astersql_tests_realtikvtest_brietest::harness::{
    EmptyRangeEnd, EmptyRangeStart, KeyRange, LogBackupKit, SchedulerRule, TestCtx, ensure_pd_mock,
    http_get_json, pd_reset_region_rules_baseline, require, reset_engine, serial_guard,
    testfailpoint, testkit,
};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// 该辅助函数负责 读取 pd 调度器 rules。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn get_pd_scheduler_rules(t: &TestCtx, pd_addr: &str) -> Result<Vec<SchedulerRule>, String> {
    let url = format!("http://{pd_addr}/pd/api/v1/config/region-label/rules");
    let v = http_get_json(&url)?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let mut rules = Vec::new();
    for item in arr {
        rules.push(SchedulerRule {
            group_id: item["group_id"].as_str().unwrap_or("").into(),
            id: item["id"].as_str().unwrap_or("").into(),
            start_key: item["start_key"].as_str().unwrap_or("").into(),
            end_key: item["end_key"].as_str().unwrap_or("").into(),
            role: item["role"].as_str().unwrap_or("").into(),
            count: item["count"].as_i64().unwrap_or(0) as i32,
            label_keys: vec![],
            labels: vec![],
            rule_type: item["rule_type"].as_str().unwrap_or("").into(),
            data: item.get("data").cloned().unwrap_or(serde_json::json!([])),
        });
    }
    let _ = t;
    Ok(rules)
}

// 该辅助函数负责 extract 键 范围集合 from rule。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn extract_key_ranges_from_rule(rule: &SchedulerRule) -> Result<Vec<KeyRange>, String> {
    if rule.data.is_null() {
        return Ok(vec![]);
    }
    serde_json::from_value(rule.data.clone()).map_err(|err| err.to_string())
}

#[test]
fn test_extract_key_ranges_rejects_invalid_json_shape() {
    let rule = SchedulerRule {
        group_id: "pd".into(),
        id: "invalid-data".into(),
        start_key: String::new(),
        end_key: String::new(),
        role: String::new(),
        count: 0,
        label_keys: vec![],
        labels: vec![],
        rule_type: String::new(),
        data: serde_json::json!({"start_key": "a", "end_key": "b"}),
    };

    assert!(extract_key_ranges_from_rule(&rule).is_err());
}

// 该辅助函数负责 分析 调度器 rules。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn analyze_scheduler_rules(
    t: &TestCtx,
    rules: &[SchedulerRule],
    title: &str,
) -> HashMap<String, Vec<KeyRange>> {
    t.Log(format!("=== {title} ==="));
    t.Log(format!("Total rules found: {}", rules.len()));
    let mut all = HashMap::new();
    for (i, rule) in rules.iter().enumerate() {
        let key_ranges = extract_key_ranges_from_rule(rule).unwrap_or_default();
        let rule_key = format!("{}/{}", rule.group_id, rule.id);
        all.insert(rule_key, key_ranges);
        t.Log(format!(
            "Rule {}: ID={}, GroupID={}",
            i + 1,
            rule.id,
            rule.group_id
        ));
    }
    all
}

// 该辅助函数负责 find 创建 key 范围集合。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn find_new_key_ranges(baseline: &[KeyRange], current: &[KeyRange]) -> Vec<KeyRange> {
    let mut new_ranges = Vec::new();
    for current_range in current {
        let found = baseline
            .iter()
            .any(|b| b.start_key == current_range.start_key && b.end_key == current_range.end_key);
        if !found {
            new_ranges.push(current_range.clone());
        }
    }
    new_ranges
}

// 该辅助函数负责 compare 键 范围集合。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn compare_key_ranges(
    t: &TestCtx,
    baseline_ranges: &HashMap<String, Vec<KeyRange>>,
    current_ranges: &HashMap<String, Vec<KeyRange>>,
) -> bool {
    t.Log("Comparing key ranges...");
    let mut has_changes = false;
    for (rule_key, current_krs) in current_ranges {
        match baseline_ranges.get(rule_key) {
            None => {
                t.Log(format!(
                    "NEW RULE: {rule_key} with {} key ranges",
                    current_krs.len()
                ));
                has_changes = true;
            }
            Some(baseline_krs) => {
                if current_krs.len() != baseline_krs.len() {
                    t.Log(format!("RULE MODIFIED: {rule_key}"));
                    let new_ranges = find_new_key_ranges(baseline_krs, current_krs);
                    if !new_ranges.is_empty() {
                        for kr in &new_ranges {
                            if kr.start_key == EmptyRangeStart && kr.end_key == EmptyRangeEnd {
                                t.Log("FULL RANGE PAUSE detected");
                            } else {
                                t.Log("FINE-GRAINED PAUSE detected");
                            }
                        }
                        has_changes = true;
                    }
                    let removed = find_new_key_ranges(current_krs, baseline_krs);
                    if !removed.is_empty() {
                        has_changes = true;
                    }
                } else {
                    t.Log(format!(
                        "RULE UNCHANGED: {rule_key} ({} ranges)",
                        current_krs.len()
                    ));
                }
            }
        }
    }
    has_changes
}

// 该辅助函数负责 setup 数据。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn setup_test_data(kit: &LogBackupKit, task_name: &str) {
    let s = kit.simpleWorkload();
    s.createSimpleTableWithData(kit);
    kit.tk
        .MustExec("CREATE TABLE test.snapshot_table1 (id INT PRIMARY KEY, data VARCHAR(100))");
    kit.tk
        .MustExec("INSERT INTO test.snapshot_table1 VALUES (1, 'snapshot1'), (2, 'snapshot2')");
    kit.RunLogStart(task_name, |_| {});
    kit.RunFullBackup(|_| {});
    kit.tk
        .MustExec("CREATE TABLE test.log_table1 (id INT PRIMARY KEY, data VARCHAR(100))");
    kit.tk
        .MustExec("INSERT INTO test.log_table1 VALUES (1, 'log1'), (2, 'log2')");
    kit.tk
        .MustExec("INSERT INTO test.snapshot_table1 VALUES (3, 'incremental')");
    kit.tk
        .MustExec("INSERT INTO test.log_table1 VALUES (3, 'incremental')");
    kit.forceFlushAndWait(task_name);
    kit.StopTaskIfExists(task_name);
}

// 该辅助函数负责 清理 data。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn cleanup_test_data(kit: &LogBackupKit) {
    let s = kit.simpleWorkload();
    s.cleanSimpleData(kit);
    kit.tk
        .MustExec("DROP TABLE IF EXISTS test.snapshot_table1, test.log_table1");
}

// 该辅助函数负责 检查 scheduler pausing behavior。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn check_scheduler_pausing_behavior(
    t: &TestCtx,
    pd_addr: &str,
    baseline_key_ranges: &HashMap<String, Vec<KeyRange>>,
) -> Vec<SchedulerRule> {
    let mut final_rules = Vec::new();
    let max_retries = 20;
    for i in 0..max_retries {
        thread::sleep(Duration::from_millis(200));
        let rules = match get_pd_scheduler_rules(t, pd_addr) {
            Ok(r) => r,
            Err(e) => {
                t.Log(format!(
                    "Failed to get scheduler rules (attempt {}): {e}",
                    i + 1
                ));
                continue;
            }
        };
        let mut has_new = false;
        for rule in &rules {
            let rule_key = format!("{}/{}", rule.group_id, rule.id);
            if let Ok(key_ranges) = extract_key_ranges_from_rule(rule) {
                if let Some(baseline) = baseline_key_ranges.get(&rule_key) {
                    if key_ranges.len() > baseline.len() {
                        has_new = true;
                    }
                }
            }
        }
        if has_new {
            final_rules = rules;
            break;
        }
        if i == max_retries - 1 {
            final_rules = rules;
        }
    }
    final_rules
}

/// `TestLogRestoreFineGrainedSchedulerPausing`.
// 该用例覆盖 log 恢复 fine grained 调度器 pausing。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_log_restore_fine_grained_scheduler_pausing() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let addr = ensure_pd_mock();
    // Point PD lookups used by helpers to mock addr via config path rewrite:
    // helpers hardcode 127.0.0.1:2379 — bind mock there if possible, else patch via ensure.
    // Our ensure_pd_mock binds :0; rewrite get to use config Path.
    let kit = LogBackupKit::new(&t);
    let task_name = "test-fine-grained-scheduler";
    pd_reset_region_rules_baseline();

    setup_test_data(&kit, task_name);
    cleanup_test_data(&kit);

    // Use actual mock addr for baseline
    let baseline_rules = get_pd_scheduler_rules(&t, &addr).unwrap();
    let baseline_key_ranges = analyze_scheduler_rules(
        &t,
        &baseline_rules,
        "BASELINE SCHEDULER RULES (before restore)",
    );

    let (tx, rx) = mpsc::sync_channel(1);
    let t2 = t.clone();
    let baseline2 = baseline_key_ranges.clone();
    let addr2 = addr.clone();
    testfailpoint::EnableCall(
        &t,
        "github.com/pingcap/tidb/br/pkg/task/log-restore-scheduler-paused",
        move || {
            t2.Log("Failpoint triggered - checking PD scheduler rules");
            // During pause, rules already have new ranges (pd_add_fine_grained_pause).
            let final_rules = check_scheduler_pausing_behavior(&t2, &addr2, &baseline2);
            let _ = tx.send(final_rules);
        },
    );

    kit.RunStreamRestore(|rc| {
        kit.SetFilter(&mut rc.Config, &["test.snapshot_table1", "test.log_table1"]);
        // Keep RestoreConfig.FilterStr in sync (RunRestore reads both).
        rc.FilterStr = rc.Config.FilterStr.clone();
    });

    let restore_rules = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("Timeout waiting for failpoint callback");
    let restore_key_ranges = analyze_scheduler_rules(
        &t,
        &restore_rules,
        "SCHEDULER RULES DURING FILTERED RESTORE",
    );
    let has_changes = compare_key_ranges(&t, &baseline_key_ranges, &restore_key_ranges);
    require::TrueMsg(
        &t,
        has_changes,
        "Fine-grained scheduler pausing should be detected during filtered restore",
    );

    kit.tk
        .MustQuery("SELECT COUNT(*) FROM test.snapshot_table1")
        .Check(&testkit::Rows(&["3"]));
    kit.tk
        .MustQuery("SELECT COUNT(*) FROM test.log_table1")
        .Check(&testkit::Rows(&["3"]));

    let final_rules = get_pd_scheduler_rules(&t, &addr).unwrap();
    let final_key_ranges = analyze_scheduler_rules(
        &t,
        &final_rules,
        "FINAL SCHEDULER RULES (after filtered restore)",
    );
    let final_has_changes = compare_key_ranges(&t, &baseline_key_ranges, &final_key_ranges);
    require::False(&t, final_has_changes);
}
