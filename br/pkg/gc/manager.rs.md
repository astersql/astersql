# `br/pkg/gc/manager.rs`

## 文件定位

本文件是 `astersql-br-pkg-gc` library crate 的 GC 管理契约与实现选择入口。crate 根文件 [`br/pkg/gc/lib.rs`](./lib.rs) 通过 `#[path = "manager.rs"]` 挂载本模块，并重导出这里的 `KeyspaceID`、`NullspaceID`、GC 状态类型、三个 trait 以及 `NewManager`。crate 的 [`Cargo.toml`](./Cargo.toml) 将 Go 包来源标为 `br/pkg/gc`，当前生产依赖只有 `astersql-br-pkg-errors`；PD 能力没有在此绑定到某个网络客户端，而是被压缩为 trait，便于上层接入真实客户端或测试替身。

文件本身不执行 RPC，也不维护后台任务。它把公共操作面交给 `Manager`，把 PD 的两组最小能力分别抽象为 `PdClient` 与 `GCStatesClient`，最后由 `NewManager` 在 [`manager_global.rs`](./manager_global.rs) 和 [`manager_keyspace.rs`](./manager_keyspace.rs) 之间选择。仓库搜索时未发现非测试 Rust 代码直接调用本文件的 `NewManager`；当前生产接线更多是通过 `Arc<dyn astersql_br_pkg_gc::Manager>` 消费已经构造好的管理器，例如 `br/pkg/task/backup.rs`，因此不能仅凭本工厂推断所有 BR 入口已经在 Rust 侧完成实例化。

## 核心职责

1. 用 `KeyspaceID = u32` 和 `NullspaceID = 0xffff_ffff` 建立与 TiKV keyspace 标识相同的分派边界。`NullspaceID` 是“全局/空 keyspace”哨兵，并不是普通业务 keyspace 0。
2. 定义 BR 实际使用的 PD GC 状态子集：`GCState` 包含 GC safepoint、事务 safepoint和 barrier 列表；`GCBarrierInfo` 描述单个 barrier 的 ID、时间戳与秒级 TTL。
3. 以 `GCStatesClient` 表达 keyspace 作用域的状态读取、barrier 设置和删除；以 `PdClient` 表达 global 路径需要的旧 safepoint API，以及取得 keyspace client 的能力。
4. 以 `Manager` 向 BR 上层屏蔽 global 与 keyspace 两套 PD API 的差异，统一提供查询、设置和删除服务 safepoint 的操作。
5. `NewManager` 只依据 keyspace ID 选择实现：等于 `NullspaceID` 时构造 global manager，否则构造绑定指定 keyspace 的 keyspace manager。

## 主要符号

- `pub type KeyspaceID = u32`：PD/TiKV keyspace 的本地别名。工厂参数与 `keyspaceManager.keyspace_id` 都使用该类型。
- `pub const NullspaceID: KeyspaceID`：值为 `u32::MAX` 的 global 模式哨兵。它是 `NewManager` 唯一的分支条件。
- `GCState`：可克隆、可调试且有默认值的状态快照；字段 `GCSafePoint`、`TxnSafePoint` 和 `GCBarriers` 与 Go/PD 命名保持一致。默认值仅是 Rust 数据构造能力，不代表 PD 的真实初始状态。
- `GCBarrierInfo`：barrier 回执/快照。`TTL` 为秒，`i64::MAX` 表示永不过期；本文件不校验 TTL。
- `GCStatesClient: Send + Sync`：keyspace client 契约。`GetGCState` 返回完整快照；`SetGCBarrier` 返回 PD 接受后的 barrier 信息；`DeleteGCBarrier` 用 `Option` 表示被删除项可能不存在。
- `PdClient: Send + Sync`：PD client 的最小适配面。`UpdateGCSafePoint` 与已弃用的 `UpdateServiceGCSafePoint` 服务 global 实现，`GetGCStatesClient` 为指定 ID 创建或取得 keyspace client。
- `Manager: Send + Sync`：上层稳定接口。`GetGCSafePoint` 查询可见 safepoint，`SetServiceSafePoint` 接收 `BRServiceSafePoint`，`DeleteServiceSafePoint` 显式移除保护。注释规定 `TTL <= 0` 的设置等价于删除，具体分支由实现承担。
- `NewManager(Arc<dyn PdClient>, KeyspaceID) -> Arc<dyn Manager>`：公开工厂，返回线程安全、共享所有权的 trait object，不泄露具体实现类型。

## 执行流程

