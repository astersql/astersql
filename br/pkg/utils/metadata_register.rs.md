# `br/pkg/utils/metadata_register.rs`

## 文件定位

当前 RustCodeGraph 显示该文件被 `lightning/pkg/importer/import.rs` 使用；其 `registerTaskToPD` 用本适配器构造 Lightning 任务注册器。

## 核心职责

- `MetadataRegisterClient` 保存一个可克隆的、带 keyspace 命名空间的真实 etcd 客户端。
- 每次操作前，私有方法 `client` 都从调用方的 `stubs::context::Context` 创建 `MetadataContext`，把取消状态传递到元数据 RPC。
- `impl EtcdRegisterClient` 将状态机要求的七项操作映射到真实客户端：写入、授予租约、持续续租、单次续租、读取、撤销租约和查询 TTL。
- 在两种接口形态之间做必要转换：字节串与 `String`、元数据客户端返回值与 `register.rs` 的响应结构、元数据错误与 `SharedError`。
- 为持续续租补上一条同步通道和后台线程，因为 `NamespacedEtcdClient::keepalive` 只执行一次续租请求，而注册状态机期待可持续接收回执的 `Receiver<LeaseKeepAliveResponse>`。

该文件不决定注册 key、默认 TTL、重建租约阈值或关闭顺序；这些策略都在 `register.rs` 的 `TaskRegisterImpl` 中。

## 主要符号

- `pub struct MetadataRegisterClient(pub NamespacedEtcdClient)`：唯一公开类型。元组字段也是公开的，构造方式是 `MetadataRegisterClient(client)`；派生 `Clone` 后，各副本共享 `NamespacedEtcdClient` 内部的 etcd session、RPC worker、endpoint 与 namespace。
- `MetadataRegisterClient::client(&self, ctx: &Context) -> NamespacedEtcdClient`：私有辅助函数。克隆 BR 上下文，并通过 `MetadataContext::with_cancellation_checker` 把 `ctx.is_cancelled()` 安装到克隆客户端。`with_context` 只替换该客户端副本的调用上下文，不改变共享 session。
- `impl EtcdRegisterClient for MetadataRegisterClient`：公开行为边界，实现 `put`、`grant`、`keep_alive`、`keep_alive_once`、`get`、`revoke`、`time_to_live`。所有方法都返回 `SharedError`，以适配 `TaskRegisterImpl` 的统一错误路径。
- `keep_alive` 内部创建 `std::sync::mpsc` 通道并启动线程；线程按“查询 TTL → 发送回执 → 等待约 TTL/3 或取消 → 发起一次 keepalive”的顺序循环。

本文件没有模块级常量、trait 定义、条件编译项或额外状态类型。

## 执行流程

真实生产主链目前是：

1. `lightning/pkg/importer/import.rs::registerTaskToPD` 通过 `dialEtcdWithCfg` 得到 `NamespacedEtcdClient`，该客户端已经包含解析后的 keyspace namespace。
2. 调用方构造 `MetadataRegisterClient(client.clone())`，再传给 `register.rs::NewTaskRegister`；注册类型为 `RegisterLightning`，任务名为随机 UUID。
3. `TaskRegisterImpl::RegisterTask` 依次调用本适配器的 `grant`、`put` 和 `keep_alive`。`put` 把空字符串值绑定到新租约；底层 `NamespacedEtcdClient::put` 自动在 key 前附加 namespace。
4. 本文件的 `keep_alive` 立即执行一次底层 keepalive。成功后启动生产回执的线程并把接收端交给 `TaskRegisterImpl::keepalive_loop`。
5. 适配线程先用 `time_to_live` 确认租约仍有效并上报当前 TTL；随后调用 `Context::wait_cancelled_timeout`，等待 `max(ttl / 3, 1)` 秒；未取消时再调用一次底层 `keepalive`。TTL 非正、查询/续租失败、调用方丢弃 receiver 或上下文取消都会结束线程并关闭通道。
6. `TaskRegisterImpl::keepalive_loop` 观察到通道断开后，依据自身的剩余 TTL 策略重建 keepalive 流，必要时重新 grant 并 put。同一策略也服务于模拟客户端，因此不在本文件重复实现。
7. Lightning 返回的清理闭包只执行一次：先调用 `TaskRegister::Close`，等待状态机线程退出并通过本适配器撤销租约，再由调用方关闭原始 `NamespacedEtcdClient`。客户端所有权明确留在调用方，本适配器不主动关闭它。

