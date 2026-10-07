# [`br/pkg/gc/lib.rs`](lib.rs)

## 文件定位

`br/pkg/gc/lib.rs` 是 Cargo 包 `astersql-br-pkg-gc` 的 crate 根。`br/pkg/gc/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它指定为库入口，并用 `package.metadata.porting.go-package = "br/pkg/gc"` 记录对应的 Go 包。该文件本身是门面：负责声明生产模块、组织公开 API 和挂载独立测试，不直接访问 PD、维护 GC 状态或启动后台任务。

文件用 `#[path = "..."]` 显式接入四个生产模块：`manager.rs`、`manager_global.rs`、`manager_keyspace.rs`、`safepoint.rs`。四个模块均为 `pub mod`，因此调用方既可从 crate 根使用重导出，也可在需要非门面符号时沿模块路径访问。crate 根还允许 `non_snake_case`、`non_camel_case_types`、`non_upper_case_globals`，以保留 Go 移植 API 的命名，而不是要求各文件逐项抑制 lint。

## 核心职责

1. 建立 GC 子系统的模块边界：统一纳入 Manager 契约、global 实现、keyspace 实现和 safepoint 生命周期辅助函数（`lib.rs:10-20`）。
2. 提供稳定的 crate 根公开面：从 `manager` 重导出 `Manager`、`NewManager`、`PdClient`、`GCStatesClient`、`GCState`、`GCBarrierInfo`、`KeyspaceID`、`NullspaceID`；从 `safepoint` 重导出服务安全点数据、默认 TTL、校验/启动函数和轻量 `Context`（`lib.rs:23-31`）。
3. 保持生产代码与测试分离：只在 `cfg(test)` 下通过路径挂载 `parity_test.rs`、`mock_test.rs`、`manager_test.rs`、`safepoint_test.rs`（`lib.rs:33-47`），测试逻辑没有内嵌进生产源文件。

这个门面让上游只依赖 `astersql_br_pkg_gc`，无需知道 global 与 keyspace 的具体类型。实现选择发生在 `manager::NewManager`：`NullspaceID` 返回 global Manager，其他 `KeyspaceID` 返回绑定对应 keyspace 的 Manager（`manager.rs:108-117`）。

## 主要符号

- 模块 `manager`：公开统一 `Manager: Send + Sync` trait，以及其 PD/GCStates 适配接口。`Manager` 的三个操作是 `GetGCSafePoint`、`SetServiceSafePoint`、`DeleteServiceSafePoint`；`NewManager(Arc<dyn PdClient>, KeyspaceID)` 是实现选择入口（`manager.rs:47-117`）。
- 模块 `manager_global`：公开模块但未从根重导出具体 `globalManager`；其 `Manager` 实现用 `UpdateGCSafePoint(0)` 查询全局安全点，并用兼容性的 `UpdateServiceGCSafePoint` 设置或删除服务安全点（`manager_global.rs:41-125`）。
- 模块 `manager_keyspace`：公开模块但未从根重导出具体 `keyspaceManager`；构造时调用 `GetGCStatesClient(keyspace_id)` 绑定作用域，此后经 `GetGCState`、`SetGCBarrier`、`DeleteGCBarrier` 操作屏障（`manager_keyspace.rs:29-139`）。
- `BRServiceSafePoint { ID, TTL, BackupTS }`：服务安全点值对象；TTL 单位为秒，Manager 写入的保护时间戳为 `BackupTS - 1`（`safepoint.rs:121-157`、两个 Manager 实现）。
- `MakeSafePointID() -> String`：生成 `br-` 前缀的 UUID v4 形态 ID（`safepoint.rs:190-237`）。
- `CheckGCSafePoint(&Context, &dyn Manager, u64)`：要求目标 TS 严格大于当前 GC safepoint；Manager 查询错误按 Go 语义告警后忽略（`safepoint.rs:239-260`）。
- `StartServiceSafePointKeeper(&Context, BRServiceSafePoint, Arc<dyn Manager>)`：校验参数、预检 GC、同步首次设置，然后创建后台续约线程（`safepoint.rs:263-337`）。
- 四个默认 TTL：`DefaultBRGCSafePointTTL=300`、`DefaultCheckpointGCSafePointTTL=4320`、`DefaultStreamStartSafePointTTL=1800`、`DefaultStreamPauseSafePointTTL=86400` 秒（`safepoint.rs:74-85`）。

