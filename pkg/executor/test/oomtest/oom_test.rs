// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 对应 `pkg/executor/test/oomtest/oom_test.go`。
//
// Go 用例通过 zap Core hook 捕获 expensive_query / memory exceeds quota 日志，
// 再配合完整 TestKit SQL 验证 update/insert/replace/delete 路径的 MemTracker。
// Rust 用例执行相同 SQL，并从生产 BgLogger 内存 sink 断言真实 statement tracker
// 触发的日志；`oomCapture` 的字段解析与过滤契约另有独立回归测试。

#![allow(non_snake_case)]

use std::sync::OnceLock;

use astersql_testkit::{Rows, TestKit};
use astersql_util_logutil::log::BgLogger;
use astersql_util_memory::action::DefLogPriority;
use astersql_util_set::{NewStringSet, StringSet};
use astersql_util_syncutil::Mutex;

/// Go 用例里 expensive_query 日志消息原文，用作 messageFilter 命中样本。
const EXPENSIVE_QUERY: &str = "expensive_query during bootstrap phase";
/// rateLimitAction 把超限处理委托给 fallback 时的日志文案。
const RATE_LIMIT_DELEGATE: &str =
    "memory exceeds quota, rateLimitAction delegate to fallback action";

/// 对应 Go 包级 `var oom *oomCapture`。
static OOM: OnceLock<Mutex<OomCapture>> = OnceLock::new();
static SQL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 懒初始化并返回全局 `OomCapture`（加锁保护并发写 tracker）。
fn oom() -> &'static Mutex<OomCapture> {
    OOM.get_or_init(|| Mutex::new(OomCapture::new()))
}

/// 对应 Go `oomCapture`：筛选日志消息并保存 tracker 文本。
pub struct OomCapture {
    tracker: String,
    message_filter: StringSet,
}

impl OomCapture {
    fn new() -> Self {
        Self {
            tracker: String::new(),
            message_filter: NewStringSet(&[]),
        }
    }

    /// 对应 Go `AddMessageFilter`。
    pub fn AddMessageFilter(&mut self, vals: &[&str]) {
        for val in vals {
            self.message_filter.Insert((*val).to_owned());
        }
    }

    /// 对应 Go `ClearMessageFilter`。
    pub fn ClearMessageFilter(&mut self) {
        self.message_filter.Clear();
    }

    /// 对应 Go `SetTracker`。
    pub fn SetTracker(&mut self, tracker: &str) {
        self.tracker = tracker.to_owned();
    }

    /// 对应 Go `GetTracker`。
    pub fn GetTracker(&self) -> String {
        self.tracker.clone()
    }

    /// 对应 Go `Write`：解析 OOM 错误或按 messageFilter 记录 tracker。
    pub fn Write(
        &mut self,
        entry_message: &str,
        first_field_error: Option<&str>,
    ) -> Result<(), String> {
        if entry_message == "memory exceeds quota" {
            // Go 直接读取 fields[0] 并断言为 error；缺失字段或标记是测试
            // hook 自身的不变量破坏，必须 panic，不能降级成可忽略的返回错误。
            let err = first_field_error.expect("OOM error field missing");
            let begin = err
                .find("8001]")
                .unwrap_or_else(|| panic!("begin not found"));
            let end = err
                .find(" holds")
                .unwrap_or_else(|| panic!("end not found"));
            self.tracker = err[begin + "8001]".len()..end].to_owned();
            return Ok(());
        }
        if self.message_filter.Exist(entry_message) {
            self.tracker = entry_message.to_owned();
        }
        Ok(())
    }

    /// 对应 Go `Check`：仅在启用时把自身加入 CheckedEntry。
    pub fn Check(&self, enabled: bool) -> bool {
        enabled
    }
}

/// 对应 Go `registerHook`：初始化全局 oom 捕获器。
fn register_hook() {
    let _ = oom();
}

fn new_testkit() -> TestKit {
    TestKit::new(astersql_testkit::mockstore::CreateMockStoreAndDomain().0)
}

fn assert_log_since(entries_before: usize, expected: &str) {
    assert!(
        BgLogger().entries()[entries_before..]
            .iter()
            .any(|entry| entry.message == expected),
        "production log did not contain {expected:?}"
    );
}

fn assert_no_log_since(entries_before: usize, unexpected: &str) {
    assert!(
        BgLogger().entries()[entries_before..]
            .iter()
            .all(|entry| entry.message != unexpected),
        "production log unexpectedly contained {unexpected:?}"
    );
}

/// 对应 Go `TestMain`。
#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();
    register_hook();
}

