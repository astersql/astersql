# `pkg/store/mockstore/mockstorage/embedded_rpc.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-mockstorage` crate，是 canonical mock storage 与内嵌 UniStore RPC/MVCC 服务之间的适配层。crate 入口 `pkg/store/mockstore/mockstorage/lib.rs` 将模块声明为私有 `embedded_rpc`，但公开重导出 `EmbeddedRpcStore`；`pkg/store/mockstore/mockstorage/Cargo.toml` 则表明它直接依赖 `astersql-kv` 和相邻的 `astersql-store-mockstore-unistore`。

上游通过 `KVStore::NewEmbeddedRpc`（`storage.rs:311`）选择这条后端路径。普通 `KVStore` 仍使用本地 `BTreeMap` 版本链；只有 `KVStoreInner::embedded_rpc` 为 `Some` 时，`canonical_storage.rs` 的事务、快照读取和提交逻辑才调用本文件。因此它不是网络 TiKV 客户端，也不是完整 `kv::Storage` 实现，而是供 mock storage 复用 canonical 事务外壳的进程内 MVCC 后端。

## 核心职责

- `EmbeddedRpcStore` 保存共享的 `RPCClient` 与 `unistore::Cluster`，分别负责发请求和查询 Region/leader/store 元数据。
- `context` 根据键定位 Region leader，构造带 Region epoch、store ID、请求优先级和可选 marker 的 `RpcContext`。
- `get` 与 `scan` 把快照读转换为内嵌 RPC；`scan` 额外负责把全局范围拆成逐 Region 请求并合并结果。
- `commit` 用 `Prewrite` 加 `Commit` 实现简化的两阶段提交；`rollback` 按 Region 发送 `BatchRollback`。
- `value` 统一解释 `RpcResponse<T>` 的 Region 错误、键错误和成功值，并把可重试键错误映射为 `kv::ErrTxnRetryable`。
- `close` 将生命周期收尾委托给 `RPCClient::close`。

本文件刻意不管理事务本地写缓冲、快照缓存、时间戳分配或 `kv::Transaction` trait；这些职责位于 `canonical_storage.rs` 和 `storage.rs`。

## 主要符号

- `pub struct EmbeddedRpcStore { client: Arc<RPCClient>, cluster: Arc<unistore::Cluster> }`：模块唯一公开类型。字段私有，调用者只能取得克隆后的 client 或使用 crate 内读写接口。
- `error(message) -> kv::errors::SharedError`：把可显示消息封装为 KV 共享错误。
- `value<T>(RpcResponse<T>) -> Result<T, SharedError>`：按 `region_error`、`key_error`、`value` 的顺序取值。`key_error.retryable` 为真时保留 canonical KV 层可识别的重试分类，否则保留服务器消息；三者均无时报告缺失响应值。
- `EmbeddedRpcStore::new() -> crate::Result<Self>`：调用 `unistore::mock::New("", ..., NULL_KEYSPACE_ID, ...)` 创建临时目录上的 Server、RPC client、PD facade 和 Cluster；本类型只保留 client 与 cluster。构造错误转为 `MockStorageError::Rpc`。
- `client() -> Arc<RPCClient>`：向测试和上层集成暴露同一个共享 client，例如安装请求拦截器或读取请求 marker。
- `close() -> crate::Result<()>`：关闭 client，并把 UniStore RPC 错误转为 mock storage 错误。
- `context(key, priority, marker)`：完成键到 Region、leader、store 地址和请求上下文的映射。
- `send(address, Request)`：以固定 5 秒超时同步调用 `RPCClient::send_request`。
- `get(key, version, priority, marker)`：发送 `Request::Get`，仅接受 `Response::Get`。
- `scan(version, lower, upper, reverse, priority, marker)`：逐 Region 正向读取，最后按需整体反转。
- `groups(keys, priority, marker)`：以 Region ID 为键构造 `BTreeMap<u64, (address, context, keys)>`，供写阶段按 Region 批处理。
- `rollback(start_ts, keys, ...)`：每个分组发送 `Request::BatchRollback`，仅接受 `Response::Unit`。
- `commit(start_ts, commit_ts, writes, ...)`：把 `Some(value)` 转为 `Put`、`None` 转为 `Delete`，执行预写、失败清理、primary 提交和 secondary 提交。

文件没有条件编译项、trait 定义或模块级常量；公开面只有类型本身和 `client`，其余构造及数据操作限定在 crate 内。

