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

// `ProcessInfo` 浅拷贝行为的单元测试。
//
// 验证 Clone 后标量字段相等、函数指针与 Arc 字段仍指向同一对象（浅拷贝而非深拷贝）。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use astersql_session_sessmgr::ProcessInfo;
use astersql_session_sessmgr::memory::{NewTracker, Tracker};
use astersql_session_sessmgr::stmtctx::{NewStmtCtx, ReferenceCount, StatementContext};

/// 占位统计回调，供 StatsInfo 字段浅拷贝断言使用。
fn my_func(_: &dyn Any) -> HashMap<String, u64> {
    HashMap::new()
}

/// 构造含共享 Arc 字段的 ProcessInfo，断言 Clone 为浅拷贝。
#[test]
fn test_process_info_shallow_cp() {
    let mem: Arc<Tracker> = Arc::from(NewTracker(-1, -1));
    // 写入约 1.875GiB 用量，确保 MemTracker 状态非零。
    mem.Consume((1_i64 << 30) + (1_i64 << 29) + (1_i64 << 28) + (1_i64 << 27));

    let ref_count = Arc::new(ReferenceCount::default());
    let stmt_ctx: Arc<StatementContext> = Arc::from(NewStmtCtx());
    let info = ProcessInfo {
        ID: 233,
        User: "PingCAP".to_owned(),
        Host: "127.0.0.1".to_owned(),
        DB: "Database".to_owned(),
        Info: "select * from table where a > 1".to_owned(),
        CurTxnStartTS: 23333,
        StatsInfo: Some(my_func),
        StmtCtx: Some(Arc::clone(&stmt_ctx)),
        RefCountOfStmtCtx: Some(Arc::clone(&ref_count)),
        MemTracker: Some(Arc::clone(&mem)),
        RedactSQL: String::new(),
        SessionAlias: "alias123".to_owned(),
        ..ProcessInfo::default()
    };

    let cp = info.Clone();
    assert!(!std::ptr::addr_eq(&cp, &info));
    assert_eq!(cp.ID, info.ID);
    assert_eq!(cp.User, info.User);
    assert_eq!(cp.Host, info.Host);
    assert_eq!(cp.DB, info.DB);
    assert_eq!(cp.Info, info.Info);
    assert_eq!(cp.CurTxnStartTS, info.CurTxnStartTS);

    // reflect.DeepEqual couldn't cmp two function pointer.
    // 函数指针无法用 DeepEqual；改为比较函数地址。
    assert!(std::ptr::fn_addr_eq(
        cp.StatsInfo.unwrap(),
        info.StatsInfo.unwrap()
    ));

    // Arc 字段应仍指向同一底层对象。
    assert!(Arc::ptr_eq(cp.StmtCtx.as_ref().unwrap(), &stmt_ctx));
    assert!(Arc::ptr_eq(
        cp.RefCountOfStmtCtx.as_ref().unwrap(),
        &ref_count
    ));
    assert!(Arc::ptr_eq(cp.MemTracker.as_ref().unwrap(), &mem));
    assert_eq!(cp.RedactSQL, info.RedactSQL);
    assert_eq!(cp.SessionAlias, info.SessionAlias);
}
