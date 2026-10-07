# `br/pkg/registry/heartbeat.rs`

## 文件定位

本文件是 `astersql-br-pkg-registry` crate 的 restore 任务心跳实现，对应 Go 文件 [`heartbeat.go`](heartbeat.go)。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod heartbeat` 装载它，并以 `pub use heartbeat::*` 将公开符号平铺到 crate 根。该 crate 的 [`Cargo.toml`](Cargo.toml) 把库入口指定为 `lib.rs`，`package.metadata.porting.go-package` 指向 `br/pkg/registry`，且没有外部 Cargo 依赖；SQL 会话、上下文和错误等边界均来自 crate 内的 `stubs`。

它位于 restore registry 元数据链路中：单次心跳把任务行的 `last_heartbeat_time` 更新为当前 Unix 秒；`HeartbeatManager` 在独立线程中重复该动作。它只提供可观测的活跃时间，不负责判定或清理卡住任务。过期判断与状态转移位于 [`registration.rs`](registration.rs) 的 `is_task_stale`、`transition_stale_task_to_paused` 等路径。

当前 Rust 接线止于同 crate 的 `Registry::UpdateHeartbeat`、`Registry::StartHeartbeatManager` 和测试。对 Rust 源的精确引用搜索未发现任务层生产调用；相对地，Go 版本由 `br/pkg/task/stream.go:2580` 的 `RegisterRestoreIfNeeded` 调用 `StartHeartbeatManager`。因此不能把 Go 的完整生产接线误写成 Rust 当前已接线事实。

## 核心职责

- `update_heartbeat` 构造固定库表上的参数化 `UPDATE`，将当前 Unix 秒和 `restore_id` 作为两个 `SqlValue` 交给 `Session::ExecuteInternal`，并为执行错误补充任务 ID。
- `HeartbeatManager` 保存专用 heartbeat session、上下文、任务 ID、刷新间隔、停止发送端和线程句柄；`Start` 负责启动，`Stop`/`Drop` 负责收束。
- 后台循环在启动后立即发送一次心跳，然后按固定速率边界刷新。单次写失败是“尽力而为”：错误不会终止循环。
- 停止既可来自显式 `Stop`，也可来自 `Context::Done()` 或停止通道断开；显式停止会等待工作线程退出。

## 主要符号

- `pub const UpdateHeartbeatSQLTemplate: &str`：公开 SQL 模板。两个 `{}` 只用于注入内部常量 `RestoreRegistryDBName`/`RestoreRegistryTableName`；两个 `%?` 保留给 session 参数绑定。
- `const defaultHeartbeatIntervalSeconds: u64 = 60` 与 `default_interval() -> Duration`：默认 60 秒刷新周期，后者同时供空管理器和正常构造器使用。
- `unix_now() -> i64`：读取 `SystemTime::now()` 相对 Unix epoch 的整秒；系统时间早于 epoch 时返回 `0`，不传播错误或 panic。
- `render_sql(template, args) -> String`：按顺序替换 `{}`，不处理 `%?`。这是私有的轻量模板函数；调用点只传入编译期库表常量。
- `pub fn update_heartbeat(session, ctx, restore_id) -> Result<()>`：本文件的单次写入口。参数顺序固定为时间戳 `I64`、任务 ID `U64`。
- `pub struct HeartbeatManager`：内部字段均私有。`session` 和 `ctx` 用 `Option` 表达未配置状态；`Arc<Mutex<Box<dyn Session>>>` 允许工作线程独占访问非并发 session；`stop_tx`/`join` 同时表达是否有活动 worker。
- `HeartbeatManager::new()`/`Default::default()`：创建没有 session/context、`restore_id = 0` 的空管理器，调用 `Start` 不会启动线程。
- `pub fn NewHeartbeatManager(...) -> HeartbeatManager`：Go 风格公开构造器，只保存配置，不自动启动。
- `HeartbeatManager::Start(&mut self)`：幂等启动；已有 `join`、缺 session 或缺 context 时直接返回。
- `HeartbeatManager::Stop(&mut self)`：发送停止信号并 `join`；对未启动或已经停止的管理器是空操作。
- `impl Drop for HeartbeatManager`：调用 `Stop`，为忘记显式停止的路径提供 RAII 兜底。
- `#[cfg(test)] #[path = "heartbeat_test.rs"] mod heartbeat_test`：测试逻辑保存在独立文件，生产构建不编译该模块。

