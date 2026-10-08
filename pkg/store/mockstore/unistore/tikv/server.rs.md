# `pkg/store/mockstore/unistore/tikv/server.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-tikv` crate 的进程内 mock TiKV 服务聚合层。crate 由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 声明，`lib.rs` 以 `pub mod server` 暴露本模块；它不是网络监听器，也没有直接实现 protobuf/gRPC trait，而是向上游提供普通 Rust 方法。`pkg/store/mockstore/unistore/server/server.rs` 的 `setup_stand_alone_inner_server` 创建 `MvccStore` 和 `StandAloneInnerServer` 后构造 `Arc<Server>`，`pkg/store/mockstore/unistore/rpc.rs::RPCClient` 再把进程内请求分派到这些方法。

该层位于“请求适配”与“具体存储/Region 实现”之间：同步入口是 `RPCClient::dispatch`，批量入口是 `tikv/server_batch.rs::handle_batch_request`；本文件负责 Region 上下文校验、写操作 latch 包装、响应错误归一化，以及 Raw KV、MPP 和死锁检测等 mock 状态。真正的 MVCC 算法在 `mvcc.rs`，Region 路由与 latch 在 `region.rs`/`mock_region.rs`，Raft/Snapshot 生命周期占位在 `inner_server.rs`。

## 核心职责

1. 以 `Server` 聚合 `RegionManager`、`MvccStore`、`InnerServer`、可选 `CoprocessorHandler` 和 `DetectorServer`，给进程内客户端提供统一服务面。
2. 对事务 KV 请求先通过 `request_region` 验证请求的 Region/Store/Epoch；写路径经 `with_latches` 按键哈希加锁，调用 `MvccStore` 后无论成功或返回 `MvccError` 都释放 latch。
3. 用 `RpcResponse<T>` 把成功值、`KeyError`、`RegionError` 分成互斥通道，并由 `convert_to_key_error` 赋予 MVCC 错误重试或中止语义。
4. 在独立内存容器中实现 Raw KV 的 TTL、扫描、CAS、范围删除和 checksum，以及 MPP 任务的创建、投包、取消、连接和移除生命周期。
5. 将 Coprocessor、Region split、死锁检测及 `raft`/`batch_raft`/`snapshot` 请求转发给对应下游；`stop` 负责幂等关闭所持资源。

## 主要符号

- `IsolationLevel`：请求隔离级别枚举，默认 `SnapshotIsolation`；另有 `ReadCommitted` 和 `RcCheckTs`。当前文件保存该值但不据此分支，具体读语义由下游及上游请求构造决定。
- `RpcContext`：携带 `priority`、请求本地 `request_marker`、`RequestContext`、隔离级别、`resolved_locks` 和 `committed_locks`。本文件的读接口实际向 `MvccStore` 传递 `resolved_locks`；`priority`、`request_marker`、`committed_locks` 在本文件中不消费。
- `KeyError`：统一记录死锁、锁冲突、写冲突及 `retryable`/`abort` 分类。
- `RpcResponse<T>`：`ok`、`from_mvcc`、`from_region` 三个私有构造器分别填充成功值、键错误或 Region 错误。
- `CoprocessorHandler`：`Send + Sync` 扩展接口；`handle(request, start_ts, region)` 接收原始载荷和已验证的 `RegionContext`。
- `MppTask`：以 `(store_id, task_id)` 唯一定位的内存任务，保存 payload、取消位和已缓冲 packets。
- `Server`：核心聚合类型。`new` 初始化空 Raw/MPP 状态和未停止标志；`with_coprocessor` 以 builder 方式挂载处理器；`mvcc_store` 返回共享存储句柄。
- `request_region`、`with_latches`：事务请求的内部公共前置流程。后者用于悲观锁、悲观回滚、心跳、事务状态检查、prewrite/flush/commit、cleanup 和 batch rollback。
- `kv_*`、`mvcc_get_*`：对 `MvccStore` 的事务读写、锁管理、GC、范围删除和调试查询门面。
- `raw_*`：基于 `RwLock<BTreeMap<Vec<u8>, (Vec<u8>, Option<u64>)>>` 的有序 Raw KV 实现。
- `create_mpp_task`、`dispatch_mpp_packet`、`cancel_mpp_task`、`establish_mpp_connection`、`remove_mpp_task`：MPP 内存任务状态机。
- `convert_to_key_error`：将 `MvccError` 映射到 mock RPC 错误分类。
- `injected_pessimistic_deadlock`：读取 `pessimisticLockReturnDeadlock` failpoint，在 Region 校验前为首个 mutation 合成死锁。
- `hash`：Raw checksum 使用的 FNV-1a 64 位哈希辅助函数。

## 执行流程

