# [`br/pkg/registry/lib.rs`](./lib.rs)

## 文件定位

`br/pkg/registry/lib.rs` 是 Cargo 包 `astersql-br-pkg-registry` 的 crate 根。`br/pkg/registry/Cargo.toml` 以 `[lib] path = "lib.rs"` 指向本文件，仓库根 `Cargo.toml` 将 `br/pkg/registry` 列为 workspace member；metadata 又明确记录其 Go 对照包为 `br/pkg/registry`。

该文件本身不是注册算法实现，而是迁移期的模块装配与公共 API 门面：它加载 `stubs.rs`、`heartbeat.rs`、`registration.rs`，在测试构建时加载 `parity_test.rs`，再把三个生产模块的公开项平铺重导出到 crate 根。`br/pkg/task/Cargo.toml` 已通过路径依赖消费该 crate，`br/pkg/task/stream.rs` 也实际使用根导出的 `Registry`、`Context` 和 `Error`；不过同文件的 `RegisterRestoreIfNeeded` 仍是空操作，所以当前只能确认配置保护流程已有局部接线，不能把 Rust crate 描述成 Go registry 包的完整生产替代。

## 核心职责

`lib.rs` 只承担四项职责：

1. 通过 `pub mod stubs` 建立本地错误、上下文、SQL session、domain/glue、filter 和任务状态等迁移适配边界。
2. 通过 `pub mod heartbeat` 纳入单次心跳更新与后台 `HeartbeatManager` 生命周期。
3. 通过 `pub mod registration` 纳入恢复任务的登记、恢复、暂停、注销、冲突检查、过期任务处理和全局配置协调逻辑。
4. 通过三条 `pub use ...::*` 提供类似 Go `package registry` 的包级访问体验，并用 `#[cfg(test)] mod parity_test` 保持对齐测试不进入生产构建。

文件级 `#![allow(...)]` 对整个 crate 放宽 dead code、Go 风格命名、未使用项和 Clippy 检查。这反映机械移植兼容需求，不是运行时错误处理，也不能作为所有导出 API 已生产接线的证据。

## 主要符号

- `pub mod stubs`（`lib.rs:20-21`）：公开迁移适配层。关键根级重导出包括 `Result`/`Error`、`Context`、`SqlValue`/`Row`、`RestrictedSQLExecutor`/`Session`、`Storage`/`Domain`/`Glue`、`TaskStatus*`、`PiTRIdTracker` 和 filter API。
- `pub mod heartbeat`（`lib.rs:24-25`）：公开 `UpdateHeartbeatSQLTemplate`、`update_heartbeat`、`HeartbeatManager` 与 `NewHeartbeatManager`。
- `pub mod registration`（`lib.rs:28-29`）：公开 `RestoreRegistryDBName`、`RestoreRegistryTableName`、`FilterSeparator`、`StaleTaskThresholdMinutes`、`RegistrationInfo`、`RegistrationInfoWithID`、`Registry` 与 `NewRestoreRegistry`；`Registry` 的公开方法组成主要业务面。
- `mod parity_test`（`lib.rs:32-34`）：仅 `cfg(test)` 下可见的私有测试模块，验证常量、注册状态机、冲突、心跳、错误和资源生命周期等 Go/Rust 契约。
- `pub use heartbeat::*`、`pub use registration::*`、`pub use stubs::*`（`lib.rs:37-39`）：把子模块公开项提升到 crate 根；调用者无需写 `registration::Registry` 即可使用 `astersql_br_pkg_registry::Registry`。

`lib.rs` 自身没有业务常量、struct、enum、trait、函数或 `impl`；模块声明、条件编译、lint 策略和 glob re-export 就是它的完整逻辑。

## 执行流程

crate 根没有初始化副作用，只有调用者使用重导出的 API 时才执行子模块逻辑。典型注册流程由 `registration.rs` 实现：

