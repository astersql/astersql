# `br/pkg/utils/store_manager.rs`

## 文件定位

本文件属于 `astersql-br-pkg-utils` crate；crate 入口 `br/pkg/utils/lib.rs` 通过 `#[path = "store_manager.rs"] pub mod store_manager` 公开该模块。它对应 Go 文件 `br/pkg/utils/store_manager.go`，负责抽象并管理 BR 到 TiKV store 的 gRPC 连接，而不执行具体备份 RPC。

当前 Rust 迁移将网络边界拆成 `GrpcConn`、`GrpcConnFactory` 和 `PdClient` 三个 trait，避免本 crate 直接绑定具体 gRPC/PD 客户端。`br/pkg/utils/Cargo.toml` 的说明也明确该 crate 为适配 darwin arm64 而使用本地 stub 边界；因此本文件提供的是可注入的连接生命周期实现，不等同于 Go 版本已经完整接入生产 gRPC 栈。

RustCodeGraph 对当前索引的调用关系显示，具体 `StoreManager` 的构造和连接回调主要由 `br/pkg/utils/store_manager_test.rs` 直接覆盖；BR 的 Rust 连接层 `br/pkg/conn/conn.rs` 使用独立的 `StoreManagerHandle` 边界。扩展或接线时应先确认这两套抽象是否需要适配，不能仅凭 Go 调用链推断 Rust 已经走到本实现。

## 核心职责

- `Pool` 实现固定容量、惰性创建的通用连接池：池未满时调用工厂新建连接，池满后按游标轮询复用；`Close` 尽力关闭所有连接并清空池。
- `StoreManager` 按 `store_id` 缓存单条 `GrpcConn`。首次使用时通过 `PdClient::get_store` 取得元数据，再委托 `GrpcConnFactory::dial` 建链。
- `TryWithConn`/`WithConn` 在缓存连接上执行调用方回调；`RemoveConn` 关闭并删除单个缓存项；`ResetBackupClient` 删除旧连接后重拨；`Close` 关闭全部缓存连接。
- `KeepaliveParams`、`DialTimeout` 和 `TLS` 保存 Go API 所需配置语义，但本文件没有自行构造 gRPC dial options；`get_grpc_conn_locked` 只读取这些字段，真正应用配置的责任在注入的 `GrpcConnFactory`。
- 统一处理取消、PD 查询失败、拨号失败和关闭失败：取消/查询/拨号错误向上传播，关闭错误仅记录日志以继续清理。

## 主要符号

- `GrpcConn: Send + Sync`：连接最小接口。`target()` 仅用于诊断，`close()` 释放连接并可返回 `SharedError`。
- `GrpcConnFactory: Send + Sync`：`dial(&Context, &Store)` 是唯一实际建链入口。对 `StoreManager`，传入从 PD 得到的 `Store`；对 `Pool`，传入 `Store::default()`，目标选择必须由工厂自身配置。
- `PdClient: Send + Sync`：通过 `get_store(ctx, store_id)` 获取目标 store 元数据。
- `defaultDialTimeout`：未设置正值超时时使用 30 秒；`resetRetryTimes`：重拨最多三次。
- `Pool`：持有连接数组、轮询游标、容量和连接工厂。`mu` 将一次 `Get` 或 `take_conns` 整体串行化，其余 mutex 保护具体字段。
- `NewConnPool(capacity, new_conn)`：创建空池并预分配容量，返回 `Arc<Pool>`。
- `KeepaliveParams`：保存 ping 间隔、超时和空闲时是否发送；默认值为 10 秒、3 秒、`true`。
- `StoreManager`：核心状态为 `pdClient`、`grpcClis: Mutex<HashMap<u64, Arc<dyn GrpcConn>>>`、keepalive、可选 TLS、公开的 `DialTimeout` 和连接工厂。
- `StoreManager::NewStoreManager`：创建空缓存；Rust 比 Go 多接收一个显式 `conn_factory` 参数。
- `get_grpc_conn_locked`：内部拨号入口。查询 PD 后优先使用 `peer_address`，为空则回退 `address`，再委托工厂拨号；函数名中的 `locked` 表示调用方通常已持有连接表锁。
- `TryWithConn` 与 `WithConn`：前者接受可失败回调，后者把无返回值回调包装成 `Ok(())`。
- `ResetBackupClient`：移除旧连接后最多重拨三次，成功则缓存并返回原始 `Arc<dyn GrpcConn>`。
- `PDClient`、`GetKeepalive`、`TLSConfig`：返回管理器保存的依赖或配置副本。
- `inject_failpoint`：当前是空实现；它只保留 Go failpoint 的符号位置，不会产生通知文件或三秒注入等待。

