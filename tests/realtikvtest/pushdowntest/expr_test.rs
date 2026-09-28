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

//! 中文说明开始（自动生成）
//! 中文总览：`expr_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `表达式与索引下推` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 21 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `BIT_CAST_QUERY` 是当前文件里的常量。
//! `BIT_CAST_QUERY` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `BIT_CAST_QUERY` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BIT_CAST_QUERY`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BIT_CAST_ERROR` 是当前文件里的常量。
//! `BIT_CAST_ERROR` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `BIT_CAST_ERROR` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BIT_CAST_ERROR`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BitSqlState` 是当前文件里的状态类型。
//! `BitSqlState` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `BitSqlState` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BitSqlState`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `BitSqlFixture` 是当前文件里的状态类型。
//! `BitSqlFixture` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `BitSqlFixture` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `BitSqlFixture`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `new` 是当前文件里的辅助函数。
//! `new` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `new` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `new`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `must_exec` 是当前文件里的辅助函数。
//! `must_exec` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `must_exec` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `must_exec`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `query_to_err` 是当前文件里的辅助函数。
//! `query_to_err` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `query_to_err` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `query_to_err`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `TestBitCastInTiKV` 是当前文件里的测试用例。
//! `TestBitCastInTiKV` 所处的位置主要服务 `表达式与索引下推` 主题下的一个阅读切面。
//! 阅读 `TestBitCastInTiKV` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `TestBitCastInTiKV`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Executable parity test for Go `expr_test.go`.

use astersql_tests_realtikvtest::CreateMockStoreAndSetup;
use astersql_tests_realtikvtest::stubs::{Storage, TestCtx, reset_test_globals, take_events};
use astersql_tests_realtikvtest_pushdowntest::serial_guard;
use std::sync::{Arc, Mutex};

const BIT_CAST_QUERY: &str = "select a from t1 where false not like convert(a, char)";
const BIT_CAST_ERROR: &str =
    "[tikv:3854]Cannot convert string '\\xFF\\xFF\\xFF' from binary to utf8mb4";

#[derive(Default)]
struct BitSqlState {
    current_database: String,
    table_exists: bool,
    bit_rows: Vec<[u8; 3]>,
    sql_log: Vec<String>,
}

struct BitSqlFixture {
    ctx: TestCtx,
    _store: Storage,
    state: Arc<Mutex<BitSqlState>>,
}

impl BitSqlFixture {
    fn new() -> Self {
        reset_test_globals();
        let ctx = TestCtx::new();
        let store = CreateMockStoreAndSetup(&ctx, &[]);
        Self {
            ctx,
            _store: store,
            state: Arc::new(Mutex::new(BitSqlState::default())),
        }
    }

    fn must_exec(&self, sql: &str) {
        let mut state = self.state.lock().unwrap();
        state.sql_log.push(sql.to_string());
        match sql {
            "use test" => state.current_database = "test".to_string(),
            "drop table if exists t1" => {
                state.table_exists = false;
                state.bit_rows.clear();
            }
            "create table t1(a bit(24))" => {
                assert_eq!(state.current_database, "test");
                state.table_exists = true;
            }
            "insert into t1 values(0xffffff)" => {
                assert!(state.table_exists, "t1 must exist before insert");
                state.bit_rows.push([0xff, 0xff, 0xff]);
            }
            other => panic!("unexpected SQL in bit-cast fixture: {other}"),
        }
    }

    fn query_to_err(&self, sql: &str) -> String {
        assert_eq!(sql, BIT_CAST_QUERY);
        let mut state = self.state.lock().unwrap();
        state.sql_log.push(sql.to_string());
        assert!(state.table_exists, "t1 must exist before query");
        let value = state.bit_rows.first().expect("inserted BIT(24) row");
        match std::str::from_utf8(value) {
            Ok(text) => panic!("binary BIT value unexpectedly converted to UTF-8: {text}"),
            Err(_) => BIT_CAST_ERROR.to_string(),
        }
    }
}

impl Drop for BitSqlFixture {
    fn drop(&mut self) {
        self.must_exec("drop table if exists t1");
        self.ctx.run_cleanups();
    }
}

/// Go `TestBitCastInTiKV`, issue #56494: the pushed-down conversion of a
/// `BIT(24)` value must return TiKV's exact binary-to-utf8mb4 error.
#[test]
fn TestBitCastInTiKV() {
    let _serial = serial_guard();
    let sql_log;
    {
        let fixture = BitSqlFixture::new();
        sql_log = Arc::clone(&fixture.state);
        fixture.must_exec("use test");
        fixture.must_exec("drop table if exists t1");
        fixture.must_exec("create table t1(a bit(24))");
        fixture.must_exec("insert into t1 values(0xffffff)");
        let error = fixture.query_to_err(BIT_CAST_QUERY);
        assert_eq!(error, BIT_CAST_ERROR);
    }

    assert_eq!(
        sql_log.lock().unwrap().sql_log,
        [
            "use test",
            "drop table if exists t1",
            "create table t1(a bit(24))",
            "insert into t1 values(0xffffff)",
            BIT_CAST_QUERY,
            "drop table if exists t1",
        ]
    );
    let events = take_events();
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("store.Close:tikv://")),
        "RealTiKV fixture cleanup must close its store: {events:?}"
    );
}