1. `NewRestoreRegistry` 通过 `Glue::CreateSession` 创建普通 SQL session 和独立心跳 session，并通过 `Domain::InfoSchema().TableByName` 判断 `mysql.tidb_restore_registry` 是否存在；仅“表不存在”会降级为 `table_exists = false`，其他错误直接返回。
2. `Registry::ResumeOrCreateRegistration` 解析 restored TS，在悲观事务中查找匹配任务，恢复 paused 任务或插入 running 任务，并收集需要等待的 resetting task id。
3. `StartHeartbeatManager` 为当前 restore id 建立后台心跳；`heartbeat.rs` 先立即写一次 `last_heartbeat_time`，之后按固定间隔刷新，直到显式停止或 context 取消。
4. `CheckTablesWithRegisteredTasks`、`PauseTask`、`Unregister`、`FindAndDeleteMatchingTask` 等方法处理表冲突和任务状态转换。
5. `OperationAfterWaitIDs` 等待已记录的 resetting 任务离开该状态后执行配置操作；`GlobalOperationAfterSetResettingStatus` 将当前任务转为 resetting，在没有其他未完成任务时执行全局恢复操作。
6. `Close` 关闭两个 session 并停止心跳管理器；`HeartbeatManager::Drop` 也会兜底停止并 join 工作线程。

当前 Rust 生产接线可直接确认的是 `br/pkg/task/stream.rs` 中 `RestoreTiKVConfigControl` 持有 `Arc<Registry>`，并用上述两个全局配置协调方法包裹 GC ratio 与 RocksDB background jobs 的变更/恢复。恢复任务注册函数本身仍未接线，文档不把 Go `RunRestore` 的完整调用链映射为 Rust 现状。

## 数据与状态

`lib.rs` 不持有数据或全局可变状态。主要状态分布在重导出的实现中：

- `Registry` 持有普通 session、心跳 session、可选 `HeartbeatManager`、`wait_ids` 和 `table_exists`。两个 session 都以 `Arc<Mutex<Box<dyn Session>>>` 保存，既支持 registry 方法串行访问，也允许心跳线程共享专用 session。
- `RegistrationInfo` 保存 filters、start/restored TS、上游 cluster id、是否包含系统表和命令；`RegistrationInfoWithID` 再附加 registry 行 id。
- 持久状态位于 `mysql.tidb_restore_registry`，核心状态值为 `running`、`paused`、`resetting`。`FilterSeparator` 使用 ASCII Unit Separator 连接 filter 字符串，`StaleTaskThresholdMinutes` 为 5。
- `HeartbeatManager` 保存 session、context、restore id、interval、停止发送端和线程句柄；`stubs.rs` 还包含用于测试调整等待时间的进程级原子开关。

`Cargo.toml` 的 `[dependencies]` 为空，说明 SQL、domain、filter 和 context 边界目前由本地 `stubs.rs` 表达，而不是直接依赖完整 TiDB/TiKV Rust 组件。这是迁移状态限制，不应与 Go 版本的真实依赖能力混同。

## 依赖与调用关系

编译依赖方向是 `lib.rs -> {stubs.rs, heartbeat.rs, registration.rs}`。`heartbeat.rs` 使用 `registration` 的库表名常量以及 `stubs` 的 session/context/error；`registration.rs` 同时使用心跳管理器与 stubs 边界。测试构建再增加 `lib.rs -> parity_test.rs`，而 `heartbeat.rs` 自己在 `cfg(test)` 下挂接独立的 `heartbeat_test.rs`。

RustCodeGraph 的 `files --filter br/pkg/registry` 确认目标、两个 Go 对照实现、三个 Rust 生产模块和两份 Rust 测试均在索引中；`node --file br/pkg/registry/lib.rs` 确认该文件只有 39 行装配代码。对 `NewRestoreRegistry` 的查询同时找到 Go `registration.go:213` 与 Rust `registration.rs:354`，证明同名构造入口的对应关系。

生产 Rust 上游是 `br/pkg/task`：其 Cargo manifest 声明 `astersql-br-pkg-registry = { path = "../registry" }`，`stream.rs` 使用 `Registry` 执行 restore 前后的全局配置协调。Rust 测试上游包括 crate 内 `parity_test.rs`、`heartbeat_test.rs`，以及 `br/pkg/task/stream_test.rs` 对 `Registry::from_sessions` 和根级 session trait/type 的使用。