注意：`SharedError`、`AnnotatedError` 和 `Trace` 位于公开 `safepoint` 模块，但没有在 crate 根的 `pub use` 列表中；需要它们的调用方必须写 `astersql_br_pkg_gc::safepoint::...`。

## 执行流程

`lib.rs` 没有可执行函数，运行时流程由它暴露的 API 串接：

1. 上游持有 `Arc<dyn PdClient>`，调用 crate 根的 `NewManager`。`keyspace_id == NullspaceID` 时选择 `newGlobalManager`；否则选择 `newKeyspaceManager`，后者立即取得并缓存 keyspace 专属 `GCStatesClient`。
2. 上游构造 `BRServiceSafePoint`，通常以 `MakeSafePointID` 生成 ID，并根据备份、检查点或流任务选择默认 TTL。
3. `StartServiceSafePointKeeper` 先拒绝空 ID 或非正 TTL，再调用 `CheckGCSafePoint`；只有 `BackupTS` 尚未被 GC 越过时才同步执行第一次 `SetServiceSafePoint`。
4. global 实现将服务安全点写为 `BackupTS.wrapping_sub(1)`，走 PD 旧服务 safepoint API；keyspace 实现以相同 TS 约定调用 GC barrier API。keyspace 路径的 `TTL <= 0` 会转为删除。
5. 首次写入成功后，Keeper 线程以 `TTL/3` 为续约周期、5 秒为 GC 复检周期工作，直到共享 `Context` 被取消。续约失败只告警并继续；周期复检发现 GC 已越过目标 TS 时 panic，保持 Go `log.Panic` 的终止语义。
6. 生命周期拥有者停止任务时取消 Context，并按需要调用 `DeleteServiceSafePoint`。例如 `br/pkg/task/backup.rs:325-397` 持有 Manager/SP，启动 Keeper，并在清理路径删除服务安全点。

## 数据与状态

crate 根自身没有全局可变状态。它公开的数据与状态所有权如下：

- `Arc<dyn Manager>` 和 `Arc<dyn PdClient>` 负责跨调用方/后台线程共享；`Manager`、`PdClient`、`GCStatesClient` 均要求 `Send + Sync`（`manager.rs`）。
- global Manager 只持有共享 PD 客户端，状态权威在 PD；keyspace Manager 额外持有 `keyspace_id` 和已绑定的 `GCStatesClient`（`manager_global.rs:22-30`、`manager_keyspace.rs:21-44`）。
- `BRServiceSafePoint` 是可克隆值，Keeper 把其克隆值移动到后台线程；`Context` 以 `Arc<AtomicBool>` 共享取消状态（`safepoint.rs:87-129`）。
- `MakeSafePointID` 使用进程内 `AtomicU64` 序号、当前时间和进程 ID 混合生成 ID。独立测试验证格式、顺序唯一性和 100 线程并发唯一性，但它不是持久化 ID 分配器（`safepoint_test.rs:test_make_safe_point_id`）。
- `GCState`/`GCBarrierInfo` 是 Rust 侧仅包含 BR 所需字段的 PD 数据模型；`GCBarrierInfo::TTL` 使用秒数，`i64::MAX` 表示永不过期（`manager.rs:24-61`）。

## 依赖与调用关系

直接 Cargo 依赖只有同仓库的 `astersql-br-pkg-errors`，供 safepoint 参数错误和 GC 越界错误使用；标准库提供 `Arc`、原子量、线程、时间与测试探针文件 I/O。`Cargo.toml` 明确说明生产库不引入 kvproto/grpcio，PD 能力由本 crate 的窄 trait 注入。

主要上游证据：

- `br/pkg/backup/client.rs:98,435,459` 以 `gc::Manager` 暴露 Manager，调用 `CheckGCSafePoint` 和 `MakeSafePointID`。
- `br/pkg/task/backup.rs:325-397` 通过 crate 根类型管理备份 GC 保护的启动与清理。
- `br/pkg/task/stubs.rs:838-891` 在任务适配层保存 `Arc<dyn Manager>` 并转调 `MakeSafePointID`。
- `tests/realtikvtest/brietest/gc_keyspace_test.rs:154` 用 crate 根的 `NewManager` 接上真实测试 RPC，验证 keyspace GC barrier 路径。