一次性路径由 `TaskRegisterImpl::RegisterTaskOnce` 驱动：先经 `get` 查 key；不存在时 grant+put，存在时读取首条 KV 的 lease 并调用 `keep_alive_once`。`metadata_register_test.rs` 覆盖了重复调用复用同一 lease 的行为。

## 数据与状态

本类型自身只有 `NamespacedEtcdClient` 一个字段，没有独立缓存或可变业务状态。底层客户端持有共享的 etcd session、RPC worker、endpoint、namespace 和逐副本 context；因此克隆适配器不会建立新连接，也不会丢失 keyspace 隔离。

数据转换规则如下：

- `put` 将 `&str` value 编码为 UTF-8 字节，并以 `Some(lease)` 绑定租约。
- `grant` 将底层返回的 lease ID 与请求 TTL 组合为 `LeaseGrantResponse`，并把 `error` 置为空；真实 RPC 错误通过 `Err` 返回。
- `get` 使用 `get_entries` 保留每条记录的 lease；key 和 value 用 `String::from_utf8_lossy` 转换。无效 UTF-8 不会报错，而会出现替换字符。底层 `get_entries` 已从返回 key 中剥离 keyspace namespace，因此上层仍看到 `/tidb/brie/import/...` 形式的逻辑 key。
- `time_to_live` 只保留 TTL 整数；`keep_alive` 回执包含同一 lease ID 和查询所得 TTL。
- `revoke` 不修改本地字段；租约 ID 的生命周期由 `TaskRegisterImpl::curLeaseID` 管理。

## 依赖与调用关系

上游调用关系：

- `lightning/pkg/importer/import.rs::registerTaskToPD` 是 RustCodeGraph 找到的直接生产调用者：构造适配器，调用 `NewTaskRegister` 和 `RegisterTask`，并在清理闭包中先关闭注册器、后关闭元数据客户端。
- `br/pkg/utils/metadata_register_test.rs::existing_register_state_machine_reuses_lease_and_cleans_real_keys` 是直接独立测试调用者，使用真实 etcd endpoint 验证一次性注册、租约复用与清理。

下游依赖关系：

- `crate::register::*` 提供 `EtcdRegisterClient`、所有响应结构以及特殊错误 `LeaseNotFound`；`TaskRegisterImpl` 是本适配器的实际消费者。
- `crate::stubs::context::Context` 提供克隆、取消检查与带超时等待，用于桥接同步注册状态机。
- `astersql_metaservice::NamespacedEtcdClient` 提供 `with_context`、`put`、`grant`、`keepalive`、`get_entries`、`revoke` 和 `time_to_live`。底层 RPC 在专用 `RpcWorker` 的 Tokio runtime 中执行，本文件保持同步 API。
- `astersql_errors::SharedError` 擦除具体错误类型，使状态机可以统一传播并对 `LeaseNotFound` 做 downcast。
- `std::thread` 与 `std::sync::mpsc` 把一次续租 RPC 适配为持续回执流。

## 错误处理与边界

除 `revoke` 外，底层 `MetaServiceError` 都直接经 `SharedError::new` 包装并返回。`revoke` 对错误文本包含 `requested lease not found` 的情况专门转换为类型化的 `LeaseNotFound`；`TaskRegisterImpl::Close` 会 downcast 此类型并将重复关闭视为成功。这个字符串匹配是兼容边界：若下游错误消息变化，幂等关闭语义可能失效。