## 执行流程

1. 调用方以 PD 客户端、keepalive、可选 TLS 和连接工厂调用 `NewStoreManager`；连接缓存初始为空，`DialTimeout` 为零，表示使用 30 秒默认值。
2. `TryWithConn` 先检查 `Context::is_cancelled`。若已取消，直接返回 `Canceled`，不会访问连接表。
3. 函数锁住 `grpcClis`。命中 `store_id` 时直接克隆缓存的 `Arc` 并在锁内执行回调；未命中时调用 `get_grpc_conn_locked`。
4. `get_grpc_conn_locked` 调用当前为空的 failpoint 钩子，经 PD 获取 `Store`，选择 peer address 或普通 address 写日志，并读取超时、keepalive 和 TLS 配置，最后由工厂拨号。
5. 拨号成功后连接写入 `grpcClis`，随后仍在同一临界区执行回调；拨号失败则包装为 `ErrFailedToConnect`，消息包含 `store_id`。
6. `WithConn` 复用上述流程，但把不返回错误的回调适配成可失败回调。
7. `ResetBackupClient` 先调用 `RemoveConn` 关闭并删除旧项，再锁住连接表并重拨最多三次。第 1、2、3 次失败后分别休眠 3、4、5 秒；成功立即缓存并返回，全部失败返回最后一个错误。
8. `StoreManager::Close` 遍历并关闭缓存连接，但不删除映射；`Pool::Close` 则先取走全部连接、清空池并重置轮询游标，再逐一关闭。

独立的 `Pool::Get` 流程是：持有总锁后检查连接数；小于容量则调用工厂并追加，达到容量则取 `conns[next]`，随后以容量为模推进游标。

## 数据与状态

`StoreManager` 的主要不变量是一个 `store_id` 至多对应一个缓存槽位。连接以 `Arc` 共享，所以回调取得的连接可以在调用结束后继续持有；但缓存替换或移除只影响映射和管理器持有的引用，不保证销毁其他持有者。

`grpcClis` 的 mutex 同时保护查找、拨号、插入和回调执行。这刻意保持 Go `grpcClis.mu` 的粗粒度语义，防止同一 store 并发首次访问时重复拨号，也意味着任何 store 的慢回调都会阻塞其他 store 的连接操作。

`Pool` 使用 `cap` 限制连接数量，`next` 只在池满后前进。`take_conns` 使用 `std::mem::take` 原子地清空向量并把 `next` 归零，因此 `Pool::Close` 后可以再次惰性建链。与之不同，`StoreManager::Close` 保留 `grpcClis` 内容；再次调用 `WithConn` 会复用已经执行过 `close()` 的对象。这一行为由 `close_retains_cached_connections_like_go` 明确锁定为 Go 对齐语义，通常只适合终止期调用。

配置状态中，`DialTimeout == Duration::ZERO` 表示 30 秒默认值；`tlsConf == None` 表示无 TLS 配置。当前实现没有把解析出的地址、有效超时、keepalive 或 TLS 作为独立参数传给工厂，工厂只能从 `Store` 和自身状态完成实际拨号。

## 依赖与调用关系

上游方面，`br/pkg/utils/lib.rs` 公开模块，`br/pkg/utils/store_manager_test.rs` 直接构造并验证 `StoreManager`。RustCodeGraph/源码搜索显示 Rust 生产文件没有直接调用 `StoreManager::NewStoreManager`、`TryWithConn` 或 `WithConn`；`br/pkg/conn/conn.rs` 的 `StoreManagerHandle` 及 `br/pkg/backup/client.rs` 的 `ClientMgr` 是更上层的客户端边界，重置客户端的调用链并未直接落到本文件的 concrete 类型。

Go 对照链则是 `br/pkg/conn/conn.go` 的 `Mgr` 嵌入 `*utils.StoreManager`，`GetBackupClient`/`GetLogBackupClient` 经 `WithConn` 构造具体 RPC client，备份失败路径可经 `ResetBackupClient` 重建。这是理解本文件目标角色的依据，但不是 Rust 已完成接线的证据。

下游依赖包括：

