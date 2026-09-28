// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Port of `pkg/autoid_service/autoid_test.go`.
// Uses an in-memory `AutoIdStorage` that preserves Go alloc / rebase / concurrency /
// gRPC semantics. Full mockstore+testkit schema bootstrap is not required because the
// service only keys allocations by `(db_id, tbl_id)`.
//
// autoid 服务（自增 ID 分配服务）的测试模块，移植自 Go 版
// `pkg/autoid_service/autoid_test.go`。
//
// 术语说明：
// - 自增 ID（auto id）：数据库为表的自增列批量预分配的单调递增整数区间，
//   分配结果以 `(min, max]` 半开区间返回，客户端在区间内自行发号；
// - rebase（重定基准）：把某张表的自增计数器调整到指定基准值，默认只允许
//   增大，带 `force` 标志时才允许回退（减小）；
// - keyspace（键空间）：NextGen 架构下用于多租户隔离的命名空间，classic
//   架构没有该概念，用哨兵值 `NULLSPACE_ID` 表示；
// - gRPC：跨进程远程调用协议，autoid 服务通过它对外提供分配接口。
//
// 测试通过内存版存储 `MemoryStore` 替代真实 TiKV 后端，只验证分配、
// 重定基准、并发安全与 gRPC 通道这几类语义。

use std::collections::HashMap;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use astersql_config_kerneltype::{IsClassic, IsNextGen};
use autoid_dependency::{AutoIdKey, IdStore, IdTransaction, Result as AutoIdResult};
use grpcio::{ChannelBuilder, Environment, ServerBuilder, ServerCredentials};
use kvproto::autoid::{
    AutoIdAllocClient, AutoIdRequest, AutoIdResponse, RebaseRequest, RebaseResponse,
};

use crate::{AutoIdStorage, Service, create_grpc_service, mock_for_test};

/// tikv `NullspaceID` / classic default keyspace.
/// classic 架构下表示“无 keyspace”的哨兵值（`u32::MAX`）。
const NULLSPACE_ID: u32 = u32::MAX;
/// NextGen SYSTEM keyspace id used by Go `TestAPI` / `TestGRPC`.
/// NextGen 架构下 SYSTEM 键空间的固定 ID，供 `TestAPI` / `TestGRPC` 场景使用。
const SYSTEM_KEYSPACE_ID: u32 = 0xFFFFFF - 1;

/// Fixed ids standing in for Go `test` / `t1` (or `t`) table meta ids.
/// 固定的数据库/表元数据 ID，代替 Go 测试中通过建库建表得到的真实 ID。
const DB_ID: i64 = 1;
/// 与 [`DB_ID`] 配套的固定表 ID。
const TABLE_ID: i64 = 100;

/// 分配目标：以 `(db_id, tbl_id)` 二元组标识“为哪张表分配自增 ID”。
#[derive(Clone, Copy)]
struct Dest {
    db_id: i64,
    tbl_id: i64,
}

/// 内存版自增 ID 存储，实现 `IdStore` 与 `AutoIdStorage`，
/// 用 `HashMap` + 互斥锁模拟真实 TiKV 后端的键值存取与事务语义。
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
    keyspace_id: u32,
    uuid: String,
}

impl MemoryStore {
    fn new(uuid: &str, keyspace_id: u32) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            keyspace_id,
            uuid: uuid.to_owned(),
        }
    }
}

/// 内存版“事务”：直接借用锁内的 HashMap 做读写。
/// 由于整个操作在同一把互斥锁内执行，天然满足事务的原子性要求
/// （事务 = 一组要么全部生效、要么全部不生效的读写操作）。
struct MemoryTransaction<'a> {
    values: &'a mut HashMap<AutoIdKey, i64>,
}

impl IdTransaction for MemoryTransaction<'_> {
    fn get(&self, key: AutoIdKey) -> AutoIdResult<i64> {
        Ok(*self.values.get(&key).unwrap_or(&0))
    }

    fn put(&mut self, key: AutoIdKey, value: i64) -> AutoIdResult<()> {
        self.values.insert(key, value);
        Ok(())
    }

    fn inc(&mut self, key: AutoIdKey, step: i64) -> AutoIdResult<i64> {
        let value = self.get(key)?.wrapping_add(step);
        self.values.insert(key, value);
        Ok(value)
    }

    fn copy_to(&mut self, from: AutoIdKey, to: AutoIdKey) -> AutoIdResult<()> {
        let value = self.get(from)?;
        self.values.insert(to, value);
        Ok(())
    }
}