## 执行流程

1. `KVStore::NewEmbeddedRpc` 创建普通 wall-clock TSO mock store，再调用 `EmbeddedRpcStore::new` 注入 `Arc`。`unistore::mock::New` 在空路径情况下创建临时目录并启动进程内 Server。
2. 事务或快照点读先在 `canonical_storage.rs` 处理本地写缓冲/快照缓存；需要访问持久 MVCC 状态时调用 `get`。`get` 经 `context` 定位 leader，随后同步发送 `Request::Get`，并由 `value` 解包。
3. 范围读由 `scan` 调用 Region manager 的 `scan_regions`。每个 Region 的请求区间取用户范围与 Region 范围的交集；空交集跳过。请求自身始终 `reverse: false`、`key_only: false`、`limit: usize::MAX`，所有键值对汇总后才在 `reverse == true` 时反转。
4. 事务扫描的“读己之写”不在本文件完成：`KVTxn::canonical_scan` 先取得这里的 RPC rows，再用本地 `writes` 覆盖、插入或删除，最后决定正反序。
5. `commit` 对空写集直接成功。非空时以 `BTreeMap` 的首键作为 primary，并按 Region 分组全部键。第一阶段对每组发送相同 `start_ts`、primary 和 3000 ms lock TTL 的 `PrewriteRequest`。
6. 任一预写失败时，对完整写集调用 `rollback`。回滚成功则返回原错误；回滚也失败则返回同时包含原错误和 cleanup 错误的消息。
7. 全部预写成功后先单独提交 primary，再按 Region 提交除 primary 外的 secondary，所有提交使用同一个 `commit_ts`。这是为了避免 secondary 先失败后又回滚一个已提交事务。
8. `KVTxn::Commit` 成功后才清空本地写集并使事务失效；`KVTxn::Rollback` 在事务仍有效时调用本文件的 `rollback`。`KVStore::Close` 首次关闭时调用本文件的 `close`。

下游 `RPCClient::dispatch`（`unistore/rpc.rs`）将这些请求分别路由到 Server 的 `kv_get`、`kv_scan`、`kv_prewrite`、`kv_commit` 与 `kv_batch_rollback`。

## 数据与状态

`EmbeddedRpcStore` 自身只有两个 `Arc` 字段，不保存写集或读取缓存。`client` 与 `cluster` 指向同一套内嵌服务及元数据视图；复制 `Arc` 不复制服务状态。

读请求携带：键或范围、MVCC `version`、Region 上下文、整型 priority 和可选 request marker。写请求携带：`start_ts`、`commit_ts`、按字节序排列的 `BTreeMap<Vec<u8>, Option<Vec<u8>>>` 写集，以及同样的 priority/marker。`Some` 表示 Put，`None` 表示 Delete；Delete 的 mutation value 使用空字节串，但操作类型仍明确为 `MutationOp::Delete`。

有三个重要顺序不变量：写集和 Region 分组均使用 `BTreeMap`，所以 primary 是字节序最小的键，Region 执行顺序也稳定；Region 内键保持输入迭代顺序；扫描按 Region 顺序拼接正向结果，反向扫描只在完整结果收集后反转。

`context` 每次请求都重新读取 Region manager，避免在本类型中缓存过期的 leader/epoch。当前实现不在 Region 错误后刷新并重试，而是把错误返回上层。

## 依赖与调用关系

直接上游如下：

- `storage.rs::KVStore::NewEmbeddedRpc` 构造本类型，`KVStore::EmbeddedRpc` 返回克隆后的 `Arc`，`KVStore::Close` 触发关闭。
- `canonical_storage.rs::KVTxn::Get` 和 `Snapshot::Get` 调用 `get`。
- `canonical_storage.rs` 中事务与快照的 `canonical_scan` 调用 `scan`。
- `canonical_storage.rs` 中 `impl kv::Transaction for Transaction` 的 `Commit` 调用 `commit`，`Rollback` 调用 `rollback`；`SetOption(kv::Priority, ...)` 通过 `client().request_marker()` 关联请求 marker。
- `pkg/executor/test/loaddatatest/main_test.rs::with_shared_load_data_store` 使用 `NewEmbeddedRpc`，并在共享 client 上安装 priority 检查拦截器，证明该适配层也用于上层 executor 测试接线。

直接下游如下：

