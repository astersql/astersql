# `pkg/ddl/owner_mgr.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；crate 根在 `pkg/ddl/lib.rs` 中以 `pub mod owner_mgr` 公开该模块，独立测试则以 `#[cfg(test)] mod owner_mgr_test` 接入。它提供一个按 keyspace 划分的、进程内全局 DDL owner 状态表，以及启动、关闭和查询该状态的 API（`OWNER_MANAGERS`、`start_owner_manager`、`close_owner_manager`、`owner_manager`）。

当前 Rust 实现只维护内存状态，并没有持有 etcd client、租约或 `astersql-owner` 的选举管理器。仓库内对这些公开函数的 Rust 引用仅见 `pkg/ddl/owner_mgr_test.rs`；因此它目前是已公开但尚未接入 Rust DDL 生产启动主链的移植片段，不能等同于 Go 版本实际参与 DDL owner 选举的实现。`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/ddl"` 共同确认了 crate 边界与 Go 对照目录。

## 核心职责

- `managers` 惰性建立进程级映射，并预置空字符串 keyspace 的默认项，以对应 classic kernel 的默认 keyspace 约定。
- `start_owner_manager` 在互斥锁保护下获取或创建指定 keyspace 的 `OwnerManager`，执行存储类型和 etcd 可用性检查，并返回该管理器的 ID。
- `OwnerManager::start` 保证重复启动幂等：已经启动时直接成功；非 TiKV 存储也直接成功且不产生 owner 身份；TiKV 且 etcd 不可用时返回错误；其余情况下生成 ID 并把本地状态标为 owner。
- `close_owner_manager` 关闭指定 keyspace 的管理器，但保留映射项和已生成的 ID；`owner_manager` 返回克隆快照，避免把全局锁或可变引用暴露给调用者。

这里的“owner”只是 `started && is_owner` 的本地布尔判定（`OwnerManager::is_owner`），没有实现分布式竞选、失主检测或故障转移。

## 主要符号

- `OWNER_MANAGERS: OnceLock<Mutex<BTreeMap<String, OwnerManager>>>`：进程级单例。`OnceLock` 负责一次初始化，`Mutex` 串行化映射和管理器状态变更，`BTreeMap` 以 keyspace 名称为键。
- `NEXT_ID: AtomicU64`：从 1 开始的进程内单调计数器；只用于拼接 `ddl-owner-{n}` 格式的本地 ID。
- `managers() -> &'static Mutex<BTreeMap<String, OwnerManager>>`：内部惰性初始化入口，初始映射包含 `"" -> OwnerManager::default()`。
- `OwnerManager { id, started, is_owner }`：公开类型、私有字段；实现了 `Clone`、`Debug`、`Default`、`Eq` 和 `PartialEq`，公开方法为 `start`、`close`、`id`、`is_owner`。
- `OwnerManager::start(&mut self, tikv_store, etcd_available) -> Result<(), String>`：核心状态转换函数。检查顺序很重要：`started` 或非 TiKV 会先返回，因此已经启动的实例即使随后传入 `etcd_available = false` 也保持成功。
- `start_owner_manager(keyspace, tikv_store, etcd_available) -> Result<String, String>`：公开的按 keyspace 启动门面；成功时克隆并返回当前 ID。
- `close_owner_manager(keyspace)`：公开关闭门面；keyspace 不存在时静默无操作。
- `owner_manager(keyspace) -> Option<OwnerManager>`：公开只读快照门面；不存在时返回 `None`。

## 执行流程

启动流程从 `start_owner_manager` 开始：

1. 调用 `managers`，首次调用时创建全局表并预置默认 keyspace。
2. 获取全局互斥锁；通过 `BTreeMap::entry(...).or_default()` 为命名 keyspace 创建默认管理器。
3. 调用 `OwnerManager::start`。若管理器已启动，立即成功；若不是 TiKV，立即成功且保持默认空 ID、未启动、非 owner 状态。
4. TiKV 路径检查 `etcd_available`；为假时返回固定错误文本，且不改变状态。
5. 检查通过后用 `NEXT_ID.fetch_add(1, Ordering::Relaxed)` 分配序号，设置 ID、`started = true`、`is_owner = true`。
6. 门面克隆 ID 后返回，并在函数退出时释放互斥锁。

关闭流程由 `close_owner_manager` 获取同一把锁，若找到条目则调用 `OwnerManager::close`。关闭只清除 `started` 和 `is_owner`，不删除条目、不清空 ID。查询流程同样短暂持锁并克隆整个 `OwnerManager`，调用者看到的是某一时刻的快照。