主要下游边：`NewManager -> newGlobalManager/newKeyspaceManager`；`StartServiceSafePointKeeper -> CheckGCSafePoint -> Manager::GetGCSafePoint`，以及 `StartServiceSafePointKeeper -> Manager::SetServiceSafePoint`。RustCodeGraph 还将 `NewManager` 的直接 Rust 调用者识别为 `tests/realtikvtest/brietest/gc_keyspace_test.rs:test_keyspace_backup_uses_gc_barrier`；crate 根文件本身主要形成模块/导入边，不形成普通函数调用边。

## 错误处理与边界

- `StartServiceSafePointKeeper` 对空 ID、零/负 TTL 返回带 `ErrInvalidArgument` cause 的 `AnnotatedError`，不会启动线程；GC 预检或首次写入失败也会同步返回（`safepoint.rs:268-288`）。
- `CheckGCSafePoint` 的边界是严格比较：`ts <= safe_point` 返回 `ErrBackupGCSafepointExceeded`；`ts > safe_point` 通过。读取 PD 失败则告警并返回成功，这是有意继承的 Go 可用性取舍，不代表已确认安全点（`safepoint.rs:239-260`）。
- Manager 的 PD/GCStates 错误经 `Trace` 传播；当前 Rust `Trace` 不捕获栈，只保留原错误对象（`safepoint.rs:59-65`）。
- 两种实现都用 `BackupTS.wrapping_sub(1)`。因此 `BackupTS == 0` 会变为 `u64::MAX`；门面没有额外校验，新增调用方不得假定这里会拒绝零时间戳。
- 后台续约错误不结束 Keeper；后台 GC 复检越界则 panic。调用 API 返回 `Ok(())` 只表示线程成功启动，不提供 join handle，也不代表后续续约永久成功。
- global Manager 的 `SetServiceSafePoint` 不像 keyspace Manager 那样在本地把 `TTL <= 0` 显式转为 Delete，而是直接把 TTL 传给旧 PD API；统一语义最终依赖该 API。调用方若需要确定删除，应使用 `DeleteServiceSafePoint`。

## 并发与资源生命周期

`Manager`、`PdClient` 和 `GCStatesClient` 的 `Send + Sync` 约束，使同一实例可以安全放入 `Arc` 并交给 Keeper 线程。`StartServiceSafePointKeeper` 在当前线程完成校验、GC 预检和首次写入；随后 `thread::spawn` 接管 Manager、SP 与克隆 Context。线程没有返回句柄，唯一正常停止机制是调用 `Context::WithCancel` 返回的 cancel 闭包；`Context::Background` 永不自行结束。

线程把下一次续约与复检的 `Instant` 分开维护，并以不超过 10ms 的粒度睡眠，从而轮询取消状态。测试 `safepoint_test.rs:test_global_safepoint_keeper` 和 `test_keyspace_safepoint_keeper` 覆盖首次写入、TTL/3 周期续约、取消后停止、预检边界与查询错误忽略；`mock_test.rs` 提供同步保护的 Mock 状态。`cfg(test)` 挂载方式确保这些测试只编译进测试目标，且位于独立文件。

global/keyspace 实现中的环境变量信号文件与 3 秒观察窗口只在对应设置成功且探针启用时生效；它们服务集成测试观测，会阻塞当前调用线程，不能误写为生产异步行为（`manager_global.rs:72-84`、`manager_keyspace.rs:93-105`）。

## 与 Go 版本的对应关系

Rust crate 根覆盖整个 Go `br/pkg/gc` 包，而 Go 不需要对应的 `lib.go` 门面。对应关系为：

- `manager.rs` ↔ `manager.go`：相同的三方法 Manager 契约和按 `tikv.NullspaceID` 分派的 `NewManager`。
- `manager_global.rs` ↔ `manager_global.go`：全局查询走 `UpdateGCSafePoint(ctx, 0)`，设置/删除走旧 `UpdateServiceGCSafePoint`。
- `manager_keyspace.rs` ↔ `manager_keyspace.go`：按 keyspace 获取 GCStatesClient，以 barrier 表达服务保护，非正 TTL 转删除。
- `safepoint.rs` ↔ `safepoint.go`：默认 TTL、`BRServiceSafePoint`、ID 生成、严格 GC 边界检查、同步首次设置和 TTL/3 续约流程一致。