Go 上游更完整：`br/pkg/task/restore.go` 创建并关闭 registry，执行 pause/unregister/conflict check；`br/pkg/task/stream.go` 完成注册、心跳启动及配置协调。它们属于 Go 调用图，不能当作 Rust 已接线证据。

## 错误处理与边界

`lib.rs` 不直接产生错误；重导出的实现使用 `stubs::Result<T>` 与 `Error`：

- `NewRestoreRegistry` 会传播 session 创建错误和非“表不存在”的 infoschema 错误；若第二个 session 创建失败，当前本地 trait 边界没有显式关闭第一个 session，这一点沿用现有实现，扩展资源所有权时需重点复核。
- 事务 helper 在 `BEGIN PESSIMISTIC`、回调、`COMMIT` 任一步失败时返回错误；回调失败会尝试 `ROLLBACK`，rollback 错误不覆盖原回调错误。
- registry 表不存在时，一部分协调方法按无 registry 的兼容路径直接执行回调；其他方法必须依据各自实现判断，不能统一假设为成功。
- 心跳 SQL 写失败会附加 restore id；后台循环与 Go 一样把单次心跳失败视为可恢复，不终止线程，而直接调用 `UpdateHeartbeat` 仍向调用者返回错误。
- 锁获取处存在 `unwrap`；若持锁线程 panic 导致 mutex poisoning，后续调用也会 panic。`Close` 对 poisoned session lock 则跳过该 session 的显式 `Close`。
- 三个 glob re-export 来源若新增同名公开符号，可能导致根命名空间歧义或编译失败；新增 API 必须检查冲突。

## 并发与资源生命周期

并发主要位于 `HeartbeatManager`。`Start` 具有幂等保护：已有 join handle 时不会重复启动。新线程先建立固定速率 deadline、立即发送首个心跳，再用最长 50ms 的超时切片检查 stop channel 与 context；慢心跳之后只追赶一个已到期 tick，避免突发补写。普通 registry session 与心跳 session 分离，心跳线程只锁专用 session，避免与主事务共用一个执行器。

`Stop` 先取出 sender 并发送停止消息，再取出并 join 线程，因此重复调用为空操作，且返回后工作线程不再持有 session 锁。`Drop for HeartbeatManager` 调用 `Stop`；`Registry::Close` 关闭 session 后调用 `StopHeartbeatManager`，而 `Registry` 本身没有 `Drop` 实现，所以生产调用方仍应显式 `Close` 或确保其持有的 manager 被正常析构。

`heartbeat_test.rs::ticker_starts_before_the_initial_heartbeat_like_go` 使用阻塞首个写入的独立测试 session 验证：ticker 在初始心跳前已启动，初始写入释放后已到期 tick 会立刻执行。`parity_test.rs::go_rust_public_contract_matches` 验证 manager 可 start/stop，且 `Close`/心跳错误路径不会破坏测试数据库状态。

## 与 Go 版本的对应关系

模块映射直接对应 Go 同包文件：`heartbeat.rs` 对照 `heartbeat.go`，`registration.rs` 对照 `registration.go`，crate metadata 对照 `br/pkg/registry`。常量、SQL 模板、两 session 架构、悲观事务、任务状态流转、冲突检测、过期任务判断、心跳初写与 ticker 行为，以及 reset 前后全局配置协调均按 Go 语义移植。

可确认的差异包括：

- Go 直接使用 TiDB `domain`、`infoschema`、`kv`、`sqlexec`、table-filter 与日志组件；Rust 通过无外部 Cargo 依赖的本地 traits/stubs 模拟这些边界。
- Go `HeartbeatManager` 持有 `*Registry` 并使用 goroutine/channel；Rust manager 直接持有共享 heartbeat session、context，并使用 OS thread、`std::sync::mpsc` 和 `JoinHandle`。
- Rust 额外提供 `Registry::from_sessions`、`table_exists`、`wait_ids` 等可测接口；这些不是 Go 包的公开同名 API。
- Go `br/pkg/task/restore.go` 已完整创建、使用和关闭 registry；Rust `RegisterRestoreIfNeeded` 当前为空操作，仅配置控制路径明确持有并调用 registry。