`keep_alive` 在创建线程前先执行一次续租；首次失败会同步返回错误，不留下适配线程。线程启动后的 TTL 查询、发送或续租失败不再直接返回给最初调用者，而是通过线程退出关闭通道，让 `TaskRegisterImpl::keepalive_loop` 进入既有的重连/重新授租流程。

TTL 查询结果 `<= 0` 被视为租约失效。等待间隔最少一秒，避免小 TTL 造成忙循环；计算使用 `(ttl / 3).max(1) as u64`，且只在已确认 TTL 为正后转换。`get` 的 lossy UTF-8 转换保证读取不会因非法字节失败，但调用方若要求字节级 key/value 保真，需要扩展响应模型而不是假定字符串无损。

上下文取消通过每次派生的 `MetadataContext` 传递给底层 RPC，并由适配线程在等待阶段检查。由于 API 是同步的，取消结果体现为底层错误或线程/通道结束，而不是异步取消句柄。

## 并发与资源生命周期

每次成功调用 `keep_alive` 都会新建一个无显式 `JoinHandle` 的线程和一对 mpsc 端点。线程拥有派生客户端、BR context 与 sender；接收端由 `TaskRegisterImpl` 的 keepalive 线程持有。以下任一条件会释放适配线程：租约 TTL 无效或查询失败、receiver 被丢弃、等待期间上下文取消、后续 keepalive 失败。

正常关闭时，`TaskRegisterImpl::Close` 先取消 child context，再等待自己的 keepalive 消费线程结束；消费线程退出会丢弃 receiver，本适配线程随即在取消等待或下一次发送处退出。适配器本身不 join 线程，也不关闭共享 etcd session；`NamespacedEtcdClient` 的连接关闭必须由创建它的调用方负责，Lightning 的清理闭包遵守这一顺序。

`NamespacedEtcdClient` 可克隆且内部 session 由 `Arc` 共享，适配器符合 `EtcdRegisterClient: Send + Sync`。本文件没有额外锁；连接串行化、RPC worker 生命周期和底层 client 锁均由 `astersql-metaservice` 管理。扩展持续续租时必须避免创建无法通过 context 或 receiver 断开终止的孤儿线程。

## 与 Go 版本的对应关系

差异在于 Go etcd SDK 直接返回长生命周期 keepalive channel，Rust 元数据客户端的 `keepalive` 是单次请求，所以本文件用线程、TTL 查询和 mpsc 合成通道。

## 扩展指南

- 新增状态机所需的 etcd 操作时，先修改 `register.rs::EtcdRegisterClient`，再在 `MetadataRegisterClient` 和 `register_test.rs` 的模拟客户端中同步实现；真实 I/O 转换应继续留在本文件。
- 修改 key/value 表示时应重点检查 `get` 的 namespace 剥离和 lossy UTF-8 语义，并同步 `metadata_register_test.rs` 的真实 key 断言及 `register_test.rs` 的状态机断言。
- 修改续租算法时要区分两层职责：重授租/re-put/retry 属于 `TaskRegisterImpl::keepalive_loop`，单次 RPC 到回执通道的适配属于本文件。不要在两层重复重试，否则可能产生重复 lease、额外线程或关闭延迟。
- 若底层元数据客户端未来直接提供长生命周期 keepalive stream，应替换 `keep_alive` 的轮询线程，同时保留 channel 断开触发上层恢复、context 取消和 receiver 丢弃可终止资源的契约。
- 调整 `revoke` 错误识别时优先使用底层的类型化错误；若只能匹配文本，必须同步验证 `TaskRegisterImpl::Close` 的幂等行为。
- 生产入口若新增 BR/IMPORT INTO 调用者，应沿用“注册器先关闭、客户端后关闭”的所有权顺序，并增加独立测试文件；不要把测试嵌入本生产文件。

兼容风险主要是 Go/Rust lease 生命周期差异和错误分类；性能风险主要是每个持续注册占用一个适配线程，并按 TTL/3 发起 TTL 查询与 keepalive 两类 RPC。
