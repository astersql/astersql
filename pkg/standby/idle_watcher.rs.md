# `pkg/standby/idle_watcher.rs`

源码：[idle_watcher.rs](./idle_watcher.rs)

## 文件定位

`pkg/standby/idle_watcher.rs` 属于 `astersql-standby` crate；crate 根 `pkg/standby/lib.rs` 公开模块 `idle_watcher`，并再导出 `IdleWatcherConfig` 与 `start_idle_watcher`。它把 Go 版 `LoadKeyspaceController.OnServerCreated` 中的空闲退出判断拆成可配置的 Rust 后台线程，依赖 `astersql-server::server::Server` 读取连接和关闭状态，依赖同 crate 的 `LoadKeyspaceController` 保存重启原因并发出退出请求。

RustCodeGraph 对 `start_idle_watcher` 的调用者查询只找到 `pkg/standby/idle_watcher_test.rs::in_transaction_sleep_connection_prevents_idle_exit`，全仓精确搜索也没有找到生产调用者。因此当前可确认的是“API 已从 crate 根导出且测试可调用”，不能据此声称它已接入服务器启动主链。Go 版则把同类逻辑放在 `pkg/standby/idle_watcher.go::LoadKeyspaceController.OnServerCreated` 生命周期回调中，并由 `cmd/tidb-server/main.go` 在创建 Server 后调用。

## 核心职责

- `IdleWatcherConfig` 集中表达空闲阈值、检查周期、starter 部署模式和零后端开关，避免循环直接读取全局配置。
- `start_idle_watcher` 处理启停边界：阈值为零时完全禁用；启用时先刷新最近活跃时间，再创建命名线程。
- `idle_loop` 周期性组合五类信号作出退出决策：最近活跃时间、连接总数、非 `Sleep` 用户进程数、事务中用户进程数、交互式客户端数。
- `unix_seconds` 为监视器提供 Unix 秒时间；系统时钟早于 Unix epoch 时返回 `0`，不把时间读取错误暴露给调用方。

本文件只“请求退出”，不直接关闭连接、回收 Manager 资源或终止进程；实际动作由 `LoadKeyspaceController` 中的 `ExitSignaler` 以及后续关闭流程承担。

## 主要符号

- `const CLIENT_INTERACTIVE: u32 = 1 << 10`：MySQL `CLIENT_INTERACTIVE` capability 位。`idle_loop` 用它识别需要维持实例的交互式连接；其值与 Go 的 `mysql.ClientInteractive` 语义对应。
- `pub struct IdleWatcherConfig`：可克隆、可调试打印的启动配置。
  - `max_idle: Duration`：允许的最大空闲时长；`Duration::ZERO` 表示禁用。
  - `check_interval: Duration`：轮询间隔。
  - `starter_mode: bool` 与 `zero_backend_enabled: bool`：共同选择零后端的 `Terminate` 路径。
- `impl Default for IdleWatcherConfig`：默认禁用监视，检查间隔为 10 秒，两个模式开关均为 `false`；默认值本身不会创建线程。
- `pub fn start_idle_watcher(...) -> Option<JoinHandle<()>>`：唯一公开启动入口。`None` 明确表示未启用，`Some` 允许调用方在需要时等待线程退出。
- `fn idle_loop(...)`：私有决策循环，拥有 controller、`Arc<Server>` 和配置。
- `fn unix_seconds() -> i64`：私有时间辅助函数，精度为秒。

本文件没有 trait、枚举、宏或条件编译项。

## 执行流程