impl IdStore for MemoryStore {
    /// 在“事务”中执行给定操作：先取得互斥锁，再把锁内数据包装为
    /// `MemoryTransaction` 交给回调，锁的独占性保证了操作的原子性。
    fn run_in_transaction(
        &self,
        operation: &mut dyn FnMut(&mut dyn IdTransaction) -> AutoIdResult<()>,
    ) -> AutoIdResult<()> {
        let mut values = self.values.lock().unwrap();
        operation(&mut MemoryTransaction {
            values: &mut values,
        })
    }
}

impl AutoIdStorage for MemoryStore {
    fn uuid(&self) -> &str {
        &self.uuid
    }

    fn keyspace_id(&self) -> u32 {
        self.keyspace_id
    }

    /// 返回 etcd（分布式协调服务）中的命名空间前缀：
    /// classic（无 keyspace）返回空串，NextGen 按 keyspace_id 拼接路径。
    fn etcd_namespace(&self) -> String {
        if self.keyspace_id == NULLSPACE_ID {
            String::new()
        } else {
            format!("/keyspaces/tidb/{}/", self.keyspace_id)
        }
    }
}

/// 自增 ID 分配请求的结果包装：同时携带响应体与调用层错误，
/// 便于断言时区分“RPC 失败”和“业务 errmsg”两类错误。
struct AutoIdResp {
    response: AutoIdResponse,
    error: Option<String>,
}

impl AutoIdResp {
    /// 断言分配成功且返回的区间恰好为 `(minv, maxv]`。
    fn check(self, minv: i64, maxv: i64) {
        assert!(self.error.is_none(), "{:?}", self.error);
        assert!(
            self.response.errmsg.is_empty(),
            "{:?}",
            self.response.errmsg
        );
        assert_eq!(
            (self.response.min, self.response.max),
            (minv, maxv),
            "AutoIDResponse mismatch"
        );
    }

    /// 断言 RPC 本身成功，但业务层返回了非空错误信息
    /// （例如自增值已达上限、无法继续分配的场景）。
    fn check_errmsg(self) {
        assert!(self.error.is_none(), "{:?}", self.error);
        assert!(
            !self.response.errmsg.is_empty(),
            "expected non-empty errmsg"
        );
    }

    /// 断言 RPC 本身成功，且业务错误文案与 Go 端完全一致。
    fn check_errmsg_eq(self, expected: &str) {
        assert!(self.error.is_none(), "{:?}", self.error);
        assert_eq!(String::from_utf8_lossy(&self.response.errmsg), expected);
    }
}

/// rebase（重定基准）请求的结果包装，结构与 [`AutoIdResp`] 对应。
struct RebaseResp {
    response: RebaseResponse,
    error: Option<String>,
}

impl RebaseResp {
    /// 断言 RPC 成功且业务错误信息恰好等于 `msg`（空串表示无错误）。
    fn check(self, msg: &str) {
        assert!(self.error.is_none(), "{:?}", self.error);
        let errmsg = String::from_utf8_lossy(&self.response.errmsg);
        assert_eq!(errmsg.as_ref(), msg);
    }
}

/// 查询当前自增计数器的值并断言等于预期。
/// 约定 `n = 0` 的分配请求不消耗任何 ID，仅回读当前值，
/// 因此返回的 `min` 与 `max` 应相等且等于当前计数器。
fn check_curr_value(cli: &Service, to: Dest, minv: i64, maxv: i64, keyspace_id: u32) {
    let req = AutoIdRequest {
        db_id: to.db_id,
        tbl_id: to.tbl_id,
        n: 0,
        keyspace_id,
        ..Default::default()
    };
    let resp = cli.allocate(req).expect("AllocAutoID");
    assert!(resp.errmsg.is_empty(), "{:?}", resp.errmsg);
    assert_eq!((resp.min, resp.max), (minv, maxv));
}

/// 发起一次自增 ID 分配请求。
///
/// - `unsigned`：自增列是否为无符号类型（影响溢出回绕行为）；
/// - `n`：本次要分配的 ID 个数；
/// - `more`：可选的 `[increment, offset]` 参数，对应 MySQL 的
///   `auto_increment_increment`（步长）与 `auto_increment_offset`（起始偏移），
///   缺省均为 1。
fn auto_id_request(
    cli: &Service,
    to: Dest,
    unsigned: bool,
    n: u64,
    keyspace_id: u32,
    more: &[i64],
) -> AutoIdResp {
    // 解析可变参数：more[0] 为步长，more[1] 为偏移，均可省略。
    let mut increment = 1_i64;
    let mut offset = 1_i64;
    if !more.is_empty() {
        increment = more[0];
    }
    if more.len() >= 2 {
        offset = more[1];
    }
    let req = AutoIdRequest {
        db_id: to.db_id,
        tbl_id: to.tbl_id,
        is_unsigned: unsigned,
        n,
        increment,
        offset,
        keyspace_id,
        ..Default::default()
    };
    match cli.allocate(req) {
        Ok(response) => AutoIdResp {
            response,
            error: None,
        },
        Err(err) => AutoIdResp {
            response: AutoIdResponse::default(),
            error: Some(err.to_string()),
        },
    }
}

