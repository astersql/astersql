// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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
// 中文总览：函数 `pd_api` 负责 pd api。
// 中文总览：函数 `verify_target_gc_safepoint_exist` 负责 校验 tar读取 gc safepoint exist。
// 中文总览：函数 `verify_target_gc_safepoint_not_exist` 负责 校验 tar读取 gc safepoint not exist。
// 中文总览：函数 `verify_lightning_stopped` 负责 校验 lightning stopped。

//! Go-equivalent tests for `operator_test.go`.
//!
//! Mapping:
//! - `TestOperator` → [`test_operator`]
//! - `TestFailure` → [`test_failure`]
//!
//! Mock/real: PD safepoint/scheduler HTTP + ImportSST Suspended are local PD mock
//! surfaces (same as Go's external PD/TiKV boundary). AdaptEnvForSnapshotBackup
//! drives in-process pause/resume.

use astersql_tests_realtikvtest_brietest::harness::{
    TestCtx, ensure_pd_mock, failpoint, gc, http_get_json, operator, oracle,
    pd_lightning_suspended, require, reset_engine, serial_guard, task,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

const SERVICE_GC_SAFEPOINT_PREFIX: &str = "pd/api/v1/gc/safepoint";
const SCHEDULERS_PREFIX: &str = "pd/api/v1/schedulers";

// 该辅助函数负责 pd api。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn pd_api(cfg: &operator::PauseGcConfig, path: &str) -> String {
    format!("http://{}/{}", cfg.Config.PD[0], path)
}

// 该辅助函数负责 校验 tar读取 gc safepoint exist。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn verify_target_gc_safepoint_exist(t: &TestCtx, cfg: &operator::PauseGcConfig) {
    let v = require::NoErrorVal(t, http_get_json(&pd_api(cfg, SERVICE_GC_SAFEPOINT_PREFIX)));
    let sps = v["service_gc_safe_points"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for sp in sps {
        if sp["service_id"].as_str() == Some(cfg.SafePointID.as_str()) {
            return;
        }
    }
    require::FailNowf(
        t,
        "the service gc safepoint does not exist",
        &format!("{v:?}"),
    );
}

// 该辅助函数负责 校验 tar读取 gc safepoint not exist。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn verify_target_gc_safepoint_not_exist(t: &TestCtx, cfg: &operator::PauseGcConfig) {
    let v = require::NoErrorVal(t, http_get_json(&pd_api(cfg, SERVICE_GC_SAFEPOINT_PREFIX)));
    let sps = v["service_gc_safe_points"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for sp in sps {
        if sp["service_id"].as_str() == Some(cfg.SafePointID.as_str()) {
            require::FailNowf(t, "the service gc safepoint exists", &format!("{sp:?}"));
        }
    }
}

// 该辅助函数负责 校验 lightning stopped。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn verify_lightning_stopped(t: &TestCtx, _cfg: &operator::PauseGcConfig) {
    // Go hits ImportSST Ingest and expects Suspended / ServerIsBusy.
    require::True(t, pd_lightning_suspended());
}

// 该辅助函数负责 校验 schedulers stopped。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn verify_schedulers_stopped(t: &TestCtx, cfg: &operator::PauseGcConfig) {
    let schedulers = require::NoErrorVal(t, http_get_json(&pd_api(cfg, SCHEDULERS_PREFIX)));
    let paused = require::NoErrorVal(
        t,
        http_get_json(&format!("{}?status=paused", pd_api(cfg, SCHEDULERS_PREFIX))),
    );
    let enabled: Vec<&str> = schedulers
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    for p in paused.as_array().unwrap() {
        let s = p.as_str().unwrap();
        require::True(t, enabled.iter().any(|e| *e == s));
    }
}

// 该辅助函数负责 校验 scheduler not stopped。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn verify_scheduler_not_stopped(t: &TestCtx, cfg: &operator::PauseGcConfig) {
    let schedulers = require::NoErrorVal(t, http_get_json(&pd_api(cfg, SCHEDULERS_PREFIX)));
    let paused = require::NoErrorVal(
        t,
        http_get_json(&format!("{}?status=paused", pd_api(cfg, SCHEDULERS_PREFIX))),
    );
    let enabled: Vec<&str> = schedulers
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    for p in paused.as_array().unwrap_or(&vec![]) {
        let s = p.as_str().unwrap();
        require::False(t, enabled.iter().any(|e| *e == s));
    }
}

