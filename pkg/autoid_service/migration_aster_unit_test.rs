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

// autoid_service 迁移单元测试。
//
// 本文件验证从 Go(TiDB) 迁移到 Rust 的自增 ID 分配服务（autoid service）
// 与原 Go 实现在行为上保持一致，覆盖以下方面：
// - 有符号/无符号列的 ID 段分配、rebase（重设基准值）与溢出行为；
// - 并发分配时同一张表内 ID 段的串行化；
// - keyspace（键空间，多租户下的数据隔离单元）校验与 owner
//   （集群中唯一负责分配 ID 的节点）缓存重置；
// - `mock_for_test` 按存储 UUID 复用服务实例；
// - gRPC 服务端遵循真实的 kvproto 协议契约。
//
// 测试通过内存实现 `MemoryStore` 替代真实的 TiKV 存储，
// 从而在不依赖外部组件的情况下驱动完整的分配逻辑。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::{AutoIdStorage, Service, create_grpc_service, mock_for_test};
use autoid_dependency::{AutoIdKey, AutoIdKeyKind, IdStore, IdTransaction, Result as AutoIdResult};
use grpcio::{ChannelBuilder, Environment, ServerBuilder, ServerCredentials};
use kvproto::autoid::{
    AutoIDRequest_oneof_keyspace, AutoIdAllocClient, AutoIdRequest, AutoIdResponse, RebaseRequest,
};

/// 测试用的数据库 ID。
const DB_ID: i64 = 41;
/// 测试用的表 ID。
const TABLE_ID: i64 = 73;
/// 空 keyspace 标识：表示不启用键空间隔离（非多租户模式）。
const NULLSPACE_ID: u32 = u32::MAX;

/// 内存版 ID 存储，模拟真实实现中由 TiKV 承载的自增 ID 持久化层。
///
/// 用 `Mutex<HashMap>` 保存每个 `AutoIdKey`（库 ID + 表 ID + 自增类型）
/// 对应的当前分配上界，供测试在无外部依赖的环境下运行。
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
    keyspace_id: u32,
    uuid: String,
}

impl MemoryStore {
    /// 创建一个指定 UUID 与 keyspace ID 的内存存储。
    fn new(uuid: &str, keyspace_id: u32) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            keyspace_id,
            uuid: uuid.to_owned(),
        }
    }

    /// 构造测试表自增列（auto_increment）对应的存储键。
    fn increment_key() -> AutoIdKey {
        AutoIdKey {
            database_id: DB_ID,
            table_id: TABLE_ID,
            kind: AutoIdKeyKind::IncrementId(5),
        }
    }

    /// 直接写入全局分配上界，模拟其他节点已在存储层推进了 ID 分配进度。
    fn set_global_end(&self, value: i64) {
        self.values
            .lock()
            .unwrap()
            .insert(Self::increment_key(), value);
    }
}

/// 内存版事务句柄：在持有存储锁期间对键值表做读写，
/// 模拟真实实现中一次 TiKV 事务（保证原子性的读写操作序列）内的操作。
struct MemoryTransaction<'a> {
    values: &'a mut HashMap<AutoIdKey, i64>,
}

