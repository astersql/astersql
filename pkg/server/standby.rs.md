# `pkg/server/standby.rs`

## 文件定位

`pkg/server/standby.rs` 是 `astersql-server` crate 的待命（standby）生命周期抽象层，由 `pkg/server/lib.rs` 以公开模块 `standby` 导出。它不实现具体的待命状态机或 HTTP 业务，而是定义 Server 与待命控制器之间的能力边界，并提供默认的 `NoopStandbyController`。真正的生产控制器 `LoadKeyspaceController` 位于 `pkg/standby/standby.rs`，通过依赖 `astersql-server` 来实现这里的 trait。

本文件直接依赖标准库的 `Arc` 和同 crate 的 `http_status::Router`；`pkg/server/Cargo.toml` 将 crate 命名为 `astersql-server`、入口设为 `lib.rs`，未为 standby 声明单独 feature，因此这些接口始终属于 server 公共 API，而不是条件编译功能。

## 核心职责

- `StandbyReadyServer` 将激活阶段所需能力收窄为“初始化 TiDB/MySQL 监听器”，避免控制器依赖完整 `Server`。
- `StandbyShutdownServer` 暴露关闭协调所需的健康状态、正常关闭记录、AutoID 服务、强制关闭和 manager-free 标志，以及连接排空等待能力。
- `StandbyController` 规定从 Server 创建、等待激活、准备监听、报告结果、连接活跃到 Server 关闭的完整钩子集合，并允许向状态 HTTP 服务贡献一组带路径前缀的路由。
- `NoopStandbyController` 保持普通部署的默认行为：不阻塞、不注册 standby HTTP 路由、不参与关闭协调，只在准备激活时调用监听器初始化。
- `noop_standby_controller()` 将默认实现擦除为 `Arc<dyn StandbyController>`，供 `Server::new` 统一走与真实待命控制器相同的调用路径。

## 主要符号

- `pub trait StandbyReadyServer: Send + Sync`：仅含 `init_tidb_listener(&self) -> Result<(), String>`。`pkg/server/server.rs` 为 `Server` 实现它，并转调固有方法 `Server::init_tidb_listener`。
- `pub trait StandbyShutdownServer: Send + Sync`：关闭阶段的最小能力集。`health` 默认返回 `true`，`normal_closed_connection` 默认返回 `None`，便于不需要这些查询的测试替身采用保守默认值；其余修改状态、关闭服务和等待连接的方法必须由实现者提供。`Server` 的实现全部转调同名固有方法。
- `pub trait StandbyController: Send + Sync`：控制器协议。`handler` 接受拥有所有权的 `Arc<dyn StandbyShutdownServer>`，使 HTTP 闭包可安全长期持有真实 Server；其他 Server 参数均为同步借用，只覆盖当前钩子调用期。
- `pub struct NoopStandbyController`：无字段、可 `Default` 构造的零大小默认控制器。
- `impl StandbyController for NoopStandbyController`：除 `prepare_for_activation` 外均为空操作或返回 `None`；`prepare_for_activation` 原样返回 `server.init_tidb_listener()` 的结果。
- `pub fn noop_standby_controller() -> Arc<dyn StandbyController>`：创建并类型擦除默认控制器。`pkg/server/server.rs` 的 `Server::new` 是其直接使用者。

## 执行流程

1. `Server::new` 调用 `noop_standby_controller()`；需要真实待命行为时，组装层改用 `Server::with_standby(config, driver, controller)`。
2. `Server::with_standby` 保存控制器，在 `Arc<Server>` 完成构造后立即调用 `on_server_created`。真实 `LoadKeyspaceController` 借此刷新最后活跃时间；默认控制器不做处理。
3. 状态服务构建路由时，`pkg/server/http_status.rs` 调用 `Server::standby_handler`。该方法把真实 Server 克隆并转换为 `Arc<dyn StandbyShutdownServer>` 后传给控制器；若返回 `Some((prefix, router))`，状态服务在此前缀挂载子路由。默认控制器返回 `None`。
4. `Server::run` 完成基础监听与 Domain 设置后调用 `wait_for_activate`。真实控制器可在条件变量上阻塞，默认控制器立即返回。
5. 随后 `run` 调用 `prepare_for_activation`，再将其 `Result` 传给 `end_standby`。默认实现调用 `init_tidb_listener`；该 Server 方法检测已有 listener 时直接成功，因此即使前序已初始化也保持幂等。失败时 `run` 清除 running 状态、关闭监听并返回错误；成功后才置健康状态并启动 status/MySQL/PostgreSQL accept worker。
6. 连接登记成功后，`Server::register_connection` 调用 `on_connection_active`；真实控制器用它更新活跃信息，默认实现忽略。
7. `Server::close` 通过一次性 `close_started` 门闩进入关闭流程，先进入 shutdown mode，再调用 `on_server_shutdown`。真实控制器可关闭 AutoID、等待连接排空并通知 manager；默认实现不额外处理。