/// 发起一次 rebase（重定基准）请求，把自增计数器调整到基准值 `n`。
/// `force = false` 时只允许把计数器调大；`force = true` 时允许强制回退。
fn rebase_request(cli: &Service, to: Dest, unsigned: bool, n: i64, force: bool) -> RebaseResp {
    let req = RebaseRequest {
        db_id: to.db_id,
        tbl_id: to.tbl_id,
        base: n,
        is_unsigned: unsigned,
        force,
        ..Default::default()
    };
    match cli.rebase_ids(req) {
        Ok(response) => RebaseResp {
            response,
            error: None,
        },
        Err(err) => RebaseResp {
            response: RebaseResponse::default(),
            error: Some(err.to_string()),
        },
    }
}

/// Corresponds to Go `TestConcurrent`.
///
/// 并发正确性测试：30 个线程同时各分配 1 个自增 ID，
/// 验证服务内部的互斥保护使计数器最终恰好增加 30，既不丢失也不重复。
#[test]
fn test_concurrent() {
    let keyspace_id = if IsClassic() {
        NULLSPACE_ID
    } else {
        // use keyspace ID of SYSTEM
        SYSTEM_KEYSPACE_ID
    };
    let store = Arc::new(MemoryStore::new("concurrent", keyspace_id));
    let cli = mock_for_test(store);
    let to = Dest {
        db_id: DB_ID,
        tbl_id: TABLE_ID,
    };

    const CONCURRENCY: i64 = 30;
    // 用 Barrier（栅栏）让 30 个工作线程与主线程同时起跑，
    // 确保所有分配请求真正并发发出，而不是被启动顺序串行化。
    let start = Arc::new(Barrier::new(CONCURRENCY as usize + 1));
    let mut workers = Vec::new();
    for _ in 0..CONCURRENCY {
        let cli = cli.clone();
        let start = start.clone();
        workers.push(thread::spawn(move || {
            start.wait();
            auto_id_request(&cli, to, false, 1, keyspace_id, &[])
        }));
    }

    // Rebase to some value
    // 先把计数器重定基准到 666，作为并发分配前的已知起点
    rebase_request(&cli, to, true, 666, false).check("");
    check_curr_value(&cli, to, 666, 666, keyspace_id);
    // And +1 concurrently for 30 times
    // 放行栅栏，30 个线程并发各 +1，随后逐一确认无错误
    start.wait();
    for worker in workers {
        let resp = worker.join().unwrap();
        assert!(resp.error.is_none(), "{:?}", resp.error);
        assert!(
            resp.response.errmsg.is_empty(),
            "{:?}",
            resp.response.errmsg
        );
    }
    // Check the result is increased by 30
    check_curr_value(&cli, to, 666 + CONCURRENCY, 666 + CONCURRENCY, keyspace_id);
}

/// Corresponds to Go `TestAPI`.
///
/// API 功能测试入口：按当前编译的架构类型（classic 无 keyspace /
/// NextGen 带 keyspace）选择对应场景执行同一套用例。
#[test]
fn test_api() {
    if IsClassic() {
        // Testing scenarios without keyspace.
        test_api_with_keyspace(None);
    }

    if IsNextGen() {
        // Testing scenarios with keyspace.
        test_api_with_keyspace(Some(SYSTEM_KEYSPACE_ID));
    }
}