## 执行流程

1. `registration.rs` 的 `Registry::UpdateHeartbeat` 从 `heartbeat_session` 取出共享 session、加锁，并调用 `update_heartbeat`；session 已关闭时先返回 `heartbeat session closed`。
2. `update_heartbeat` 调用 `unix_now`，用 `render_sql` 把 `mysql.tidb_restore_registry`（由 registration 常量决定）填入 SQL 模板。
3. 它调用 `Session::ExecuteInternal(ctx, sql, [I64(current_time), U64(restore_id)])`。成功返回 `Ok(())`；失败由 `Error::Annotatef` 增加 `failed to update heartbeat for task <id>` 上下文。
4. 周期模式由 `Registry::StartHeartbeatManager` 先停止旧 manager，再克隆专用 heartbeat session 和 context，调用 `NewHeartbeatManager`、`Start`，最后保存 manager。Rust 当前这一包装入口仅在 crate 测试中被调用。
5. `Start` 创建 mpsc 停止通道并拉起线程。线程先把 `next_tick` 定为“当前时刻 + interval”，再持有 session 锁执行一次初始心跳。
6. 初始写完成后，worker 以不超过 50 ms 的 `recv_timeout` 切片等待。每个切片先检查 `ctx.Done()`；收到停止信号或发现发送端断开则退出。
7. 到达 tick 后，循环把 `next_tick` 推进到第一个未来边界，再持锁执行一次心跳。若前一次写很慢，只会立即补一次已到期心跳，不会突发补齐每个错过周期；这一点由 `ticker_starts_before_the_initial_heartbeat_like_go` 固定。
8. `Stop` 取走发送端、尝试发送、取走 join handle 并等待线程结束。此后可再次 `Start`，因为 session/context 仍保留而活动句柄已清空。

## 数据与状态

持久化数据只有 registry 行的 `last_heartbeat_time`。SQL 的 `WHERE id = %?` 将写入限定到一个 restore ID；当前函数不读取受影响行数，因此“不存在该 ID”是否算成功由 `Session::ExecuteInternal` 的语义决定，本层不会额外报错。

管理器状态可按字段组合理解：

- `join = None, stop_tx = None`：未启动或已完全停止。
- `join = Some, stop_tx = Some`：worker 已创建，重复 `Start` 是空操作。
- `session/context = None`：由 `new/default` 创建的空壳，不能启动。
- `restore_id` 在线程创建时按值复制，之后管理器没有修改任务 ID 的接口。
- `interval` 默认为 60 秒，字段私有；同模块独立测试可以构造短间隔以验证调度语义。

`Arc` 只共享 session 所有权，`Mutex` 才是串行化保证。初始心跳和每次周期心跳都在整个 `ExecuteInternal` 调用期间持锁；停止通道不携带业务数据，只传递退出信号。

## 依赖与调用关系

上游关系如下：

- [`lib.rs`](lib.rs) 声明并重导出本模块。
- [`registration.rs`](registration.rs) 的 `Registry::UpdateHeartbeat` 调用 `update_heartbeat`；`Registry::StartHeartbeatManager` 调用 `NewHeartbeatManager` 和 `Start`；`Close`、`PauseTask`、`Unregister` 及再次启动前的路径调用 `StopHeartbeatManager`，后者调用 manager 的 `Stop`。
- [`parity_test.rs`](parity_test.rs) 直接构造 manager，并通过 `Registry` 包装验证单次更新与启停。
- [`heartbeat_test.rs`](heartbeat_test.rs) 直接构造私有字段可见的 manager，验证慢初始写与 ticker 的相对时序。

下游依赖如下：

- `crate::registration::{RestoreRegistryDBName, RestoreRegistryTableName}` 决定唯一目标表，避免本文件复制 schema 名称。
- `crate::stubs::{Context, Error, Result, Session, SqlValue}` 定义取消检测、SQL 执行、类型化绑定和错误包装边界。
- 标准库的 `thread`、`mpsc`、`Arc<Mutex<_>>`、`Instant` 和 `SystemTime` 分别承担 worker、停止通知、session 共享、单调调度与墙钟时间戳。