## 数据与状态

本文件自身不保存可变业务状态。`NoopStandbyController` 是零大小类型，所有真实状态由实现 `StandbyController` 的对象拥有；`Server` 只保存 `Arc<dyn StandbyController>`。

状态通过能力接口间接访问：`StandbyReadyServer` 控制 listener 初始化；`StandbyShutdownServer` 读写 `health`、`force_shutdown`、`need_request_manager_free`，查询按 keyspace/connection ID 保存的正常关闭消息，并等待连接数归零。对应 `Server` 状态由原子变量、锁、条件变量及连接表实现，接口没有暴露这些内部同步原语。

接口的重要不变量是：控制器对象以及 ready/shutdown server 都必须满足 `Send + Sync`，可跨 Server 工作线程和 HTTP 处理闭包共享；由 `handler` 获得的 Server 是强引用，路由生命周期内不会悬空。

## 依赖与调用关系

上游调用集中在 `pkg/server/server.rs`：`Server::new` 创建默认控制器，`Server::with_standby` 调用创建钩子，`Server::run` 串联等待、准备与结束钩子，连接登记调用活跃钩子，`Server::close` 调用关闭钩子。`pkg/server/http_status.rs` 通过 `Server::standby_handler` 挂载控制器路由。

下游方面，本文件只直接调用 `StandbyReadyServer::init_tidb_listener`。`pkg/server/server.rs` 将三个 trait 适配到真实 `Server`；`pkg/standby/standby.rs` 的 `LoadKeyspaceController` 是主要生产消费者，它用 shutdown 能力实现 `/tidb-pool/activate`、`/exit`、`/checkconn`、健康拒绝、连接排空和 manager-free 流程。

crate 边界刻意避免循环依赖：`astersql-server` 只定义接口和通用 `Router` 类型，不依赖 `astersql-standby`；后者的 `Cargo.toml` 单向依赖 `astersql-server` 并实现接口。

## 错误处理与边界

激活准备使用 `Result<(), String>`。默认控制器不包装或吞掉 `init_tidb_listener` 的错误，调用方 `Server::run` 负责回滚运行标志、关闭监听并返回同一错误。`end_standby` 同样接收该结果，允许真实控制器把成功或失败通知给等待中的激活 HTTP 请求；其返回值为 `()`，报告过程不能替换原始启动结果。

`handler` 用 `Option` 表示是否存在待命路由，默认 `None` 是正常边界而非错误。`normal_closed_connection` 的默认 `None` 表示没有证据证明连接正常关闭。`health` 的默认 `true` 主要降低替身实现成本；需要根据关闭状态拒绝重复激活的生产实现必须依赖真实 `Server` 覆盖值。

本文件不验证路由前缀、不捕获实现者 panic，也不规定钩子重复调用语义。实际 Server 对 `close` 有一次性保护；真实 `LoadKeyspaceController::end_standby` 也用原子门闩保证只发布一次结果。实现新控制器时不能假设所有钩子天然幂等，应明确处理重复 HTTP 请求和并发关闭。

## 并发与资源生命周期

`Send + Sync` 约束和 `Arc` 是本文件的并发契约。控制器会同时被 Server 主流程、连接工作线程和状态 HTTP 路由访问，因此实现内部的状态必须使用原子量、互斥锁、条件变量或等价同步手段。`handler` 获取 `Arc<dyn StandbyShutdownServer>`，而创建、激活和关闭钩子只获取借用，体现了路由需要越过函数调用长期持有 Server、普通钩子不应延长生命周期的区别。

等待职责属于控制器：`wait_for_activate` 可以阻塞启动线程，`wait_zero_connections_timeout` 则由 Server 的条件变量实现带期限排空。资源释放职责仍由 Server 主流程拥有；控制器只能通过能力方法请求关闭 AutoID、设置标志或等待连接，不能直接取得 listener、连接表或锁。默认控制器既不生成后台任务也不持有资源。

## 与 Go 版本的对应关系

`pkg/server/standby.go` 定义同名的 `StandbyController`、`StandbyReadyServer` 和 `StandbyShutdownServer`。Rust 的 `wait_for_activate`、`end_standby`、`handler`、`on_connection_active`、`prepare_for_activation`、`on_server_created`、`on_server_shutdown` 分别对应 Go 的同名驼峰方法，`StandbyReadyServer::init_tidb_listener` 对应 `InitTiDBListener`。