构造流程只有一次同步分派：调用方准备共享 `PdClient` 和 keyspace ID；`NewManager` 比较 ID；global 分支调用 `newGlobalManager(pd_client)`，keyspace 分支调用 `newKeyspaceManager(pd_client, keyspace_id)`；两者被装入 `Arc<dyn Manager>` 返回。工厂不访问 PD，因此 global 构造没有 I/O；keyspace 构造的下游函数会立即调用 `PdClient::GetGCStatesClient(keyspace_id)` 绑定作用域，但该 trait 方法的具体 I/O/缓存语义由适配器决定。

运行期调用经过动态分派：global 的 `GetGCSafePoint` 用 `UpdateGCSafePoint(ctx, 0)` 查询，设置/删除通过旧的 `UpdateServiceGCSafePoint`；keyspace 的查询读取 `GetGCState().GCSafePoint`，正 TTL 设置调用 `SetGCBarrier`，非正 TTL 转入删除，显式删除调用 `DeleteGCBarrier`。两个实现都把保护时间戳计算为 `BackupTS.wrapping_sub(1)`。这些行为位于相邻实现文件，而本文件负责保证上层看到同一 `Manager` 契约。

在 BR safepoint 主链中，[`safepoint.rs`](./safepoint.rs) 的 `CheckGCSafePoint` 调用 `Manager::GetGCSafePoint` 校验备份时间戳，`StartServiceSafePointKeeper` 初次调用并周期刷新 `Manager::SetServiceSafePoint`；备份任务清理路径 `br/pkg/task/backup.rs` 调用 `DeleteServiceSafePoint`。因此该 trait 是 safepoint 注册、续租、校验和清理之间的边界，而不是 keeper 本身。

## 数据与状态

本文件拥有的静态状态只有 `NullspaceID`；没有全局可变变量。`GCState` 与 `GCBarrierInfo` 都是按值传递的快照/回执，不在本模块缓存。`BRServiceSafePoint`、`Context` 与 `SharedError` 定义在 [`safepoint.rs`](./safepoint.rs)，本模块只是引用它们以形成跨实现一致的签名。

所有客户端和管理器通过 `Arc` 共享。`NewManager` 会把传入的 `Arc<dyn PdClient>` 移交给具体实现；global manager 保存该引用，keyspace manager同时保存 PD 引用、keyspace ID 和构造时取得的 `Arc<dyn GCStatesClient>`。`Send + Sync` 是 trait 的硬约束，保证这些对象可供 keeper 线程或不同任务共享，但不意味着一次调用自动重试、串行化或具备事务性；这些语义必须由具体适配器或调用方提供。

## 依赖与调用关系

上游导出路径是 `lib.rs -> manager`，crate 用户通常经 `astersql_br_pkg_gc::{Manager, ...}` 使用契约。直接行为测试 [`manager_test.rs`](./manager_test.rs) 调用 `NewManager` 并借助 [`mock_test.rs`](./mock_test.rs) 的内存 PD 检查作用域隔离。RustCodeGraph 对本文件列出的直接使用者还包括 `br/pkg/task/restore_lifecycle.rs`、若干任务测试及 RealTiKV GC keyspace 测试；其中不少使用是类型契约或测试接线，不应等同于生产侧直接工厂调用。

下游静态依赖为 `manager_global::newGlobalManager`、`manager_keyspace::newKeyspaceManager` 和 `safepoint::{BRServiceSafePoint, Context, SharedError}`。动态依赖由 trait 实现决定：global 路径使用 `PdClient` 的两种 update 方法，keyspace 路径通过 `GetGCStatesClient` 使用三种 GC states 方法。由于 `Cargo.toml` 没有直接依赖 PD/TiKV SDK，真实 SDK 类型必须在 crate 边界外经适配器实现这些 trait。

## 错误处理与边界

所有可失败接口统一返回 `SharedError`，本文件不捕获、包装或重试错误。相邻的 global/keyspace 实现使用 `Trace` 传播 PD 失败；因此新增适配器必须保留原始错误因果，不能用默认状态掩盖失败。

关键边界如下：`NewManager` 对任何非 `NullspaceID` 的 `u32` 都选择 keyspace 路径，不验证 ID 是否存在；`GCState::default()` 不能作为“PD 返回 0”的证明；`GCBarrierInfo::TTL` 允许负值，但实际设置的非正 TTL 删除语义属于 `Manager` 实现；`BackupTS - 1` 的下溢处理不在本文件中，相邻实现当前使用 `wrapping_sub(1)`，所以 `BackupTS == 0` 会得到 `u64::MAX`，扩展时不能擅自改成饱和减法；`DeleteGCBarrier` 返回 `Ok(None)` 时契约仍表示成功调用，是否要求目标原先存在应由具体实现和上层业务决定。

## 并发与资源生命周期