/// 对应 Go `TestMemTracker4UpdateExec`。
#[test]
fn TestMemTracker4UpdateExec() {
    let _serial = SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    register_hook();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_MemTracker4UpdateExec (id int, a int, b int, index idx_a(`a`))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_MemTracker4UpdateExec values (1,1,1), (2,2,2), (3,3,3)",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_mem_quota_query = 244", Vec::new());
    let entries_before = BgLogger().entries().len();
    tk.MustExec("update t_MemTracker4UpdateExec set a = 4", Vec::new());
    assert_log_since(entries_before, EXPENSIVE_QUERY);
    tk.MustQuery(
        "select a from t_MemTracker4UpdateExec order by id",
        Vec::new(),
    )
    .Check(Rows(&["4", "4", "4"]));
}

/// 对应 Go `TestMemTracker4InsertAndReplaceExec`。
#[test]
fn TestMemTracker4InsertAndReplaceExec() {
    let _serial = SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    register_hook();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t (id int, a int, b int, index idx_a(`a`))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1), (2,2,2), (3,3,3)", Vec::new());
    tk.MustExec(
        "create table t_MemTracker4InsertAndReplaceExec \
         (id int, a int, b int, index idx_a(`a`))",
        Vec::new(),
    );

    for sql in [
        "insert into t_MemTracker4InsertAndReplaceExec values (1,1,1), (2,2,2), (3,3,3)",
        "replace into t_MemTracker4InsertAndReplaceExec values (1,1,1), (2,2,2), (3,3,3)",
        "insert into t_MemTracker4InsertAndReplaceExec select * from t",
        "replace into t_MemTracker4InsertAndReplaceExec select * from t",
    ] {
        tk.MustExec("set @@tidb_mem_quota_query = -1", Vec::new());
        let entries_before = BgLogger().entries().len();
        tk.MustExec(sql, Vec::new());
        assert_no_log_since(entries_before, EXPENSIVE_QUERY);

        tk.MustExec("set @@tidb_mem_quota_query = 1", Vec::new());
        let entries_before = BgLogger().entries().len();
        tk.MustExec(sql, Vec::new());
        assert_log_since(entries_before, EXPENSIVE_QUERY);
    }

    tk.MustExec("set @@tidb_dml_batch_size = 1", Vec::new());
    tk.MustExec("set @@tidb_batch_insert = 1", Vec::new());
    for sql in [
        "insert into t_MemTracker4InsertAndReplaceExec values (1,1,1), (2,2,2), (3,3,3)",
        "replace into t_MemTracker4InsertAndReplaceExec values (1,1,1), (2,2,2), (3,3,3)",
    ] {
        tk.MustExec("set @@tidb_mem_quota_query = -1", Vec::new());
        let entries_before = BgLogger().entries().len();
        tk.MustExec(sql, Vec::new());
        assert_no_log_since(entries_before, EXPENSIVE_QUERY);

        tk.MustExec("set @@tidb_mem_quota_query = 1", Vec::new());
        let entries_before = BgLogger().entries().len();
        tk.MustExec(sql, Vec::new());
        assert_log_since(entries_before, EXPENSIVE_QUERY);
    }
}