Rust 为无 Go 包级命名空间的模块系统增加了 crate 根重导出和 `#[path]`；用 `Arc<dyn Trait>` 替代 Go interface，用原子取消标志与 OS 线程替代 `context.Context`/goroutine/ticker。Rust `Context` 只实现取消，不携带 deadline 或 value；`Trace` 也不生成 Go errors 风格的栈。ID 生成实现为本地 xorshift 混合器并塑造成 UUID v4 形式，而 Go 使用 `google/uuid.New()`；现有测试只证明格式与有限样本唯一性，不应把两者视为相同随机性保证。

Go 测试 `manager_test.go`、`safepoint_test.go` 的主要意图已在独立 Rust `manager_test.rs`、`safepoint_test.rs` 和 `parity_test.rs` 中复刻：实现分派、作用域隔离、`BackupTS-1`、TTL 删除、错误传播、默认常量、ID 格式以及 Keeper 生命周期均有对应断言。

## 扩展指南

- 新增调用方通常应优先使用 crate 根重导出；若某符号准备成为稳定公共 API，应同步修改 `lib.rs` 的 `pub use`，并在 `parity_test.rs` 增加公开契约断言，避免只有模块路径可达。
- 扩展 Manager 操作时，必须同时修改 `manager::Manager`、global/keyspace 两个实现、`PdClient` 或 `GCStatesClient` 的最小接口，并同步独立 Mock 和 `manager_test.rs`/`parity_test.rs`。不可只给一种作用域实现默认桩。
- 修改 Keeper 的校验、周期或失败策略时，应同时对照 `safepoint.go`，扩展 `safepoint_test.rs` 的 global 与 keyspace 参数化覆盖；线程生命周期变更尤其要证明取消后没有继续调用。
- 新增生产模块时在 `lib.rs` 添加明确模块声明；新增测试仍放在独立 `*_test.rs` 文件，并只通过 `#[cfg(test)]` 挂载，遵守生产源与测试源分离约束。
- 修改 `BackupTS-1`、TTL 单位、`NullspaceID` 或错误吞吐规则会同时影响兼容性和数据安全，需要 Go/Rust 对等测试与 RealTiKV keyspace 路径验证。新增外部依赖前还应确认 `Cargo.toml` 保持生产树不引入重型 grpc/kvproto 的边界。
- 性能方面，避免在 Keeper 热循环增加阻塞 I/O 或缩短 10ms 轮询；资源方面，若新增 join/停止能力，必须保持现有“首次同步写成功后才返回并启动后台工作”的可观察顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/gc` 确认目标、四个生产实现及 Rust/Go 测试均被索引。
- RustCodeGraph：`node --file br/pkg/gc/lib.rs` 核对 47 行 crate 根、四个生产模块、两组重导出和四个 `cfg(test)` 测试模块；`node` 查询了 `manager.rs`、`manager_global.rs`、`manager_keyspace.rs`、`safepoint.rs` 的完整实现。
- RustCodeGraph：`query`/`explore` 核对 `Manager`、`NewManager`、`MakeSafePointID`、`CheckGCSafePoint`、`StartServiceSafePointKeeper`，并得到 `NewManager -> test_keyspace_backup_uses_gc_barrier`、Manager 方法到 BR task/DDL 调用方等调用证据。由于同仓库存在大量同名 `Manager`/`NewManager`，本文只采用路径限定到 `br/pkg/gc` 的结果。
- Cargo/生产调用方：读取 `br/pkg/gc/Cargo.toml`；用 `rg` 核对 `br/pkg/task/Cargo.toml`、`br/pkg/backup/client.rs`、`br/pkg/task/backup.rs`、`br/pkg/task/stubs.rs` 与 `tests/realtikvtest/brietest/gc_keyspace_test.rs` 的真实引用。
- Go 对照：读取 `br/pkg/gc/manager.go`、`manager_global.go`、`manager_keyspace.go`、`safepoint.go`。
- 独立测试：读取 `br/pkg/gc/parity_test.rs`、`manager_test.rs`、`safepoint_test.rs`，并确认 `mock_test.rs` 是共享 Mock 夹具；测试函数包括 `go_rust_public_contract_matches`、`test_new_manager`、`test_global_manager`、`test_keyspace_manager`、`test_make_safe_point_id`、`test_global_safepoint_keeper`、`test_keyspace_safepoint_keeper`。
- 本任务是只新增说明文档的静态分析，按计划不运行 Cargo；结构验证命令及结果在任务交付时记录。