典型事务请求沿 `RPCClient::send_request` → `RPCClient::dispatch` → `Server::kv_*` 运行；批请求则由 `BatchRequestHandler::dispatch_batch` 为每项请求启动 scoped thread，再由 `handle_batch_request` 调用相同 `Server` API。

读流程以 `kv_get` 为例：先由 `request_region` 调用 `RegionManager::get_region_from_context`；失败立即生成仅含 `region_error` 的响应；成功后调用 `MvccStore::get(key, version, resolved_locks)`，值或 `MvccError` 分别进入 `RpcResponse::ok` 或 `from_mvcc`。`kv_scan`、`kv_batch_get`、锁扫描和 MVCC 调试查询遵循同样的“校验后直接读取”结构。

写流程以 `kv_prewrite` 为例：从 mutations 收集键，进入 `with_latches`；该函数先验证 Region，再由 `keys_to_hash_values` 生成 latch key，在 `RegionContext::acquire_latches` 与 `release_latches` 之间执行 `MvccStore::prewrite`，最后统一转换返回值。`kv_commit` 等其他写接口复用这一骨架。需要注意，闭包正常返回时 latch 一定释放，但若下游 panic，本函数没有 RAII guard，不能保证 unwind 时释放。

`kv_pessimistic_lock` 在上述流程之前执行 failpoint：启用且存在 mutation 时，以首键、`start_ts + 1`（wrapping）和键哈希构造 `MvccError::Deadlock`，因此可在无效/默认 Region 上下文之前返回键错误；没有 mutation 时即使 failpoint 启用也继续正常 Region 校验。

Raw 路径不进入 Region 或 MVCC。`raw_put` 把相对 TTL 饱和相加为绝对过期时间；`raw_get`、`raw_get_key_ttl`、`raw_scan` 在读取时隐藏 `expires <= now_ts` 的条目，但不主动从 map 删除。扫描利用 `BTreeMap` 键序，空 `end` 表示无上界，之后按需反转、截断并在 `key_only` 时清空值。`raw_checksum` 对可见行的 key/value 哈希做 XOR，并累计行数和字节数。

MPP 路径在单个 mutex 下操作任务表：创建拒绝重复键；投包拒绝不存在或已取消任务；取消只置位；建立连接克隆当前 packets；移除才释放任务。Coprocessor 路径先校验 Region，再调用可选 handler；未挂载或 handler 返回字符串错误时生成 `abort = true` 的 `KeyError`。

## 数据与状态

`Server` 的核心共享状态均可跨线程：三个服务依赖使用 `Arc`；Raw 数据用 `RwLock<BTreeMap<...>>` 保持排序并允许并发读；MPP 任务用 `Mutex<HashMap<...>>` 保证复合状态变更原子化；`stopped: AtomicBool` 控制一次性关闭；`DetectorServer` 持有死锁检测状态。`Server` 自身没有 `Drop` 实现，生命周期关闭依赖上游显式调用 `stop`；`RPCClient::Drop` 会调用其 `close`，后者再调用 `Server::stop`。

事务数据不保存在本文件，而在共享 `MvccStore` 中。Region latch 也不属于 `Server`；`with_latches` 从已解析的 `RegionContext` 获取并释放。`RpcContext.resolved_locks` 会影响 `kv_get`、`kv_scan`、`kv_batch_get` 的锁可见性；`committed_locks` 在本文件没有读点，不能据此宣称已实现其完整语义。

Raw 过期项是“逻辑不可见、物理仍保留”；CAS 不接收 `now_ts`，直接比较 map 中旧值并在成功后写入无 TTL 的新值，因此它不会自行把过期项视为不存在。MPP `establish_mpp_connection` 返回 packets 的克隆而非消费队列，重复建立连接会看到相同缓冲内容，直到任务被移除。

## 依赖与调用关系

上游直接证据：

- `pkg/store/mockstore/unistore/server/server.rs::{setup_stand_alone_inner_server,new_mock,new}` 负责构建 `Arc<Server>`；standalone 路径先启动 `InnerServer` 并设置死锁检测状态。
- `pkg/store/mockstore/unistore/rpc.rs::RPCClient::dispatch` 将事务、Cop、MPP、MVCC 调试和 Region split 请求转给本文件；`RPCClient::close` 调用 `stop`。该客户端的常规 Raw 请求走其独立 `RawHandler`，不是本文件的 `Server::raw_*`。
- `pkg/store/mockstore/unistore/tikv/server_batch.rs::handle_batch_request` 调用事务和 Raw API；`BatchRequestHandler::dispatch_batch` 并发执行这些调用。

下游直接证据：

- `RegionManager`：Region 上下文解析、地址/Store ID 映射、split 和关闭。
- `RegionContext`：键哈希 latch 的获取与释放。
- `MvccStore`：版本读写、悲观锁/2PC、事务状态、GC、范围删除和调试信息。
- `InnerServer`：`stop`、`raft`、`batch_raft`、`snapshot`。
- `DetectorServer::detect`：死锁请求处理。
- `CoprocessorHandler::handle`：可替换的下推计算边界。