1. 调用 `start_idle_watcher(controller, server, config)`。若 `config.max_idle.is_zero()`，立即返回 `None`，不刷新活跃时间，也不创建线程。
2. 启用时调用 `LoadKeyspaceController::on_connection_active_now`。该方法通过原子 `fetch_update` 只把 `last_active` 向前推进，避免实例刚启动便沿用旧时间而被退出。
3. 使用 `thread::Builder` 创建名为 `astersql-standby-idle-watcher` 的线程，并将三个参数移动进 `idle_loop`；创建失败会在 `.expect("idle watcher thread must start")` 处 panic。
4. `idle_loop` 在 `Server::force_shutdown()` 为假时先睡眠一个 `check_interval`，然后计算 `unix_seconds().saturating_sub(controller.last_active())`。只有 `idle_seconds > max_idle.as_secs()` 才采集连接状态；恰好等于阈值时继续等待。
5. 循环从 `Server` 获取连接总数、用户进程表和 capability 表。非 `Sleep` 的进程计入 `process_count`；`ProcessInfo.state` 含 `ServerStatusInTrans` 的进程计入 `in_transaction_count`；capability 含 `CLIENT_INTERACTIVE` 的连接计入 `interactive_count`。
6. `(connection_count == 0 || process_count == 0) && in_transaction_count == 0` 定义 `idle_without_transaction`。这意味着“仍有连接但全在 Sleep”也可视为空闲，但任何正在事务中的 Sleep 连接都会否决退出。
7. 若同时处于 starter 模式、启用零后端且业务空闲，循环尝试保存正常重启原因，设置 `Server::set_need_request_manager_free()`，请求 `ExitSignal::Terminate`，然后 `break`。
8. 否则，只有业务空闲且没有交互式客户端时，循环尝试保存相同原因，请求 `ExitSignal::Interrupt`，然后 `break`。
9. 条件不满足则进入下一轮；若其他路径先设置 `force_shutdown`，下一次循环条件检查后自然退出。

## 数据与状态

持久状态不保存在监视线程本身。`IdleWatcherConfig` 在线程启动时按值移动，此后配置不会动态刷新；`server` 通过 `Arc` 共享；`LoadKeyspaceController` 是内部共享状态的克隆句柄。

最近活跃时间位于 `LoadKeyspaceController.inner.last_active`。`on_connection_active_now` 使用 `AcqRel/Acquire` 原子更新且不允许时间倒退，`last_active` 使用 `Acquire` 读取。监视器以秒为单位比较，`max_idle` 的亚秒部分经 `as_secs()` 截断；测试用 `Duration::from_nanos(1)` 因而得到零秒阈值，但仍能绕过仅检查 `Duration::is_zero()` 的禁用分支。

连接快照来自三个独立调用：`connection_count`、`user_process_list`、`client_capability_list`。这些快照之间不是一个原子事务，连接在采样期间变化时，某一轮可能看到不完全一致的计数；循环的周期性检查会在后续轮次重新评估。`user_process_list` 会排除 `ProcessInfo.internal == true` 的内部连接，事务状态以 `ProcessInfo.state` 的 MySQL 状态位为准，而不是 `Server::transaction_list()`。

正常重启原因字符串固定为 `connection idle for too long`。`save_normal_restart_info` 只有本地 keyspace 非空时才写入 `keyspace:message`；文件路径和写入状态由 controller 管理。

## 依赖与调用关系

上游边界如下：

- `pkg/standby/lib.rs` 以 `pub use` 导出 `IdleWatcherConfig` 和 `start_idle_watcher`。
- RustCodeGraph 的当前调用边为 `idle_watcher_test.rs::in_transaction_sleep_connection_prevents_idle_exit -> start_idle_watcher -> idle_loop -> unix_seconds`；没有检出生产侧对 `start_idle_watcher` 的调用。
- Go 对照入口为 `idle_watcher.go::LoadKeyspaceController.OnServerCreated`，`cmd/tidb-server/main.go` 在 `createServer` 和 `PrepareForActivation` 后调用它来启动 goroutine。相邻 Rust 实现 `pkg/standby/standby.rs::StandbyController::on_server_created` 当前只调用 `on_connection_active_now`，没有调用 `start_idle_watcher`，所以空闲线程尚未接入该生命周期钩子。

下游依赖如下：

- 标准库 `Arc`、`thread::{Builder, JoinHandle}`、`Duration/SystemTime/UNIX_EPOCH` 提供共享所有权、线程和时间。
- `astersql_parser_mysql::const::ServerStatusInTrans` 判断事务状态。
- `astersql_server::server::Server` 提供 `force_shutdown`、`connection_count`、`user_process_list`、`client_capability_list` 和 `set_need_request_manager_free`。进程与 capability 列表最终从 Server 的受锁连接集合生成。
- `crate::standby::LoadKeyspaceController` 提供活跃时间、重启信息持久化和退出信号转发；`ExitSignal` 区分 `Interrupt` 与 `Terminate`。
- `pkg/standby/Cargo.toml` 将 crate 根指定为 `lib.rs`，并声明本文件直接使用的 `astersql-parser-mysql`、`astersql-server` 路径依赖；controller 所需的其他 crate 依赖也在同一 manifest 中。