RustCodeGraph 对本文件给出的直接边包括 `unix_now -> update_heartbeat`、`render_sql -> update_heartbeat`、`update_heartbeat -> Start`、`default_interval -> new/NewHeartbeatManager` 和 `Stop -> drop`。图索引对同名 Go/Rust 符号有歧义，因此生产接线结论又用精确路径引用搜索复核。

## 错误处理与边界

- 单次公开函数保留底层 session 错误并附加 restore ID；它不重试。
- worker 对初始及周期写错误都只忽略并继续，当前 Rust 实现没有像 Go 那样调用日志后端。因此故障可观测性弱于 Go，但存活策略一致。
- `SystemTime` 早于 Unix epoch 时写入 `0`。这是防 panic 的降级值，可能被上游 stale 判定视作很旧的心跳。
- `session.lock().unwrap()` 在锁中毒时会 panic；线程 panic 后 `Stop` 对 `join()` 的错误也会忽略。本层不会把后台 panic 反馈给调用者。
- 已取消的 context 不会阻止初始心跳：`ctx.Done()` 只在初始写之后的等待循环检查。之后取消检测的最长额外轮询延迟约为 50 ms，但若 `ExecuteInternal` 正在阻塞，取消和停止都要等该调用返回。
- `Stop` 的发送失败被忽略，因为接收端已经退出也满足停止目标；`join` 仍用于回收线程。
- `render_sql` 对占位符和参数数量不做严格校验；安全性依赖当前调用只使用固定内部模板与固定库表常量。新增动态标识符时不能把用户输入直接送入它。
- `interval == 0` 会使推进 `next_tick` 的 `while next_tick <= now { next_tick += interval; }` 无法前进。生产构造器固定为 60 秒，当前仅同模块代码能设置字段；未来若开放间隔配置，必须拒绝零值。

## 并发与资源生命周期

每个活动 manager 恰有一个 OS 线程、一个 mpsc 通道和一个 `JoinHandle`。`Start` 先检查 `join`，避免同一 manager 重复拉起 worker；`Registry::StartHeartbeatManager` 还会先停止旧 manager，保证 registry 层最多持有一个心跳 worker。

worker 独占 session 的互斥锁后执行 SQL，因此同一个 heartbeat session 不会并发调用。代价是慢 SQL 会同时延迟下一次心跳和 `Stop`/`Drop` 的完成；`Stop` 没有超时机制。调度用 `Instant`，不受系统墙钟回拨影响；写入值用 `SystemTime`，反映数据库中需要展示的实际时间。

固定速率算法先设定首个 deadline，再执行初始写。若写跨过一个或多个 deadline，算法跳到未来边界并立即补一次，模拟 Go `time.Ticker` 通道最多保留一个待消费 tick 的效果。正常等待以 50 ms 切片换取停止响应性，而不是为每个周期创建新线程。

显式 `Stop` 会先发信号再 join；`Drop` 重用同一逻辑。`take()` 让停止幂等，也避免重复 join。context 自行结束或发送端意外丢弃时 worker 可先退出，后续 `Stop` 仍会 join 已结束线程并清空状态。

## 与 Go 版本的对应关系

共同语义：默认间隔均为 60 秒；构造器只配置、不启动；启动后立即写一次；周期写错误不终止循环；停止等待后台执行单元退出；SQL 更新相同库表、字段和 ID 条件；心跳用于状态洞察而非清理任务。

主要实现映射：

- Go 的 `Registry.UpdateHeartbeat` 方法被拆成 Rust 的底层 `update_heartbeat` 与 [`registration.rs`](registration.rs) 的 `Registry::UpdateHeartbeat` 包装，以便 manager 直接持有专用 session。
- Go `goroutine + time.Ticker + stopCh/doneCh` 对应 Rust `thread + Instant deadline + mpsc + JoinHandle`。
- Go manager 持 `*Registry` 并在 `Start(ctx)` 接收 context；Rust manager 持 `Arc<Mutex<Box<dyn Session>>>` 与克隆后的 `Context`，所以 `Start()` 无参数。
- Go 记录成功、失败、context 结束和停止日志；Rust 当前忽略 worker 内写错误，未提供这些日志。
- Go `Stop` 直接关闭 `stopCh`，按其代码约束只应调用一次且需在 worker 启动后调用；Rust `Stop` 对未启动和重复调用安全，`Drop` 还会自动停止。
- Rust 明确实现慢写后的单次追赶，与 Go ticker 容量为一的可观察行为对齐；独立测试专门保护这一点。