/// 为内存事务实现 `IdTransaction` 接口：提供读取、写入、自增与复制键值的能力。
impl IdTransaction for MemoryTransaction<'_> {
    fn get(&self, key: AutoIdKey) -> AutoIdResult<i64> {
        Ok(*self.values.get(&key).unwrap_or(&0))
    }

    fn put(&mut self, key: AutoIdKey, value: i64) -> AutoIdResult<()> {
        self.values.insert(key, value);
        Ok(())
    }

    fn inc(&mut self, key: AutoIdKey, step: i64) -> AutoIdResult<i64> {
        // 使用 wrapping_add 允许回绕，与无符号 ID 溢出行为的测试场景相配合。
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

/// 为内存存储实现 `IdStore`：以互斥锁模拟事务的互斥与原子提交语义。
impl IdStore for MemoryStore {
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

/// 为内存存储实现 `AutoIdStorage`：提供服务标识（UUID）、keyspace ID
/// 以及 etcd（集群元数据协调组件）命名空间前缀。
impl AutoIdStorage for MemoryStore {
    fn uuid(&self) -> &str {
        &self.uuid
    }

    fn keyspace_id(&self) -> u32 {
        self.keyspace_id
    }

    fn etcd_namespace(&self) -> String {
        // 未启用 keyspace 时返回空前缀，否则按 keyspace ID 构造隔离的命名空间路径。
        if self.keyspace_id == NULLSPACE_ID {
            String::new()
        } else {
            format!("/keyspaces/tidb/{}/", self.keyspace_id)
        }
    }
}

/// 构造一次 ID 分配请求。
///
/// - `unsigned`：目标列是否为无符号类型；
/// - `n`：本次申请的 ID 数量（0 表示只查询当前进度，不实际分配）；
/// - `increment` / `offset`：自增步长与偏移，对应 MySQL 的
///   `auto_increment_increment` / `auto_increment_offset` 语义。
fn request(unsigned: bool, n: u64, increment: i64, offset: i64) -> AutoIdRequest {
    AutoIdRequest {
        db_id: DB_ID,
        tbl_id: TABLE_ID,
        is_unsigned: unsigned,
        n,
        increment,
        offset,
        keyspace: Some(AutoIDRequest_oneof_keyspace::KeyspaceId(NULLSPACE_ID)),
        ..Default::default()
    }
}

/// 构造一次 rebase 请求：把自增基准值调整到 `base`。
/// `force` 为 true 时无条件覆盖，否则只允许把基准值往前推进。
fn rebase(unsigned: bool, base: i64, force: bool) -> RebaseRequest {
    RebaseRequest {
        db_id: DB_ID,
        tbl_id: TABLE_ID,
        is_unsigned: unsigned,
        base,
        force,
        ..Default::default()
    }
}

/// 断言分配响应成功且返回的 ID 区间为 `(min, max]`。
fn assert_range(response: AutoIdResponse, min: i64, max: i64) {
    assert!(response.errmsg.is_empty(), "{:?}", response.errmsg);
    assert_eq!((response.min, response.max), (min, max));
}

/// 验证分配 API 与 Go 版本在有符号/无符号、rebase 及溢出场景下行为一致。
///
/// 依次覆盖：基础的连续分配、按步长与偏移分配、非强制/强制 rebase 的推进与回退语义、
/// 有符号列到达 `i64::MAX` 后的分配报错，以及无符号列在 `i64::MAX` 处
/// 回绕（按位回绕到 `i64::MIN`，对应无符号视角下的继续递增）直至 `-1`
/// （即无符号最大值）后的分配报错。
#[test]
fn migration_api_matches_go_signed_unsigned_rebase_and_overflow_behavior() {
    let store = Arc::new(MemoryStore::new("api", NULLSPACE_ID));
    let service = Service::new_mock(store);

    // 有符号列的基础分配：区间左开右闭，n=0 仅探测当前进度。
    assert_range(service.allocate(request(false, 1, 1, 1)).unwrap(), 0, 1);
    assert_range(service.allocate(request(false, 10, 1, 1)).unwrap(), 1, 11);
    assert_range(service.allocate(request(false, 0, 0, 0)).unwrap(), 11, 11);
    assert_range(
        service.allocate(request(false, 128, 1, 1)).unwrap(),
        11,
        139,
    );
    assert_range(
        service.allocate(request(false, 1, 10, 5)).unwrap(),
        139,
        145,
    );

    // 非强制 rebase 只能把基准值向前推进；随后分配从新基准值开始。
    assert!(
        service
            .rebase_ids(rebase(false, 666, false))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(service.allocate(request(false, 1, 1, 1)).unwrap(), 666, 667);
    assert!(
        service
            .rebase_ids(rebase(false, 6666, false))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(
        service.allocate(request(false, 1, 1, 1)).unwrap(),
        6666,
        6667,
    );
    // 非强制 rebase 到更小的值（44）不生效，进度保持在 6667。
    assert!(
        service
            .rebase_ids(rebase(false, 44, false))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(
        service.allocate(request(false, 0, 0, 0)).unwrap(),
        6667,
        6667,
    );
    // 强制 rebase 允许把基准值回退到 44。
    assert!(
        service
            .rebase_ids(rebase(false, 44, true))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(service.allocate(request(false, 0, 0, 0)).unwrap(), 44, 44);

    // 有符号列 rebase 到 i64::MAX 之后，再申请新 ID 必须报错（有符号上界溢出）。
    assert!(
        service
            .rebase_ids(rebase(false, i64::MAX, true))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(
        service.allocate(request(false, 0, 0, 0)).unwrap(),
        i64::MAX,
        i64::MAX,
    );
    assert!(
        !service
            .allocate(request(false, 1, 1, 1))
            .unwrap()
            .errmsg
            .is_empty()
    );

    // 切换到无符号列：强制 rebase 归零后重复上面的基础分配序列。
    assert!(
        service
            .rebase_ids(rebase(true, 0, true))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(service.allocate(request(true, 0, 0, 0)).unwrap(), 0, 0);
    assert_range(service.allocate(request(true, 1, 1, 1)).unwrap(), 0, 1);
    assert_range(service.allocate(request(true, 10, 1, 1)).unwrap(), 1, 11);
    assert_range(service.allocate(request(true, 128, 1, 1)).unwrap(), 11, 139);
    assert_range(service.allocate(request(true, 1, 10, 5)).unwrap(), 139, 145);

    // 无符号列在 i64::MAX 之后按位回绕到 i64::MIN，
    // 即无符号视角下越过有符号上界后仍可继续递增分配。
    assert!(
        service
            .rebase_ids(rebase(true, i64::MAX, false))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(
        service.allocate(request(true, 0, 0, 0)).unwrap(),
        i64::MAX,
        i64::MAX,
    );
    assert_range(
        service.allocate(request(true, 1, 1, 1)).unwrap(),
        i64::MAX,
        i64::MIN,
    );
    assert_range(
        service.allocate(request(true, 1, 1, 1)).unwrap(),
        i64::MIN,
        i64::MIN + 1,
    );
    // 无符号列 rebase 到 -1（无符号最大值 u64::MAX 的位模式）后，再分配必须报错。
    assert!(
        service
            .rebase_ids(rebase(true, -1, false))
            .unwrap()
            .errmsg
            .is_empty()
    );
    assert_range(service.allocate(request(true, 0, 0, 0)).unwrap(), -1, -1);
    assert!(
        !service
            .allocate(request(true, 1, 1, 1))
            .unwrap()
            .errmsg
            .is_empty()
    );
}

/// Go 的 `errAutoincReadFailed` 是普通错误，服务会把其原始文本直接写入响应。
#[test]
fn exhausted_auto_increment_preserves_the_go_error_message() {
    let store = Arc::new(MemoryStore::new("exact-error", NULLSPACE_ID));
    let service = Service::new_mock(store);

    assert!(
        service
            .rebase_ids(rebase(false, i64::MAX, true))
            .unwrap()
            .errmsg
            .is_empty()
    );
    let response = service.allocate(request(false, 1, 1, 1)).unwrap();

    assert_eq!(response.errmsg, b"auto increment action failed");
}

/// 验证同一张表上的并发分配被串行化：多个线程各取一个 ID，
/// 得到的区间应互不重叠且恰好连续覆盖 [666, 696)。
#[test]
fn migration_concurrent_allocations_are_serialized_per_table() {
    let store = Arc::new(MemoryStore::new("concurrent", NULLSPACE_ID));
    let service = Service::new_mock(store);
    service.rebase_ids(rebase(true, 666, false)).unwrap();

    // 启动 30 个线程并发申请 ID，每个线程各申请 1 个。
    let mut workers = Vec::new();
    for _ in 0..30 {
        let service = service.clone();
        workers.push(thread::spawn(move || {
            service.allocate(request(false, 1, 1, 1)).unwrap()
        }));
    }
    let mut ranges = workers
        .into_iter()
        .map(|worker| {
            let response = worker.join().unwrap();
            assert!(response.errmsg.is_empty());
            (response.min, response.max)
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();

    // 排序后应正好是 666..696 的 30 个互不重叠的连续单位区间。
    assert_eq!(
        ranges,
        (666..696).map(|min| (min, min + 1)).collect::<Vec<_>>()
    );
    assert_range(service.allocate(request(false, 0, 0, 0)).unwrap(), 696, 696);
}

/// 验证 keyspace 校验与 owner 缓存重置行为与 Go 版服务一致：
/// 请求的 keyspace 与存储不匹配时报 "not leader"；
/// 重新成为 owner 后应丢弃本地缓存段，从存储中读取最新进度。
#[test]
fn migration_keyspace_gate_and_owner_cache_reset_match_go_service() {
    let store = Arc::new(MemoryStore::new("keyspace", 17));
    let service = Service::new_mock(store.clone());

    // 请求携带 NULLSPACE_ID，与存储的 keyspace 17 不匹配，应被拒绝。
    let error = service.allocate(request(false, 1, 1, 1)).unwrap_err();
    assert!(error.to_string().contains("not leader"));

    // 修正 keyspace 后分配成功。
    let mut matching = request(false, 1, 1, 1);
    matching.set_keyspace_id(17);
    assert_range(service.allocate(matching.clone()).unwrap(), 0, 1);

    // 模拟其他节点把存储层进度推进到 9000；本地缓存段未失效前仍返回旧进度 1。
    store.set_global_end(9000);
    assert_range(
        service
            .allocate(AutoIdRequest {
                n: 0,
                ..matching.clone()
            })
            .unwrap(),
        1,
        1,
    );
    // 重新成为 owner 会清空本地缓存，之后读取到存储中的最新进度 9000。
    service.on_become_owner();
    assert_range(
        service
            .allocate(AutoIdRequest { n: 0, ..matching })
            .unwrap(),
        9000,
        9000,
    );
}

/// 验证 `mock_for_test` 按存储 UUID 复用同一服务实例：
/// 两次以相同 UUID 获取的服务共享分配进度。
#[test]
fn migration_mock_for_test_reuses_the_service_by_store_uuid() {
    let store = Arc::new(MemoryStore::new("shared-mock", NULLSPACE_ID));
    let first = mock_for_test(store.clone());
    assert_range(first.allocate(request(false, 1, 1, 1)).unwrap(), 0, 1);

    let second = mock_for_test(store);
    assert_range(second.allocate(request(false, 0, 0, 0)).unwrap(), 1, 1);
}

/// 验证 gRPC 服务端遵循真实的 kvproto 协议契约：
/// 在随机端口启动服务后，用生成的 `AutoIdAllocClient` 客户端
/// 走完整的网络调用完成一次 ID 分配。
#[test]
fn migration_grpc_service_serves_the_real_kvproto_contract() {
    let store = Arc::new(MemoryStore::new("grpc", NULLSPACE_ID));
    let service = Service::new_mock(store);
    // 搭建 gRPC 服务端并监听随机端口（端口号 0 表示由系统分配）。
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
    // 通过真实的 gRPC 通道连接服务端并发起分配请求。
    let channel = ChannelBuilder::new(environment).connect(&format!("127.0.0.1:{port}"));
    let client = AutoIdAllocClient::new(channel);

    assert_range(
        client.alloc_auto_id(&request(false, 1, 1, 1)).unwrap(),
        0,
        1,
    );
}