## 数据与状态

单个管理器的有效状态可概括为：默认态 `id = "" / started = false / is_owner = false`；TiKV 成功启动后进入 `id != "" / true / true`；关闭后保留 ID 并进入 `id != "" / false / false`。非 TiKV 启动保持默认态。失败的 TiKV 启动也保持调用前状态，因为错误发生在所有字段赋值之前。

`is_owner` 字段虽然可独立存储，但对外查询还要求 `started` 为真，形成 `OwnerManager::is_owner == started && is_owner` 的不变量。当前文件没有提供只改变 `is_owner` 的竞选回调，因此成功启动会立刻成为本地 owner，关闭则立刻失去身份。

关闭后再次启动会分配新 ID，因为 `started` 已恢复为假；旧 ID 在成功赋新值前仍保留。`NEXT_ID` 不按 keyspace 分区，也不会因关闭而回收；它只保证当前进程内各次成功启动获得不同序号，不提供跨进程、跨重启稳定性。

## 依赖与调用关系

文件仅依赖 Rust 标准库：`BTreeMap`、`Mutex`、`OnceLock`、`AtomicU64` 和 `Ordering`。因此 `pkg/ddl/Cargo.toml` 没有为此实现引入 etcd、UUID 或 owner-election 依赖；清单中的其他 DDL 依赖不能作为本文件已经使用它们的证据。

RustCodeGraph 将 `OwnerManager`、三个公开自由函数及内部方法识别为 `pkg/ddl/owner_mgr.rs` 的符号。精确仓库搜索显示，生产 Rust 文件没有调用 `start_owner_manager`、`close_owner_manager` 或 `owner_manager`；直接调用方是 `pkg/ddl/owner_mgr_test.rs::test_owner_manager`。`pkg/ddl/lib.rs` 负责模块公开和测试接线。

Go 生产链不同：`pkg/ddl/owner_mgr.go::StartOwnerManager` 创建/启动真实管理器，`pkg/ddl/ddl.go::newDDL` 通过 `getOwnerManager` 取得 ID 和 `owner.Manager`，再注入 DDL 上下文。该调用边只证明 Go 版本的位置，不证明当前 Rust 模块已经被 Rust DDL 主链消费。

## 错误处理与边界

唯一显式业务错误来自 `OwnerManager::start` 的 TiKV/无 etcd 分支，类型是 `String`，文本固定为 `etcd client is nil, maybe the server is not started with PD`。该实现不保留底层错误源，也没有结构化错误分类。

`managers().lock().unwrap()` 出现在启动、关闭和查询门面中：若任一持锁线程 panic 导致 mutex poisoned，后续调用会再次 panic，而不是返回可恢复错误。不存在的 keyspace 在关闭时被当作成功的空操作，在查询时表现为 `None`；启动则一定通过 `or_default` 建立条目。

计数器使用 `Relaxed` 排序，只承担唯一序号分配，不承担管理器字段之间的同步；字段可见性由 mutex 提供。计数器达到 `u64` 上限时会按原子整数语义回绕，当前没有溢出检查。接口中的 `tikv_store` 与 `etcd_available` 是调用者提供的布尔事实，文件自身不会读取真实配置、探测 PD 或创建客户端。

## 并发与资源生命周期

全局表初始化由 `OnceLock` 保证线程安全且只执行一次。所有 keyspace 共享一把 `Mutex`，所以不同 keyspace 的启动、关闭和查询也会串行；当前临界区很短，但若未来把网络连接或选举启动放进 `OwnerManager::start`，继续在全局锁内执行会扩大阻塞范围。

ID 序号通过 `AtomicU64` 分配，即使未来分配动作移出互斥区仍不会发生数据竞争；当前 mutex 已经串行化调用，原子主要表达全局计数器自身的线程安全。`owner_manager` 返回克隆值，使读取者不会持有锁，也无法通过快照修改全局状态；快照在返回后可能立即过期。

本实现没有后台任务、channel、取消令牌、etcd session 或需要显式释放的外部资源。`close` 只是内存状态转换，且不清空 ID、不移除 keyspace。进程结束时静态数据整体销毁；没有公开的全局重置入口，测试通过使用不同 keyspace 和显式关闭来隔离可观察状态。

## 与 Go 版本的对应关系

结构映射如下：Rust 的 `OWNER_MANAGERS` 对应 Go 的 `globalOwnerManagers`；`start_owner_manager`/`close_owner_manager` 对应 `StartOwnerManager`/`CloseOwnerManager`；Rust `OwnerManager` 对应 Go 私有 `ownerManager`；空字符串默认 keyspace 和按 `store.GetKeyspace()` 分桶的设计意图一致。

