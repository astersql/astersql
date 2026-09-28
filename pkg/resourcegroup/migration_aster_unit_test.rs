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

// 资源组 runaway / 消费上报 trait 的迁移单元测试。
//
// 用本地桩实现验证 `RunawayChecker` 与 `ConsumptionReporter` 契约：
// watch list、cop 请求改写、RU 阈值、kill 动作以及两类消费上报形状。

use super::{ConsumptionReporter, DEFAULT_RESOURCE_GROUP_NAME, RunawayChecker};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// 测试用 runaway 动作枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// 杀掉查询。
    Kill,
}

/// 测试用 coprocessor 请求桩。
#[derive(Default)]
struct Request {
    /// 是否已被标记为限流/降速。
    throttled: bool,
}

/// 测试用 RU 明细。
#[derive(Default)]
struct RuDetails {
    /// 已读请求单元数。
    read_units: u64,
}

/// 测试用检查错误。
#[derive(Debug, Eq, PartialEq)]
struct CheckError(&'static str);

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for CheckError {}

/// `RunawayChecker` 的最小可测实现。
struct Checker {
    /// 累计处理的 key 数。
    processed_keys: AtomicUsize,
    /// 当前配置的 runaway 动作。
    action: Action,
}

impl RunawayChecker for Checker {
    type Action = Action;
    type Error = CheckError;
    type Request = Request;
    type RuDetails = RuDetails;

    fn before_executor(&self) -> Result<String, Self::Error> {
        Ok("watch-match".to_owned())
    }

    fn before_cop_request(&self, req: &mut Self::Request) -> Result<(), Self::Error> {
        // 模拟发送前改写：标记请求已限流。
        req.throttled = true;
        Ok(())
    }

    fn check_thresholds(
        &self,
        detail: &Self::RuDetails,
        process_keys: i64,
        err: Option<Self::Error>,
    ) -> Option<Self::Error> {
        // 累加处理 key；读 RU 超过 10 则判定超限。
        self.processed_keys
            .fetch_add(process_keys.max(0) as usize, Ordering::Relaxed);
        if detail.read_units > 10 {
            Some(CheckError("ru threshold exceeded"))
        } else {
            err
        }
    }

    fn reset_total_processed_keys(&self) {
        self.processed_keys.store(0, Ordering::Relaxed);
    }

    fn check_action(&self) -> Self::Action {
        self.action
    }

    fn check_rule_kill_action(&self) -> (String, bool) {
        ("kill rule".to_owned(), self.action == Action::Kill)
    }
}

/// `ConsumptionReporter` 的可断言桩：记录两次上报调用参数。
#[derive(Default)]
struct Reporter {
    /// `(资源组名, 消费)` 列表。
    reports: Mutex<Vec<(String, Consumption)>>,
    /// `(资源组名, tikv, tidb, tiflash)` RU v2 列表。
    ruv2_reports: Mutex<Vec<(String, f64, f64, f64)>>,
}

/// 测试用消费采样。
#[derive(Clone, Debug, PartialEq)]
struct Consumption(f64);

impl ConsumptionReporter for Reporter {
    type Consumption = Consumption;

    fn report_consumption(&self, resource_group_name: &str, consumption: &Self::Consumption) {
        self.reports
            .lock()
            .unwrap()
            .push((resource_group_name.to_owned(), consumption.clone()));
    }

    fn report_ruv2_consumption(
        &self,
        resource_group_name: &str,
        tikv_ruv2: f64,
        tidb_ruv2: f64,
        tiflash_ruv2: f64,
    ) {
        self.ruv2_reports.lock().unwrap().push((
            resource_group_name.to_owned(),
            tikv_ruv2,
            tidb_ruv2,
            tiflash_ruv2,
        ));
    }
}

/// 编译期断言类型满足 `Send + Sync`。
fn assert_send_sync<T: Send + Sync>() {}

/// 默认资源组名与 Go 常量一致。
#[test]
fn default_resource_group_name_matches_go() {
    assert_eq!(DEFAULT_RESOURCE_GROUP_NAME, "default");
}

/// 覆盖 runaway 检查契约：watch、cop 改写、阈值、动作与重置。
#[test]
fn runaway_checker_preserves_go_contract_and_concurrent_action() {
    assert_send_sync::<Checker>();
    let checker = Checker {
        processed_keys: AtomicUsize::new(0),
        action: Action::Kill,
    };
    let mut request = Request::default();

    assert_eq!(checker.before_executor().unwrap(), "watch-match");
    checker.before_cop_request(&mut request).unwrap();
    assert!(request.throttled);
    assert_eq!(
        checker.check_thresholds(&RuDetails::default(), 7, None),
        None
    );
    assert_eq!(checker.processed_keys.load(Ordering::Relaxed), 7);
    assert_eq!(
        checker.check_thresholds(&RuDetails { read_units: 11 }, 3, None),
        Some(CheckError("ru threshold exceeded"))
    );
    assert_eq!(checker.check_action(), Action::Kill);
    assert_eq!(
        checker.check_rule_kill_action(),
        ("kill rule".to_owned(), true)
    );
    checker.reset_total_processed_keys();
    assert_eq!(checker.processed_keys.load(Ordering::Relaxed), 0);
}

/// 验证两类 Go 侧消费上报形状均被正确转发并记录。
#[test]
fn consumption_reporter_forwards_both_go_report_shapes() {
    let reporter = Reporter::default();
    reporter.report_consumption("analytics", &Consumption(12.5));
    reporter.report_ruv2_consumption("analytics", 1.0, 2.0, 3.0);

    assert_eq!(
        *reporter.reports.lock().unwrap(),
        vec![("analytics".to_owned(), Consumption(12.5))]
    );
    assert_eq!(
        *reporter.ruv2_reports.lock().unwrap(),
        vec![("analytics".to_owned(), 1.0, 2.0, 3.0)]
    );
}