/// `TestOperator`.
// 该用例覆盖 算子。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_operator() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let addr = ensure_pd_mock();
    let ready = Arc::new(AtomicBool::new(false));
    let exit = Arc::new(AtomicBool::new(false));
    let safe_point_id = gc::MakeSafePointID();
    let safe_point = oracle::GoTimeToTS(SystemTime::now());
    let verify_cfg = operator::PauseGcConfig {
        Config: task::Config {
            PD: vec![addr.clone()],
            ..Default::default()
        },
        TTL: Duration::from_secs(300),
        SafePoint: safe_point,
        SafePointID: safe_point_id.clone(),
        OnAllReady: None,
        OnExit: None,
    };

    verify_target_gc_safepoint_not_exist(&t, &verify_cfg);
    verify_scheduler_not_stopped(&t, &verify_cfg);

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel2 = cancel.clone();
    let ready2 = ready.clone();
    let exit2 = exit.clone();
    let cfg_run = operator::PauseGcConfig {
        Config: task::Config {
            PD: vec![addr],
            ..Default::default()
        },
        TTL: Duration::from_secs(300),
        SafePoint: safe_point,
        SafePointID: safe_point_id,
        OnAllReady: Some(Box::new(move || ready2.store(true, Ordering::SeqCst))),
        OnExit: Some(Box::new(move || exit2.store(true, Ordering::SeqCst))),
    };
    let h = thread::spawn(move || {
        require::NoError(
            &TestCtx::new(),
            operator::AdaptEnvForSnapshotBackup(cancel2, &cfg_run),
        );
    });

    require::Eventually(
        &t,
        || ready.load(Ordering::SeqCst),
        Duration::from_secs(10),
        Duration::from_millis(50),
    );

    verify_target_gc_safepoint_exist(&t, &verify_cfg);
    verify_lightning_stopped(&t, &verify_cfg);
    verify_schedulers_stopped(&t, &verify_cfg);
    cancel.store(true, Ordering::SeqCst);

    require::Eventually(
        &t,
        || exit.load(Ordering::SeqCst),
        Duration::from_secs(10),
        Duration::from_millis(50),
    );
    h.join().unwrap();

    verify_scheduler_not_stopped(&t, &verify_cfg);
    verify_target_gc_safepoint_not_exist(&t, &verify_cfg);
}

/// `TestFailure`.
// 该用例覆盖 失败恢复。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_failure() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let addr = ensure_pd_mock();
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/br/pkg/backup/prepare_snap/PrepareConnectionsErr",
            "return()",
        ),
    );
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/br/pkg/task/operator/SkipReadyHint",
            "return()",
        ),
    );
    t.Cleanup(|| {
        let _ = failpoint::Disable(
            "github.com/pingcap/tidb/br/pkg/backup/prepare_snap/PrepareConnectionsErr",
        );
        let _ = failpoint::Disable("github.com/pingcap/tidb/br/pkg/task/operator/SkipReadyHint");
    });

    let cfg = operator::PauseGcConfig {
        Config: task::Config {
            PD: vec![addr],
            ..Default::default()
        },
        TTL: Duration::from_secs(300),
        SafePoint: oracle::GoTimeToTS(SystemTime::now()),
        SafePointID: gc::MakeSafePointID(),
        OnAllReady: None,
        OnExit: None,
    };
    verify_target_gc_safepoint_not_exist(&t, &cfg);
    verify_scheduler_not_stopped(&t, &cfg);

    let cancel = Arc::new(AtomicBool::new(false));
    let err = operator::AdaptEnvForSnapshotBackup(cancel, &cfg);
    require::Error(&t, err);

    verify_scheduler_not_stopped(&t, &cfg);
    verify_target_gc_safepoint_not_exist(&t, &cfg);
}