## 错误处理与边界

- `max_idle == Duration::ZERO` 是明确的禁用边界；不会分配后台资源。
- 线程创建不是可恢复错误：`spawn` 失败会 panic，而不是由返回值传给调用方。
- `save_normal_restart_info` 的 `Result` 在两条退出路径都被 `let _ = ...` 丢弃。即使日志写入失败，Manager 标记和退出信号仍继续执行；调用方无法从 watcher 得知持久化失败。
- `unix_seconds` 在 `SystemTime` 早于 epoch 时回退到 `0`。空闲差值用 `saturating_sub`，避免算术溢出；若 `last_active` 大于当前时间，结果为负数并且不会达到正的空闲阈值。
- `max_idle.as_secs() as i64` 在极端大于 `i64::MAX` 秒的 `Duration` 上会截断/回绕；当前代码没有显式拒绝这种配置。这是理论配置边界，正常配置预期远小于该值。
- `check_interval == Duration::ZERO` 不被禁止，会形成高频循环并持续获取连接快照；扩展配置校验时应明确是否允许。
- `command != "Sleep"` 依赖服务器生成的命令字符串。当前独立测试通过 `Command2Str` 取得 `ComSleep` 的真实名称，避免测试硬编码另一种拼写。
- 退出阈值使用严格大于号；空闲恰好 `max_idle` 秒不会退出。

## 并发与资源生命周期

每次启用调用都会创建一个独立 OS 线程；本文件没有单例保护，因此重复调用会创建多个 watcher，并可能重复写重启原因或请求退出。调用方拥有返回的 `JoinHandle`，但生产接线尚未检出，无法确认实际生命周期中是否保存或 join 该句柄。

线程持有 `Arc<Server>`，所以 watcher 退出前 Server 不会被释放。线程没有专用取消 token：正常终止仅发生在观察到 `force_shutdown == true`，或成功走到任一退出决策并 `break`。因为每轮先 `sleep(check_interval)`，外部强制关闭后的可见退出延迟最长约为一个检查周期；睡眠不能被提前唤醒。

Server 内部对连接集合使用读锁生成各次快照；controller 的最近活跃时间使用原子变量；重启日志路径/本地 keyspace 与测试退出信号各自受锁保护。watcher 不在自身范围持有跨调用锁，也不创建 channel、async task 或事务。

零后端路径在请求 `Terminate` 前先设置 `need_request_manager_free`，保持“退出后需 Manager 回收”的顺序；普通路径不设置该标志并请求 `Interrupt`。

## 与 Go 版本的对应关系

Rust 逻辑直接对应 `pkg/standby/idle_watcher.go`：

- Go 的 `OnConnActive` 原子 CAS 循环对应 Rust `LoadKeyspaceController::on_connection_active_now` 的 `fetch_update`，都保证 `lastActive` 不向后移动。
- Go `OnServerCreated` 从全局配置读取 `MaxIdleSeconds`、starter 模式和零后端开关，并固定使用 10 秒 ticker；Rust 将它们显式放入 `IdleWatcherConfig`，便于注入与测试，默认检查间隔仍为 10 秒。
- 两版都在空闲时间严格大于阈值后，计算连接数、非 Sleep 进程数、事务中进程数和交互式连接数；两版的业务空闲表达式及分支优先级相同。
- Go starter + zero-backend 分支设置 Manager 回收标记并发送 `SIGTERM`；Rust 对应 `set_need_request_manager_free` 加 `ExitSignal::Terminate`。普通分支 Go 发送 `SIGINT`；Rust 对应 `ExitSignal::Interrupt`。
- Go 每次超过阈值都会记录包含四种计数的 info 日志；Rust 文件没有等价日志调用。Rust 只保存正常重启原因，且静默忽略保存错误。
- Go goroutine 使用 `time.Ticker` 并在 goroutine 返回时 stop；但发送进程信号后源码没有显式 `return`。Rust 在线程内发出退出请求后立即 `break`，并额外以 `force_shutdown` 作为循环条件。
- Go 生命周期方法由 `pkg/server/standby.go::StandbyController` 声明，并在 `cmd/tidb-server/main.go` 接线；Rust 目前只从 crate 根导出显式启动函数，而 `pkg/standby/standby.rs::on_server_created` 仅刷新活跃时间。RustCodeGraph 与全仓精确搜索均未发现生产调用边，这是迁移接线差异，不应误报为完整等价集成。