已对齐的局部语义包括：非 TiKV 路径启动为空操作；重复启动幂等；TiKV 路径缺少 etcd 时使用相同错误文本；关闭后管理器条目保留；classic 与 nextgen 可使用空或命名 keyspace。Rust 测试 `pkg/ddl/owner_mgr_test.rs::test_owner_manager` 覆盖这两种 keyspace、UniStore、缺 etcd、TiKV 成功、重复启动和关闭后的状态；Go 测试 `pkg/ddl/owner_mgr_test.go::TestOwnerManager` 验证 UniStore 不创建资源、TiKV 创建资源。

尚未对齐的关键行为是：Go `ownerManager` 持有 `*clientv3.Client` 和 `owner.Manager`，使用 UUID，调用 `owner.NewOwnerManager`，并在 `Close` 中关闭竞选管理器与 etcd client；Rust 只有字符串和布尔状态，成功启动便直接把 `is_owner` 置真。Go 的 `ddl.go::newDDL` 会取得真实 owner manager 并注入调度链，Rust 仓库没有对应调用。因此当前 Rust 文件不具备真实分布式互斥、租约续期、owner 转移、故障恢复或资源关闭能力。

## 扩展指南

若要把该模块接入真实 DDL owner 生命周期，应从 `OwnerManager` 的字段和 `OwnerManager::start/close` 扩展资源所有权，并在 Rust DDL 构造/启动入口建立调用边，而不是只增强查询门面。需要同步考虑：按 keyspace 选择选举键、真实 store/config 判断、etcd client 创建失败的错误链、竞选异步状态回调、重复启动、关闭幂等、先释放 manager 还是 client，以及 DDL scheduler 只能由真实 owner 启动的不变量。

任何网络或竞选初始化都不应长时间占用当前全局 mutex；可考虑把“定位 keyspace 条目”与“启动外部资源”分阶段设计，同时防止并发双启动。若 API 改为返回共享句柄而非克隆快照，应明确锁粒度、句柄生命周期和关闭时的竞态。ID 若需与 Go 兼容或跨进程唯一，应替换当前进程计数器，而不是依赖 `ddl-owner-{n}`。

测试必须继续放在独立的 `pkg/ddl/owner_mgr_test.rs`，不要内嵌到生产源文件。至少同步覆盖：并发启动同一/不同 keyspace、失败后重试、关闭后重启、资源关闭错误、竞选成功与失主、panic/poison 策略，以及生产构造入口确实消费该 manager。涉及 Go 语义对齐时还应复核 `pkg/ddl/owner_mgr.go`、`pkg/ddl/owner_mgr_test.go` 和 `pkg/ddl/ddl.go::newDDL`，避免把当前简化状态模型误当作完整移植。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库，目标文件可由 `node --file pkg/ddl/owner_mgr.rs --offset 1 --limit 400` 完整读取；索引识别 `OwnerManager`、`start_owner_manager`、`close_owner_manager` 和 `owner_manager`。
- RustCodeGraph `query OwnerManager --limit 20` 及对三个自由函数的 `query --kind function --json`：区分了 `pkg/ddl/owner_mgr.rs`、`pkg/ddl/owner_mgr.go` 和 `pkg/owner/manager.rs` 等同名符号。`callers/callees` 对这些短小自由函数未返回可用边，因而用精确仓库引用搜索补证。
- 生产源码：`pkg/ddl/owner_mgr.rs`（完整 92 行），用于核对所有静态量、结构体、方法、状态转换、错误文本和同步原语。
- crate 与模块入口：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`，用于核对 `astersql-ddl` 归属、Go 移植元数据、公开模块和独立测试模块。
- Rust 测试：`pkg/ddl/owner_mgr_test.rs::test_owner_manager`，用于核对空/命名 keyspace、UniStore、TiKV、缺 etcd、重复启动和关闭保留条目的行为。
- Go 对照：`pkg/ddl/owner_mgr.go`、`pkg/ddl/owner_mgr_test.go::TestOwnerManager`、`pkg/ddl/ddl.go::newDDL`，用于核对真实 etcd/owner 资源生命周期及生产接线，并识别 Rust 当前未实现的部分。
- 精确 `rg` 调用点检查：除目标文件外，三个 Rust API 的直接使用只出现在 `pkg/ddl/owner_mgr_test.rs`；`pkg/ddl/lib.rs` 仅公开模块。这是“尚未接入 Rust 生产主链”结论的直接依据。