`Cargo.toml` 表明 crate 的普通依赖仅显式列出启用 failpoints 的 `fail`；大量 AsterSQL 内部依赖只在 Windows target 节声明。就本文件的直接编译依赖而言，大部分类型来自本 crate 模块，failpoint 来自 `fail`。该 manifest 事实不等价于“服务只支持 Windows”，只能说明当前依赖声明的条件布局。

## 错误处理与边界

`RpcResponse<T>` 的设计意图是成功、键错误、Region 错误三选一；本文件所有构造路径遵守这一约定。`convert_to_key_error` 的分类为：`Deadlock` 填 `deadlock`，但不标 retry/abort；`KeyLocked` 可重试并携带 `Lock`；`WriteConflict`/`CommitTsExpired` 可重试并填 conflict；`AlreadyExists`/`AlreadyCommitted` 中止；其余错误默认中止且不可重试。

Region 校验通常先于存储访问，但有两个显著边界：`kv_pessimistic_lock` 的注入死锁早于 Region 校验；`split_region` 直接按 `context.region.region_id` 调用 manager，由 manager 自行返回 `RegionError`。Raw 和 MPP API 返回 `Option`、tuple 或 `Result<_, String>`，不使用 `RpcResponse`。

锁 poison 被视为不可恢复：Raw/MPP 的 `.read()`、`.write()`、`.lock()` 都以 `expect` panic。Coprocessor 缺失、handler 失败及 MPP 非法状态使用字符串错误，类型信息有限。`stop` 先原子置 stopped，再关闭 MVCC 和 Region manager，最后返回 `inner_server.stop()`；前两者无返回错误，若最后一步失败，后续 `stop` 因幂等标志已设置而直接成功，不会重试 InnerServer。

当前文件是 mock 行为面而非完整 TiKV 协议面：没有网络超时、protobuf 编解码、请求引用计数等待或完整 gRPC streaming 语义。新增行为不得把 mock 的字符串错误或内存状态误认为生产 TiKV 保证。

## 并发与资源生命周期

事务写并发由 Region latch 串行化相同键哈希上的冲突；多键请求一次取得全部哈希。读路径不取 latch，依靠 MVCC 版本和锁检查。批处理器会并发调用 `&Server`，因此 `MvccStore`、Region 实现以及本文件的锁容器必须维持 `Send + Sync` 约束。

Raw 的单次读写在 `RwLock` 临界区完成；`raw_scan` 持有读锁完成键收集、TTL 过滤、复制和截断。CAS 在一次写锁临界区内比较并写入，因而对本 map 原子。MPP 的每个操作都持有全局任务表 mutex，简单可靠但不同任务之间也会相互串行；连接会在锁内克隆全部 packets，数据量大会延长临界区。

`stop` 通过 `AtomicBool::swap(Ordering::AcqRel)` 保证只有首个调用执行关闭序列，`main_test.rs::standalone_server_stop_is_idempotent_and_closes_resources` 验证两次调用只关闭底层 bundle 一次。与 Go 版本不同，Rust `stop` 没有 refCount 等待循环；因此上游应在不再发起请求后关闭。MPP 生命周期测试验证重复创建、取消后拒绝投包/连接、移除后不存在。Raw 测试验证无界扫描和 TTL 边界。

## 与 Go 版本的对应关系

同路径 `server.go` 是语义对照源。共同点包括：`Server` 聚合 MVCC、Region manager 和 inner server；事务 RPC 先建立/校验请求上下文；悲观锁支持 `pessimisticLockReturnDeadlock`，使用首 mutation 并令 lock TS 为 start version 加一；错误转换区分 locked、retryable/conflict、already-exists、deadlock、commit-expired 等；服务关闭依次覆盖 MVCC、Region manager 和 inner server；MPP 任务以 store/task 标识管理。

Rust 版本不是 Go gRPC 实现的逐方法等价复制：