- `unistore::mock::New` 创建内嵌 Server、mock PD、Cluster 和 `RPCClient`。
- `Cluster::region_manager` 提供 `get_region_by_key`、`scan_regions` 和 store 列表。
- `RPCClient::send_request` 检查 closed 状态、执行可选拦截器、验证地址并同步 dispatch；本文件固定传入 5 秒超时。
- `Request`/`Response` 与 `tikv::server::RpcResponse` 定义传输边界；`Mutation`/`PrewriteRequest` 定义 MVCC 写入负载。
- `astersql_kv::errors` 提供上层统一错误和 `ErrTxnRetryable` 分类。

RustCodeGraph 索引将本文件识别为 21 个符号，并确认 `EmbeddedRpcStore` 在 `lib.rs` 被导入/重导出；精确名称查询还定位到 `storage.rs::EmbeddedRpc` 和独立回归测试。当前索引的 `callers/callees` 命令对这些 impl 方法未输出边，因此方法级调用关系以上述已读取源码引用为准。

## 错误处理与边界

- 构造与关闭阶段错误使用 `MockStorageError::Rpc(String)`；数据路径使用 `kv::errors::SharedError`，以便满足 canonical KV trait。
- `value` 优先返回 Region 错误，其次键错误，最后成功值。可重试键错误会丢弃具体消息并归类为 `ErrTxnRetryable`，这是上层冲突重试判断所需行为；非可重试键错误和 Region 错误保留显示文本。
- 请求返回了错误的 `Response` 变体时，`get`、`scan`、`rollback`、预写和提交均立即返回带操作名的错误，避免错误地解释其他响应。
- `scan` 会逐项检查 `KvPair.error`，任一项失败就停止并丢弃尚未返回的整体结果。范围采用半开区间 `[lower, upper)`；无上界用空字节表示 open end。`start >= end` 的非空交集被跳过。
- `context` 对 Region 不存在、无 leader、leader 对应 store 不存在分别报错。本文件没有 Region retry/backoff、leader 刷新或请求拆分重试。
- 预写阶段失败会尽力回滚全部键；但 primary 提交后 secondary 提交失败时不会回滚，因为 primary 已经提交。调用者会得到错误，而底层可能需要后续锁解析才能完成 secondary；当前 mock 适配层没有恢复循环。
- 固定 5 秒 RPC 超时不会触发 `RPCClient` 中“短于 1 秒”的 deadline failpoint 分支；拦截器、closed、地址和服务器错误仍会传播。
- 空写集提交成功，空键集回滚不发送请求。`close` 后继续请求会由 client 返回 `RpcError::Cancelled`。

## 并发与资源生命周期

`EmbeddedRpcStore` 通过 `Arc` 共享 client 与 cluster，方法只借用 `&self`，自身不含可变容器或锁。同步的 `send` 不创建线程；多 Region 的 scan、prewrite、commit、rollback 都按顺序执行，没有并行 fan-out，也没有流式返回，所以大范围扫描会把全部结果保存在 `Vec` 中。

实际并发保护位于下游 `RPCClient`：`closed` 是 `AtomicBool`，请求拦截器位于 `RwLock<Option<_>>`，请求 marker 使用线程局部状态。`RPCClient::close` 通过原子 swap 幂等化关闭，停止 Server；若 `mock::New` 创建的是非持久临时目录，还会删除该目录。`KVStore::Close` 也只在首次从 open 切换到 closed 时调用它。

本类型未实现 `Drop`，因此调用者应显式执行 `KVStore::Close`。`new` 返回的 mock PD handle 被丢弃，但 client 与 cluster 持有运行请求所需的共享对象；这个所有权安排由 `unistore::mock::New` 和 `RPCClient` 的内部 `Arc` 保证。

## 与 Go 版本的对应关系

同目录 Go 文件 `pkg/store/mockstore/mockstorage/storage.go` 的 `mockStorage` 是对 `tikv.KVStore` 与 `copr.Store` 的包装；Go 事务由 client-go `KVStore.Begin` 创建，快照由 driver adapter 包装，关闭时依次关闭 coprocessor store 与 KVStore。Go 目录中没有 `embedded_rpc.go`，也没有与 `EmbeddedRpcStore` 一一对应的类型。

因此本文件属于 Rust canonical mock storage 为复用已移植 UniStore RPC/MVCC 服务而增加的局部接线，不应宣称是 Go 文件的逐函数翻译。语义目标仍与 Go 路径一致：通过 TiKV 风格的事务 RPC 提供快照读、两阶段提交、写冲突和幂等关闭；但 Rust 在此显式完成 Region 分组、响应分类及 primary-first 提交，而 Go 版本把这些细节交给 client-go。