- `crate::kvproto::metapb::Store`：承载 store ID 与地址；来自本 crate 的 stub 边界。
- `crate::stubs::context::Context`：提供取消检查并传给 PD/连接工厂。
- `astersql_br_pkg_errors::{Canceled, ErrFailedToConnect}`：提供稳定的错误类别。
- `astersql_errors::{Trace, Annotate, SharedError}`：追踪 PD 错误并为拨号错误增加上下文。
- `astersql_br_pkg_logutil`：记录拨号、重试和关闭失败。
- `astersql_util::security::TLS`：保存可共享的 TLS 配置材料。
- 标准库 `Arc`、`Mutex`、`HashMap`、`thread::sleep`：实现共享所有权、串行化、缓存和同步重试。

## 错误处理与边界

- `RemoveConn` 和 `TryWithConn` 只在入口检查取消；取消返回 `Canceled`。`ResetBackupClient` 通过首次 `RemoveConn` 间接做入口检查，但重试休眠期间不会再次主动检查取消，是否能中止正在进行的拨号取决于工厂如何使用 `Context`。
- PD 查询错误经 `trace_err` 增加追踪信息。拨号错误以 `ErrFailedToConnect` 注解并附带 store ID；回调错误由 `TryWithConn` 原样返回。
- 所有 mutex 使用 `expect(...)`，锁中毒会 panic，而不是转换为业务错误。
- `Pool` 没有校验 `capacity > 0`。零容量下第一次 `Get` 会走满池分支并索引空向量，属于调用方必须避免的前置条件。
- `Pool::Get` 在持有总锁时调用工厂；慢拨号会阻塞同一池的所有获取与关闭。`StoreManager` 同样在全局连接表锁内执行 PD 查询、拨号、回调和重置休眠。
- 关闭失败只记录警告并继续；调用方无法从 `Close`/成功的 `RemoveConn` 感知关闭失败。
- `get_grpc_conn_locked` 计算地址仅用于日志，未拒绝两个地址都为空的 `Store`；最终能否拨号以及错误类型由工厂决定。
- `StoreManager::Close` 不清空缓存，重复关闭可能再次调用底层 `close()`，关闭后继续取连接也会命中旧对象。
- 当前 Rust `inject_failpoint` 是 no-op，不能复现 Go `hint-get-backup-client` 的文件通知和等待行为。

## 并发与资源生命周期

`Pool` 和 `StoreManager` 都声明为可跨线程共享的结构，其 trait 对象要求 `Send + Sync`。连接用 `Arc` 返回，资源真正释放取决于所有 `Arc` 持有者以及底层 `close()` 的语义。

`Pool::Get`/`take_conns` 由 `mu` 串行化；内部字段仍分别使用 mutex。`Pool::Close` 在释放锁后关闭已经取出的连接，因此耗时或失败的 `close()` 不阻塞后续 `Get` 建立新池内容。

`StoreManager` 的锁范围更大：`TryWithConn` 在回调返回前不释放 `grpcClis`，`ResetBackupClient` 在全部拨号和 3/4/5 秒休眠期间持锁。这样与 Go 行为一致并避免重复连接，但回调若递归调用同一管理器的连接方法会死锁；长 RPC 或慢工厂也会把所有 store 的连接管理串行化。`try_with_conn_serializes_callbacks_like_go` 通过两个线程验证第二个回调必须等待第一个回调释放。

推荐生命周期为：创建管理器，按需获取/复用连接，在明确故障时单独 `RemoveConn` 或 `ResetBackupClient`，最终调用一次 `Close` 后不再使用管理器。若需要可恢复的整体关闭语义，应先修改并测试缓存清理契约，不能假设现有 `Close` 可重启。

## 与 Go 版本的对应关系

保持一致的部分包括：30 秒默认拨号超时、三次 reset、3/4/5 秒同步等待、peer address 优先及 address 回退、按 store ID 缓存、持锁执行连接回调、关闭失败仅记录、`Pool` 的惰性扩容与满池轮询，以及 `StoreManager::Close` 关闭但不删除缓存项。

Rust 为可移植和可测试性做了显式抽象：Go 直接持有 `pd.Client`、`*grpc.ClientConn`、`*tls.Config` 并调用 `grpc.DialContext`；Rust 持有 trait object 与 `TLS`，由 `GrpcConnFactory` 实现拨号。Rust 构造函数因此多一个工厂参数。

尚未等价或刻意简化的部分如下：