- Go `Server` 嵌入 `tikvpb.UnimplementedTikvServer` 并实现 protobuf RPC；Rust 暴露进程内类型，协议适配位于 `rpc.rs`。
- Go `Stop` 设置 stopped 后等待 `refCount == 0`，并记录各关闭错误；Rust 使用 `AtomicBool` 幂等，不等待在途引用，只向上传播 `InnerServer::stop` 错误。
- Go 悲观锁路径还包含 write-conflict failpoint、lock waiter、等待图清理和唤醒重试；这些逻辑没有出现在本文件的 `kv_pessimistic_lock` 中，Rust 仅注入 deadlock 后委托 `MvccStore`。
- Go Raw RPC 中存在 TODO/未实现项（例如 `RawGetKeyTTL` 返回空响应、`RawChecksum` panic）；Rust 本文件提供可工作的内存 TTL/checksum，但常规 `RPCClient` Raw 分派使用独立 `RawHandler`，不能宣称这些方法完整替代 Go RPC。
- Go MPP 使用 `MPPTaskHandler`、tunnel、异步 Cop 执行和带重试的连接建立；Rust 这里只保存 payload/packet 向量和取消位，是测试所需的轻量内存模型。
- Go `convertToKeyError` 保留 protobuf 结构化字段；Rust `KeyError` 是较粗粒度的内部表示。两者分类意图相近，但字段完备性不同。

因此扩展时应以对应 Go 方法的真实行为作为兼容目标，同时确认 Rust 上游请求枚举和下游 `MvccStore` 已具备所需能力，不能仅在本门面制造“已支持”的响应。

## 扩展指南

- 新增事务 RPC：先在 `RpcContext`/请求类型中明确所需字段；读操作复用 `request_region`，会改写数据的操作优先复用 `with_latches`，并确认所有触及键都参与哈希。随后同步接线 `rpc.rs::Request/Response/RPCClient::dispatch` 与需要的 `server_batch.rs::BatchCommand`，测试放入同目录独立 `*_test.rs`，不要内嵌到生产文件。
- 扩展错误类型：同时修改 `MvccError` 来源、`convert_to_key_error` 分类和上游 response 适配；逐项核对 Go `convertToKeyError`，明确 retry/abort 兼容性，避免把未知错误误标成可重试。
- 扩展 Raw：决定新接口应属于 `Server::raw` 还是 `RPCClient::raw_handler`，当前存在两套状态，误接会造成写后读不可见；TTL/CAS 组合尤其要明确过期项是否物理删除、是否视作不存在。
- 扩展 MPP：现有全局 mutex、packet clone 和字符串错误仅适合 mock。若加入阻塞流、容量控制或异步执行，应先定义取消/移除竞态和唤醒协议，并对照 Go handler/tunnel 生命周期。
- 扩展 Coprocessor：实现 `CoprocessorHandler` 并在构造链调用 `with_coprocessor`；测试未挂载、Region 错误和 handler 错误三个分支。
- 修改关闭流程：保持幂等，并针对“部分关闭失败后能否重试”作显式设计；同步 `main_test.rs` 的资源计数断言。

建议同步或扩充的独立测试位置是 `server_test.rs`（failpoint、TTL 边界）和 `main_test.rs`（生命周期、MPP、Raw、Region）；批分派行为放在 `server_batch_test.rs`。兼容风险主要是 Go/Rust 错误字段及请求顺序差异，性能风险主要是多键 latch 范围、Raw 长时间读锁和 MPP 全局 mutex/整包克隆。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore/tikv` 确认目标及 Go/测试邻接文件已索引；`query Server --kind struct`、`query kv_get --kind function --json`、`query with_latches --kind function --json`、`query convert_to_key_error --kind function`、`query injected_pessimistic_deadlock --kind function` 定位本文件主要符号；`node --file pkg/store/mockstore/unistore/tikv/server.rs --offset 120 --limit 120` 核对 `Server`、关闭、Region 校验、latch 和 `kv_get` 源码，并报告该文件被 15 个文件使用。当前索引的 `callers`/`callees` 命令在限定时间内未返回结果，故调用边又以以下直接源码引用复核。
- 目标源码：`pkg/store/mockstore/unistore/tikv/server.rs`，完整读取 780 行，核对所有类型、trait、函数、impl 与 failpoint；文件无条件编译属性，条件依赖布局来自 Cargo manifest。
- crate/入口：`pkg/store/mockstore/unistore/tikv/Cargo.toml`、`pkg/store/mockstore/unistore/tikv/lib.rs`、`pkg/store/mockstore/unistore/server/server.rs`。
- 上游调用：`pkg/store/mockstore/unistore/rpc.rs`、`pkg/store/mockstore/unistore/tikv/server_batch.rs`。
- Rust 测试：`pkg/store/mockstore/unistore/tikv/server_test.rs` 验证 TTL 到期边界和死锁注入先于 Region 校验；`pkg/store/mockstore/unistore/tikv/main_test.rs` 验证 stop 幂等、MPP 生命周期、Raw 无界扫描/TTL 和 Region 上下文校验。
- Go 对照：`pkg/store/mockstore/unistore/tikv/server.go` 的 `Server`/`NewServer`/`Stop`、`KvPessimisticLock`、Raw、MPP 与 `convertToKeyError`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务给定命令验证文档存在且恰有 11 个固定二级标题，并人工检查未把 Go 完整协议能力或未接线能力写成 Rust 当前事实。