/// API 用例主体：覆盖基本分配、步长/偏移分配、rebase（含 force 回退）、
/// 有符号/无符号溢出边界等场景。`keyspace_id` 为 `None` 表示 classic
/// 无 keyspace 场景。
fn test_api_with_keyspace(keyspace_id: Option<u32>) {
    let (req_keyspace_id, store_keyspace_id) = match keyspace_id {
        None => (NULLSPACE_ID, NULLSPACE_ID),
        Some(id) => (id, id),
    };

    let store = Arc::new(MemoryStore::new(
        &format!("api-{store_keyspace_id}"),
        store_keyspace_id,
    ));
    let cli = mock_for_test(store);
    let to = Dest {
        db_id: DB_ID,
        tbl_id: TABLE_ID,
    };

    // basic auto id operation
    auto_id_request(&cli, to, false, 1, req_keyspace_id, &[]).check(0, 1);
    auto_id_request(&cli, to, false, 10, req_keyspace_id, &[]).check(1, 11);
    check_curr_value(&cli, to, 11, 11, req_keyspace_id);
    auto_id_request(&cli, to, false, 128, req_keyspace_id, &[]).check(11, 139);
    auto_id_request(&cli, to, false, 1, req_keyspace_id, &[10, 5]).check(139, 145);

    // basic rebase operation
    rebase_request(&cli, to, false, 666, false).check("");
    auto_id_request(&cli, to, false, 1, req_keyspace_id, &[]).check(666, 667);

    rebase_request(&cli, to, false, 6666, false).check("");
    auto_id_request(&cli, to, false, 1, req_keyspace_id, &[]).check(6666, 6667);

    // rebase will not decrease the value without 'force'
    rebase_request(&cli, to, false, 44, false).check("");
    check_curr_value(&cli, to, 6667, 6667, req_keyspace_id);
    rebase_request(&cli, to, false, 44, true).check("");
    check_curr_value(&cli, to, 44, 44, req_keyspace_id);

    // max increase 1
    // 有符号列的上限边界：计数器已到 i64::MAX 时再分配应返回业务错误
    rebase_request(&cli, to, false, i64::MAX, true).check("");
    check_curr_value(&cli, to, i64::MAX, i64::MAX, req_keyspace_id);
    auto_id_request(&cli, to, false, 1, req_keyspace_id, &[]).check_errmsg();

    // 以下切换为无符号（unsigned）语义重跑基本分配用例
    rebase_request(&cli, to, true, 0, true).check("");
    check_curr_value(&cli, to, 0, 0, req_keyspace_id);
    auto_id_request(&cli, to, true, 1, req_keyspace_id, &[]).check(0, 1);
    auto_id_request(&cli, to, true, 10, req_keyspace_id, &[]).check(1, 11);
    auto_id_request(&cli, to, true, 128, req_keyspace_id, &[]).check(11, 139);
    auto_id_request(&cli, to, true, 1, req_keyspace_id, &[10, 5]).check(139, 145);

    // max increase 1
    // 无符号列内部仍以 i64 存储：越过 i64::MAX 后按位回绕到 i64::MIN，
    // 对应无符号视角下的继续递增（u64 的高半区）
    rebase_request(&cli, to, true, i64::MAX, false).check("");
    check_curr_value(&cli, to, i64::MAX, i64::MAX, req_keyspace_id);
    auto_id_request(&cli, to, true, 1, req_keyspace_id, &[]).check(i64::MAX, i64::MIN);
    auto_id_request(&cli, to, true, 1, req_keyspace_id, &[]).check(i64::MIN, i64::MIN + 1);

    // -1 在无符号语义下即 u64 最大值
    rebase_request(&cli, to, true, -1, false).check("");
    check_curr_value(&cli, to, -1, -1, req_keyspace_id);
    // rebase to max value, the next request should fail
    // 已重定基准到无符号最大值，下一次分配应返回业务错误
    auto_id_request(&cli, to, true, 1, req_keyspace_id, &[])
        .check_errmsg_eq("[autoid:1467]Failed to read auto-increment value from storage engine");
}

/// Corresponds to Go `TestGRPC`.
///
/// Go spins etcd for leadership; the Rust port uses `Service::new_mock` (always owner)
/// and serves the real kvproto gRPC contract, matching the integration intent of
/// `AllocAutoID` over the wire.
///
/// gRPC 集成测试：Go 版依赖 etcd 选主（leader 选举，保证同一时刻只有一个
/// 节点提供分配服务）；Rust 移植版用 `Service::new_mock` 直接充当 owner，
/// 重点验证真实 kvproto gRPC 协议链路上的 `AllocAutoID` 调用可用。
#[test]
fn test_grpc() {
    let keyspace_id = if IsClassic() {
        NULLSPACE_ID
    } else {
        // use keyspace ID of SYSTEM
        SYSTEM_KEYSPACE_ID
    };

    // 在本地随机端口启动真实 gRPC 服务端，注册 autoid 服务
    let store = Arc::new(MemoryStore::new("grpc", keyspace_id));
    let service = Service::new_mock(store);
    let environment = Arc::new(Environment::new(1));
    let grpc_service = create_grpc_service(service);
    let mut server = ServerBuilder::new(environment.clone())
        .register_service(grpc_service)
        .build()
        .unwrap();
    let port = server
        .add_listening_port("127.0.0.1:0", ServerCredentials::insecure())
        .unwrap();
    server.start();

    // 通过 gRPC 客户端走网络发起一次真实的分配调用并检查无业务错误
    let channel = ChannelBuilder::new(environment).connect(&format!("127.0.0.1:{port}"));
    let cli = AutoIdAllocClient::new(channel);
    let resp = cli
        .alloc_auto_id(&AutoIdRequest {
            db_id: 0,
            tbl_id: 0,
            n: 1,
            increment: 1,
            offset: 1,
            is_unsigned: false,
            keyspace_id,
            ..Default::default()
        })
        .expect("AllocAutoID over gRPC");
    assert!(resp.errmsg.is_empty(), "{:?}", resp.errmsg);
}