迁移元数据 `Cargo.toml [package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/store/mockstore/mockstorage`（legacy task `task-819`）。现有 Rust 回归测试验证 canonical 行为，而不是逐行比对 Go 测试；未发现同名 Go 单测或同名 Rust `embedded_rpc_test.rs`。

## 扩展指南

- 新增 RPC 操作时，应先在 UniStore `Request`/`Response` 与 server dispatch 建立真实行为，再在本文件增加严格匹配响应变体的窄接口；不要绕过 `value` 的 Region/key 错误分类。
- 支持 Region retry、leader 变更或 backoff 时，优先围绕 `context`/`send` 设计，并明确哪些错误可重试、是否需要重新拆分范围；避免缓存 Region context 后静默使用过期 epoch。
- 修改扫描时要保持 `[lower, upper)`、跨 Region 去重/排序和 reverse 语义。若改为并行或流式扫描，需要同时处理顺序、首错取消、内存上限和 per-pair error。
- 修改提交协议时要维持同一 `start_ts/commit_ts`、全组预写后再提交、primary-first 以及预写失败清理；若增加 async commit、1PC、pessimistic lock 或 secondary 恢复，不能用当前简化路径冒充完整协议。
- 修改请求上下文时应同步 `context`、`canonical_storage.rs::SetOption` 及上层 marker/priority 测试，确保后台元数据事务不会错误继承前台 marker。
- 生产逻辑仍应留在本文件，测试逻辑放在独立 `canonical_storage_test.rs` 或新增独立 `*_test.rs`。最直接的回归入口是 `embedded_rpc_transactions_preserve_snapshot_buffer_and_conflict_errors`；新增多 Region、Region 错误、secondary commit 错误或 close 生命周期行为时应扩充独立测试文件。
- 兼容风险主要是错误分类和 Go/client-go 可观察事务语义；性能风险主要是 `usize::MAX` 无界扫描、顺序 Region RPC 和完整结果缓冲。

## 验证依据

- RustCodeGraph：`status` 显示目标在有效索引中；`files --filter pkg/store/mockstore/mockstorage` 列出 `embedded_rpc.rs`；`query Embedded`/`query embedded_rpc` 定位 `EmbeddedRpcStore`、全部方法、`storage.rs::EmbeddedRpc` 和回归测试；`node EmbeddedRpcStore` 验证字段及 `lib.rs` 导入边。方法级 `callers/callees` 无输出，未据此臆造调用边。
- 目标实现：`pkg/store/mockstore/mockstorage/embedded_rpc.rs`，核对了类型、辅助函数、读/扫、Region 分组、rollback 和两阶段 commit 的完整实现。
- crate 边界：`pkg/store/mockstore/mockstorage/Cargo.toml` 与 `lib.rs`，核对 crate 名、直接依赖、Go 包映射、私有模块和公开重导出。
- Rust 直接入口：`pkg/store/mockstore/mockstorage/storage.rs` 与 `canonical_storage.rs`，核对构造、持有、关闭、点读、扫描、本地写缓冲合并、提交、回滚、priority 与 marker 传播。
- 下游实现：`pkg/store/mockstore/unistore/mock.rs`、`rpc.rs`、`tikv/server.rs`，核对临时服务构造、请求分派、错误响应、拦截器、原子关闭和临时目录清理。
- 独立 Rust 测试：`pkg/store/mockstore/mockstorage/canonical_storage_test.rs::embedded_rpc_transactions_preserve_snapshot_buffer_and_conflict_errors`，覆盖本地写缓冲扫描、快照隔离、可重试写冲突、RPC 拦截错误不被误判为 key not found，以及显式关闭；`pkg/executor/test/loaddatatest/main_test.rs::with_shared_load_data_store` 提供 priority 拦截器的上层接线证据。
- Go 对照：`pkg/store/mockstore/mockstorage/storage.go`，核对 Go wrapper 的 Begin、GetSnapshot 与 Close；目录搜索确认没有同名 Go 实现/测试，故文档明确记录为包级语义对应而非逐函数复刻。
- 按任务约束未运行 Cargo。交付结构以固定 11 个二级标题检查，并人工复核无整段源码复制、无测试内嵌建议、无未经证实的支持声明。