测试证据来自 Rust 独立文件：[`heartbeat_test.rs`](heartbeat_test.rs) 验证 ticker 在初始写之前建立且慢写后立即消费一个已到期 tick；[`parity_test.rs`](parity_test.rs) 的 `heartbeat_manager_constructor_does_not_start_worker` 验证构造/`Stop` 不写、`Start` 后立即写，`go_rust_public_contract_matches` 验证 SQL 常量、单次副作用与 registry 启停。目录中没有对应的 Go `heartbeat_test.go`，Go 语义依据是生产 [`heartbeat.go`](heartbeat.go) 与 `registration.go`/`br/pkg/task/stream.go` 调用链。

## 扩展指南

- 修改 SQL 字段或绑定顺序时，从 `UpdateHeartbeatSQLTemplate` 和 `update_heartbeat` 同步修改，并在独立 `heartbeat_test.rs` 或 `parity_test.rs` 增加捕获 SQL/参数的断言；同时核对 Go `Registry.UpdateHeartbeat`。
- 修改周期、追赶或取消策略时，集中改 `Start` 的 deadline/等待循环，并扩展 `ticker_starts_before_the_initial_heartbeat_like_go`。必须覆盖慢 SQL、context 取消、显式停止、重复启动和零间隔；不要把测试放回生产源文件。
- 若开放可配置 interval，应在构造边界拒绝 `Duration::ZERO`，并评估更短周期带来的系统表写放大。
- 若需要暴露后台失败，先定义与 Go 一致的日志或错误汇报契约；不能简单把首次错误从线程返回，因为现有 API 的 `Start` 无返回值且错误策略是继续运行。
- 若替换 session 类型或减少锁粒度，必须保持同一 session 不被并发调用，并验证 `Stop` 返回后不再发生写入。特别关注阻塞 SQL 导致无限 join 的兼容与运维风险。
- 若完成 Rust 生产接线，入口应沿 `Registry::StartHeartbeatManager` 接入对应 Rust restore/stream 注册成功路径，并让 `Close`、暂停和注销路径继续成对停止；目前不能只凭 Go 接线宣称 Rust 已运行该 worker。
- 兼容风险主要是 Go/Rust ticker 行为、错误吞吐策略和取消时序漂移；性能风险主要是心跳频率、每个 manager 一个线程以及 session 锁覆盖整个 SQL 调用。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标目录列出 `heartbeat.rs`、`registration.rs`、`stubs.rs`、`lib.rs` 及两个独立测试文件。
- RustCodeGraph 读取与查询：`node --file br/pkg/registry/heartbeat.rs`；`query HeartbeatManager`、`query update_heartbeat`、`query NewHeartbeatManager`、`query StartHeartbeatManager`、`query StopHeartbeatManager`；精确 explore 核实 `unix_now`、`render_sql`、`update_heartbeat`、构造器、`Start`、`Stop`/`drop` 与 registration 包装关系。
- 已读 Rust 直接证据：[`heartbeat.rs`](heartbeat.rs)、[`lib.rs`](lib.rs)、[`registration.rs`](registration.rs)、[`heartbeat_test.rs`](heartbeat_test.rs)、[`parity_test.rs`](parity_test.rs)。
- 已读边界/对照证据：[`Cargo.toml`](Cargo.toml)、[`heartbeat.go`](heartbeat.go)、`br/pkg/registry/registration.go`，以及精确引用搜索得到的 `br/pkg/task/stream.go:2580` Go 生产入口。
- 人工复核结论：文档区分了已实现逻辑、Rust 当前接线与 Go 生产接线；说明了 SQL 数据流、固定速率调度、失败策略、锁/线程/通道生命周期、测试位置和安全扩展点，未把桩接口或 Go 路径描述为 Rust 已完成的生产能力。
- 本任务是纯文档分析，按任务约束不运行 Cargo；最终以任务指定命令验证文档存在且固定章节恰为 11 个。