Rust 保留了 Go 的关键时序意图：激活成功返回前初始化监听器，状态服务挂载控制器路由，连接活跃时通知控制器，关闭时把受限 Server 能力交给控制器。`pkg/server/tests/standby/standby_test.go` 的 `TestStandby` 验证等待激活、状态路由可达和连接活跃通知；Rust 的 `pkg/server/tests/standby/standby_test.rs` 覆盖相同主链，并额外断言创建、准备、结束和关闭钩子的调用次数。

接口并非逐字段机械翻译。Go 的 `Handler` 接收 `*Server`，Rust 以 `Arc<dyn StandbyShutdownServer>` 限制能力并满足 HTTP 闭包生命周期；Rust shutdown trait 还包含 `health`、`normal_closed_connection` 和 `wait_zero_connections_timeout`，服务于已移植的激活健康判断、checkconn 与无需派生线程的超时排空。Go 接口只有阻塞式 `WaitZeroConn`，其 `LoadKeyspaceController` 自行启动 goroutine 并通过 timer 超时。Rust 的真实 `LoadKeyspaceController::prepare_for_activation` 会自行调用一次 `end_standby`，同时 `Server::run` 也调用它；控制器内部的 `end_once` 保证结果只发布一次。

## 扩展指南

新增待命策略时应实现 `StandbyController`，把长期状态放入实现者自己的同步结构中，并优先通过现有 ready/shutdown 能力完成工作。若确需新的 Server 能力，应先把最小方法加入相应 trait，再在 `pkg/server/server.rs` 的 `Server` 适配实现中转调；不要把完整 `Server` 暴露给控制器，以免破坏 crate 单向依赖和可测试性。

新增激活路由时，应在控制器的 `handler` 返回的 `Router` 中注册相对路径，并核对 `pkg/server/http_status.rs` 的前缀挂载语义。改变启动时序时重点检查：激活响应不能早于监听准备完成，`end_standby` 必须收到与 `prepare_for_activation` 相同的结果，失败路径必须让 `Server::run` 回收监听资源。改变关闭协议时要保持强制关闭跳过等待、AutoID owner 处理、连接排空期限和 manager-free 标志的兼容语义。

测试逻辑必须继续放在独立文件。接口/默认实现的局部回归可扩展 `pkg/server/standby_test.rs`；完整生命周期应扩展 `pkg/server/tests/standby/standby_test.rs`，并与 `pkg/server/tests/standby/standby_test.go` 的意图对齐；生产控制器行为则在 `pkg/standby/standby_test.rs` 或 `pkg/standby/standby_nextgen_test.rs` 覆盖。并发测试应使用有界等待，避免无期限挂起。

## 验证依据

- 目标源码：`pkg/server/standby.rs`，确认三个公开 trait、默认控制器及构造函数的完整定义。
- crate 与模块入口：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`，确认 `astersql-server` 边界、公开模块以及独立 `standby_test.rs` 的测试接线。
- Server 调用链：`pkg/server/server.rs` 中 `Server::new`、`with_standby`、`standby_handler`、`register_connection`、`run`、`close`、`init_tidb_listener`，以及三个能力 trait 的 `Server` 实现。
- HTTP 接线：`pkg/server/http_status.rs` 中对 `server.standby_handler()` 返回路由的前缀挂载。
- 生产实现：`pkg/standby/standby.rs` 中 `LoadKeyspaceController::handler`、`wait_zero_conn` 及其 `StandbyController` 实现；`pkg/standby/Cargo.toml` 证明它单向依赖 `astersql-server`。
- Go 对照：`pkg/server/standby.go`、`pkg/server/server.go`、`pkg/server/http_status.go`、`pkg/standby/standby.go`。
- 测试证据：`pkg/server/standby_test.rs` 验证 handler 收到真实 Server；`pkg/server/tests/standby/main_test.rs` 验证默认控制器恰调用一次 ready 能力；`pkg/server/tests/standby/standby_test.rs` 与同目录 Go 测试验证激活、路由、连接通知和关闭主链。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`query StandbyController --kind trait` 定位到 `pkg/server/standby.rs:65`，`query noop_standby_controller --kind function` 定位到第 111 行，`query LoadKeyspaceController --kind struct` 定位生产实现到 `pkg/standby/standby.rs:232`。文件级 `explore/node` 未返回文本，因此调用关系另以上述已索引符号查询和直接源码读取交叉核验。