/// 对应 Go `TestMemTracker4DeleteExec`。
#[test]
fn TestMemTracker4DeleteExec() {
    let _serial = SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    register_hook();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table MemTracker4DeleteExec1 \
         (id int, a int, b int, index idx_a(a), index idx_b(b))",
        Vec::new(),
    );
    tk.MustExec(
        "create table MemTracker4DeleteExec2 \
         (id int, a int, b int, index idx_a(a), index idx_b(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into MemTracker4DeleteExec1 values \
         (1,1,1), (2,2,2), (3,3,3), (4,4,4), (5,5,5)",
        Vec::new(),
    );

    tk.MustExec("set @@tidb_mem_quota_query = -1", Vec::new());
    let entries_before = BgLogger().entries().len();
    tk.MustExec("delete from MemTracker4DeleteExec1", Vec::new());
    assert_no_log_since(entries_before, EXPENSIVE_QUERY);

    tk.MustExec(
        "insert into MemTracker4DeleteExec1 values (1,1,1), (2,2,2), (3,3,3)",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_mem_quota_query = 1", Vec::new());
    let entries_before = BgLogger().entries().len();
    tk.MustExec("delete from MemTracker4DeleteExec1", Vec::new());
    assert_log_since(entries_before, EXPENSIVE_QUERY);

    tk.MustExec("set @@tidb_mem_quota_query = 100000", Vec::new());
    tk.MustExec(
        "insert into MemTracker4DeleteExec1 values (1,1,1)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into MemTracker4DeleteExec2 values (1,1,1)",
        Vec::new(),
    );
    let delete_join = "delete MemTracker4DeleteExec1, MemTracker4DeleteExec2 \
        from MemTracker4DeleteExec1 join MemTracker4DeleteExec2 \
        on MemTracker4DeleteExec1.a=MemTracker4DeleteExec2.a";
    let entries_before = BgLogger().entries().len();
    tk.MustExec(delete_join, Vec::new());
    assert_no_log_since(entries_before, RATE_LIMIT_DELEGATE);

    tk.MustExec(
        "insert into MemTracker4DeleteExec1 values (1,1,1)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into MemTracker4DeleteExec2 values (1,1,1)",
        Vec::new(),
    );
    let _failpoint = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/store/copr/disableFixedRowCountHint",
        "return",
    );
    tk.MustExec("set @@tidb_enable_rate_limit_action = 1", Vec::new());
    tk.MustExec("set @@tidb_mem_quota_query = 500", Vec::new());
    let entries_before = BgLogger().entries().len();
    // Go calls ExecToErr but ignores its returned error. The query may succeed
    // or fail after spill-to-disk; this case only requires the OOM action log.
    let _result = tk.Exec(delete_join, Vec::new());
    assert_log_since(entries_before, RATE_LIMIT_DELEGATE);
}

/// 对应 Go `TestOOMActionPriority`：join 结束后 fallback 优先级为 DefLogPriority。
#[test]
fn TestOOMActionPriority() {
    let _serial = SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    for i in 0..5 {
        tk.MustExec(&format!("drop table if exists t{i}"), Vec::new());
        tk.MustExec(&format!("create table t{i}(a int)"), Vec::new());
        tk.MustExec(&format!("insert into t{i} values(1)"), Vec::new());
    }
    tk.MustQuery(
        "select * from t0 join t1 join t2 join t3 join t4 order by t0.a",
        Vec::new(),
    )
    .Check(Rows(&["1 1 1 1 1"]));
    assert_eq!(
        tk.Session().StatementOOMActionPriorityForTest(),
        Some(DefLogPriority)
    );
}

/// 验证 `Check` 仅透传 enabled 标志（对齐 Go CheckedEntry 启用语义）。
#[test]
fn oom_capture_check_respects_enabled_flag() {
    let capture = OomCapture::new();
    assert!(capture.Check(true));
    assert!(!capture.Check(false));
}

/// 对齐 Go `oomCapture.Write` 的正常 OOM 解析与 messageFilter 分支。
#[test]
fn oom_capture_write_matches_go_tracker_and_filter_contract() {
    let mut capture = OomCapture::new();
    capture.SetTracker("unchanged");
    capture.Write("unrelated background message", None).unwrap();
    assert_eq!(capture.GetTracker(), "unchanged");

    capture.AddMessageFilter(&[EXPENSIVE_QUERY]);
    capture.Write(EXPENSIVE_QUERY, None).unwrap();
    assert_eq!(capture.GetTracker(), EXPENSIVE_QUERY);

    capture.ClearMessageFilter();
    capture.SetTracker("");
    capture.Write(EXPENSIVE_QUERY, None).unwrap();
    assert_eq!(capture.GetTracker(), "");

    capture
        .Write(
            "memory exceeds quota",
            Some("[executor:8001]HashJoin holds 1024 bytes"),
        )
        .unwrap();
    assert_eq!(capture.GetTracker(), "HashJoin");
}

/// Go 直接访问 `fields[0]`；缺失 OOM error 字段属于 hook 不变量破坏并 panic。
#[test]
#[should_panic(expected = "OOM error field missing")]
fn oom_capture_panics_when_error_field_is_missing() {
    let _ = OomCapture::new().Write("memory exceeds quota", None);
}

/// Go 在 OOM 错误缺少 tracker 起始标记时明确 panic。
#[test]
#[should_panic(expected = "begin not found")]
fn oom_capture_panics_when_tracker_begin_marker_is_missing() {
    let _ = OomCapture::new().Write("memory exceeds quota", Some("malformed holds 1 byte"));
}

/// Go 在 OOM 错误缺少 ` holds` 终止标记时明确 panic。
#[test]
#[should_panic(expected = "end not found")]
fn oom_capture_panics_when_tracker_end_marker_is_missing() {
    let _ = OomCapture::new().Write("memory exceeds quota", Some("[executor:8001]tracker"));
}