- Go 在本文件内创建带 deadline 的 context、设置阻塞拨号、最大三秒 gRPC backoff、TLS/非 TLS credentials 和 keepalive；Rust 只读取相关配置，没有在本文件中执行这些选项。
- Go `ResetBackupClient` 返回 `backuppb.BackupClient`；Rust 返回底层 `Arc<dyn GrpcConn>`，具体 RPC client 的构造不在这里。
- Go 提供 `ResetPDClientCallerComponent`；Rust 没有对应方法。
- Go failpoint 会按参数创建通知文件并等待三秒；Rust 钩子为空。
- Go 的指针接收者允许 `Close`/`TLSConfig` 对 nil manager 返回；Rust 的引用方法不存在 nil receiver。
- Rust 的 concrete manager 当前没有证据表明已适配到 `br/pkg/conn/conn.rs` 的生产 `StoreManagerHandle`，应将其视为迁移中边界，而不是已完成替代。

Go 同目录未搜索到针对这些符号的 `*_test.go` 用例；Rust 的两个独立测试专门固定了锁范围和关闭后保留缓存这两项 Go 语义。

## 扩展指南

- 接入真实 gRPC 时，优先实现或适配 `GrpcConnFactory`，并明确验证地址、有效 `DialTimeout`、keepalive、TLS、阻塞拨号与 backoff 都被实际应用。若工厂接口不足，应以最小改动扩充参数，同时在独立测试文件中验证透传。
- 将 concrete `StoreManager` 接入 `br/pkg/conn/conn.rs` 时，需要实现明确的适配层，把原始连接转换为 `BackupClient`/`LogBackupClient`，不要平行复制另一套缓存和重置逻辑。
- 修改锁粒度前必须保留“同一 store 不重复拨号”的不变量，并评估 Go 对齐要求。若把回调移出锁外，需要设计连接代际或每-store 锁，且更新 `try_with_conn_serializes_callbacks_like_go`。
- 修改 `Close` 时先决定它是终止操作还是可恢复清空操作。清空映射会改变现有 Go 对齐行为，必须同步调整 `close_retains_cached_connections_like_go` 并检查上层关闭顺序。
- `NewConnPool` 若面向不可信容量，应增加零容量校验并在新的独立测试中覆盖；不要在源文件内嵌测试。
- 为 reset 增加可取消等待时，应替换裸 `thread::sleep` 并在每轮检查 context，同时验证仍保持最多三次及退避时序。
- 补齐 Go 的 failpoint 或 caller-component 行为时，应确认相关依赖不会破坏 `br/pkg/utils/Cargo.toml` 声明的精简平台边界。
- 相关测试应继续放在 `br/pkg/utils/store_manager_test.rs`，符合仓库“Rust 源文件与单元测试分离”的约束。

## 验证依据

- Rust 实现：`br/pkg/utils/store_manager.rs`，已核对全部 347 行；关键证据为 `Pool::{Get, Close, take_conns}`、`NewConnPool`、三个边界 trait、`StoreManager::{NewStoreManager, get_grpc_conn_locked, RemoveConn, TryWithConn, WithConn, ResetBackupClient, Close}`。
- crate 边界：`br/pkg/utils/Cargo.toml` 与 `br/pkg/utils/lib.rs`；前者声明 `astersql-br-pkg-utils` 及精简依赖策略，后者公开 `store_manager` 模块并把测试作为独立 `cfg(test)` 模块挂载。
- Go 对照：`br/pkg/utils/store_manager.go`；核对了 Pool、拨号选项、缓存、重置、关闭、TLS 和 PD caller component 行为。上层角色参考 `br/pkg/conn/conn.go` 的嵌入与客户端构造调用链。
- Rust 独立测试：`br/pkg/utils/store_manager_test.rs`；`try_with_conn_serializes_callbacks_like_go` 验证回调在全局锁内串行，`close_retains_cached_connections_like_go` 验证关闭不清缓存且不会再次拨号。
- Rust 上层边界：`br/pkg/conn/conn.rs` 的 `StoreManagerHandle`、`br/pkg/backup/client.rs` 的 `ClientMgr`，用于确认当前 concrete 实现尚未直接形成完整生产调用链。
- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/utils/store_manager.rs` 确认目标文件含 42 个符号；`node --file ... --offset 1 --limit 500` 核对完整源码；`explore`/调用搜索确认 concrete manager 的 Rust 直接使用点集中于独立测试，并辨别 Go 与 Rust 调用链。
- 未运行 Cargo：本任务只新增说明文档，且任务计划明确禁止运行 Cargo。交付验证仅执行任务指定的十一章节结构检查，并人工复核上述事实来源。