Go 同目录没有独立 `idle_watcher_test.go`；本任务可见的直接 Go 事实来源是生产文件本身。Rust 在独立的 `idle_watcher_test.rs` 中补有一项事务保护回归测试。

## 扩展指南

- 新增空闲判据时，优先修改 `idle_loop` 中的快照采集和 `idle_without_transaction`，并保持 starter/zero-backend 分支优先于普通分支；同时核对 Go `LoadKeyspaceController.OnServerCreated`，避免两版退出语义漂移。
- 新增配置项时扩展 `IdleWatcherConfig` 及其 `Default`，并决定配置是启动时快照还是运行期可变。若要求动态更新，应引入明确的共享/通知机制，而不是假设移动进线程的配置会变化。
- 若要完成生产接线，应在真实 Server 创建生命周期处调用 `start_idle_watcher`，定义 `JoinHandle` 的所有权、关闭与 join 策略，并防止重复启动；接线前先用 RustCodeGraph 重新确认已有调用者。
- 若要改善停止延迟，可将不可中断的 `thread::sleep` 替换为具有超时的通知原语，并同时验证退出请求与外部 force-shutdown 的竞态。
- 若改变时间精度或阈值比较，需关注 `Duration::as_secs` 截断、严格 `>` 条件和系统时钟回拨；可考虑注入时钟以获得确定性测试。
- 测试必须继续放在独立的 `pkg/standby/idle_watcher_test.rs`，不要嵌入生产文件。现有测试只覆盖“事务中的 Sleep 连接阻止退出”；建议分别补齐：零阈值不建线程、普通 `Interrupt`、starter 零后端 `Terminate` 与 Manager 标记、交互式连接保护、重启日志失败不阻断退出、force-shutdown 停止，以及重复启动策略。
- 兼容性风险集中在信号类型、事务状态位和交互 capability；性能风险集中在过短 `check_interval` 导致频繁克隆连接快照，以及每次调用创建一个 OS 线程。

## 验证依据

- 目标源码：`pkg/standby/idle_watcher.rs`，核对全部 132 行、常量、配置结构、默认实现、3 个函数及所有分支；文件无条件编译项。
- crate 边界：`pkg/standby/lib.rs` 的模块声明、再导出和独立测试声明；`pkg/standby/Cargo.toml` 的 `[lib]`、porting metadata 与路径依赖。
- RustCodeGraph：`status` 显示索引包含 `pkg/standby/idle_watcher.rs`；`query/node` 定位 `IdleWatcherConfig`、`start_idle_watcher`、`idle_loop`、`unix_seconds`；符号 trail 得到测试到启动入口、启动入口到循环、循环到时间函数，以及相邻 Rust `on_server_created -> on_connection_active_now` 的边。对重载 Server 方法的图边再以其定义源码核验。
- 直接依赖实现：`pkg/standby/standby.rs::on_connection_active_now`、`last_active`、`save_normal_restart_info`、`request_exit` 与 `ExitSignal`；`pkg/server/server.rs::user_process_list`、`client_capability_list`；`pkg/server/standby.rs::StandbyShutdownServer`。
- Go 对照：`pkg/standby/idle_watcher.go::OnConnActive` 和 `LoadKeyspaceController.OnServerCreated`，核对计数、判据、部署分支与信号；`pkg/server/standby.go` 和 `cmd/tidb-server/main.go` 核对接口与生产调用点。
- 独立 Rust 测试：`pkg/standby/idle_watcher_test.rs::in_transaction_sleep_connection_prevents_idle_exit`，证明 `Sleep` 连接的 `ProcessInfo.state` 含 `ServerStatusInTrans` 时不会请求退出，并通过设置 force-shutdown 后 join 回收线程。
- 当前验证限制：没有运行 Cargo（任务明确禁止）；没有找到独立 Go idle-watcher 测试；RustCodeGraph 没有检出生产调用者，因此生产接线状态只报告为“未发现”，不作已接入推断。