三个 trait 都要求 `Send + Sync`，返回对象也由 `Arc` 管理，因此管理器可跨线程共享并由最后一个持有者释放。本文件没有锁、channel、异步 runtime、线程或 `Drop` 清理逻辑；一次 trait 调用的互斥与取消行为取决于 `Context` 和下游客户端实现。

服务 safepoint 的业务生命周期由上层 keeper 驱动：注册/刷新使用同一个服务 ID 与 TTL，退出或失败清理由显式 `DeleteServiceSafePoint` 完成。仅仅丢弃 `Arc<dyn Manager>` 不会远程删除 barrier。keyspace client 在 manager 构造时绑定并随 manager 共同存活，不能在同一个 manager 实例上切换 keyspace；需要切换时应重新调用工厂创建新实例。

## 与 Go 版本的对应关系

直接对照文件 [`manager.go`](./manager.go) 定义同名 `Manager` 接口和 `NewManager`。两端均以 Nullspace 哨兵选择 global，否则选择 keyspace；三个 Manager 方法的业务含义和 `TTL <= 0` 删除约定一致。Rust 用 `&Context` 代替 Go `context.Context`，用 `Result<_, SharedError>` 代替 `(value, error)`，用 `Arc<dyn Manager>` 代替 Go interface 的垃圾回收共享语义。

Rust 额外在本文件显式定义 `GCState`、`GCBarrierInfo`、`GCStatesClient` 和 `PdClient` 子集；Go 直接使用 `pd.Client`、`gc.GCStatesClient` 与 TiKV 类型。这是为隔离尚未直接依赖的外部 SDK 所做的端口边界，不是新增业务行为。Rust `NullspaceID` 写死为 `0xffff_ffff`，对应 Go 的 `tikv.NullspaceID`；升级 SDK 或协议时应验证该常量没有漂移。

[`manager_test.rs`](./manager_test.rs) 对齐 [`manager_test.go`](./manager_test.go) 的 `TestNewManager`、global manager 与 keyspace manager场景：验证写入作用域互不泄漏、设置后删除、keyspace 的 TTL=0 删除，以及两种模式初始查询返回 mock 的 0。Rust 用线程局部配置和 Drop 守卫恢复测试状态，而 Go 用 `t.Cleanup` 恢复全局配置；这属于测试隔离机制差异。

## 扩展指南

新增上层 GC 操作时，先判断它是否必须同时适用于 global 和 keyspace。若是，应在 `Manager` 增加最小方法，并在 `manager_global.rs`、`manager_keyspace.rs` 同步实现，在独立的 `manager_test.rs` 增加两条作用域测试，同时核对 Go `manager.go` 及其两个实现；不要把测试内嵌进生产源文件。若只是 PD 适配需要的新原语，应优先扩展 `PdClient` 或 `GCStatesClient`，避免把 SDK 类型泄露到业务调用方。

新增第三种作用域或改变分派规则时，修改点是 `KeyspaceID`/哨兵定义与 `NewManager`，并必须加入工厂路由的正反测试。修改 TTL、`BackupTS - 1` 或“不存在时删除”的语义会影响备份可恢复性与 Go 兼容性，应同步检查 `safepoint.rs` 的 keeper/校验流程、两份具体实现、`manager_test.rs`、`safepoint_test.rs` 和同路径 Go 测试。为真实 PD SDK 增加适配时，要保持 `Send + Sync`、错误因果、context 取消以及 keyspace client 的绑定生命周期，并评估 RPC 重试与并发调用是否会放大 PD 压力。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/gc` 确认 manager、global/keyspace 实现、safepoint 和独立测试均已索引；`node --file br/pkg/gc/manager.rs` 核对 117 行源码与 16 个符号；`explore`、`query NewManager` 以及 callers/callees 查询用于检查工厂和调用关系。精确 callers/callees 命令对含冒号的节点 ID 出现名称解析泛化，因此上游直接调用又以 `rg` 交叉核验。
- 已读生产与配置路径：`br/pkg/gc/manager.rs`、`br/pkg/gc/lib.rs`、`br/pkg/gc/manager_global.rs`、`br/pkg/gc/manager_keyspace.rs`、`br/pkg/gc/safepoint.rs`、`br/pkg/gc/Cargo.toml`、`br/pkg/task/backup.rs`。目标目录没有 `doc.go`。
- 已读对照与测试：`br/pkg/gc/manager.go`、`br/pkg/gc/manager_test.go`、`br/pkg/gc/manager_test.rs`；测试证据覆盖工厂分派、作用域隔离、设置/删除、TTL=0 和查询路径。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并人工复核相对链接、符号名、无整段源码复制及未接线事实说明。