测试逻辑保持在独立 Rust 文件：`parity_test.rs` 覆盖正常、边界和错误契约，`heartbeat_test.rs` 专门覆盖 ticker 时序。仓库当前 `br/pkg/registry` 同目录没有 Go `*_test.go`，因此 Go 语义只能由 `registration.go`、`heartbeat.go` 及其生产调用点交叉核对，不能声称获得了本包 Go 单元测试证据。

## 扩展指南

- 新增 registry 业务行为应放入 `registration.rs`，心跳时序与线程行为放入 `heartbeat.rs`，边界适配放入 `stubs.rs`；保持 `lib.rs` 只做模块树和公共面装配。
- 只有确需 crate 根 API 时才增加重导出；新增 public 名称前检查三个模块是否重名，并同步 `parity_test.rs` 的公共契约断言。
- 修改状态转换、SQL 参数顺序、filter 串联、stale 阈值或事务边界会影响恢复互斥与全局配置安全，必须逐项对照 `registration.go`，并在独立 Rust 测试文件中增加正常、边界、错误覆盖。
- 修改 ticker、stop/join、锁顺序或 context 轮询会带来线程泄漏、死锁、停止延迟或写放大风险，应同步 `heartbeat_test.rs`，不得把测试内嵌进 `lib.rs` 或生产实现文件。
- 若要把本地 stubs 替换为外部 Rust 依赖，应遵守仓库规则：在独立上游仓库移植、提交并发布 tag，再以统一 tag 的 Git 依赖接入；不能复制到 vendor/third_party，也不能用本地 `[patch]`。
- 若要完成 Rust 恢复注册主链接线，接入点在 `br/pkg/task/stream.rs::RegisterRestoreIfNeeded` 及相应生命周期调用，但这超出本文件分析任务；实现时必须保持 Go `restore.go`/`stream.go` 的注册、心跳、pause/unregister 顺序。

## 验证依据

本说明基于以下直接证据：

- `br/pkg/registry/lib.rs`：crate 属性、三个生产模块、一个 `cfg(test)` 模块和三条 glob re-export。
- `br/pkg/registry/Cargo.toml` 与根 `Cargo.toml`：package 名、lib 路径、Go package metadata、空依赖表及 workspace membership。
- `br/pkg/registry/registration.rs`：公开常量/数据结构、`NewRestoreRegistry`、`Registry` 状态与全部注册协调方法。
- `br/pkg/registry/heartbeat.rs`：心跳 SQL、manager 构造、线程循环、stop/join 和 `Drop`。
- `br/pkg/registry/stubs.rs`：本地 error/context/session/domain/glue/filter/task-status 适配边界。
- Rust 独立测试：`br/pkg/registry/parity_test.rs`、`heartbeat_test.rs`、`br/pkg/task/stream_test.rs`。
- Go 对照：`br/pkg/registry/registration.go`、`heartbeat.go`，以及生产消费者 `br/pkg/task/restore.go`、`stream.go`；`rg --files br/pkg/registry` 也确认同目录不存在 Go `*_test.go`。
- Rust 生产接线：`br/pkg/task/Cargo.toml` 与 `br/pkg/task/stream.rs` 的 `RestoreTiKVConfigControl`、`OperationAfterWaitIDs`、`GlobalOperationAfterSetResettingStatus`；同文件空操作 `RegisterRestoreIfNeeded` 是尚未完整接线的反向证据。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/registry` 列出 8 个相关 Go/Rust 文件；`node --file br/pkg/registry/lib.rs` 核对完整 39 行；`query NewRestoreRegistry` 核对 Go/Rust 定义位置。`rg` 用于 Cargo、Go 对照、测试挂接和跨 crate 使用点等图未完整呈现的证据。

本任务为纯文档分析，按计划不运行 Cargo。结构验收以目标文件存在且恰含规定的十一个二级标题为准。
