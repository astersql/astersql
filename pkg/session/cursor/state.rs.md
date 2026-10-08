# `pkg/session/cursor/state.rs`

## 文件定位

`state.rs` 位于 `astersql-session-cursor` crate，crate 根由 [`pkg/session/cursor/lib.rs`](lib.rs) 声明 `pub mod state`，并通过 `pub use state::*` 重导出本文件的公开类型。该 crate 的边界由 [`pkg/session/cursor/Cargo.toml`](Cargo.toml) 定义；它没有直接依赖项，说明本文件刻意保持为轻量的会话游标状态模型。

在当前 Rust 实现中，本文件不负责打开、读取或关闭游标，只定义交给 [`tracker.rs`](tracker.rs) 保存的状态值。`pkg/session/sessmgr/processinfo.rs::ProcessInfo::CursorTracker` 允许进程快照携带游标跟踪器，但代码搜索未发现 Rust 生产路径构造 `State`；因此，Go 中从 SQL 执行到 GC 安全点上报的完整接线尚不能视为已在 Rust 中落地。

## 核心职责

本文件只有一个职责：用 `State` 保存服务端游标打开时对应的事务开始时间戳 `StartTS`。时间戳使用 `u64`，与 Go 的 `uint64` 字段同宽；该值是 MVCC 读取版本及计算仍被游标占用的最小开始时间戳所需的标识。

`State` 是纯数据快照，没有校验、转换、I/O 或生命周期操作。创建者负责提供语义正确的时间戳，消费者负责解释 `0`、过旧时间戳以及 TSO 编码等业务含义。

## 主要符号

- `pub struct State { pub StartTS: u64 }`：文件唯一的生产符号。类型与字段均公开，保留 Go 命名 `StartTS`；`#[allow(non_snake_case)]` 仅抑制 Rust 命名告警。
- `Clone`、`Copy`：状态可按值传入 `Tracker::NewCursor`、存入 `CursorHandle::state`，并由 `CursorHandle::GetState` 按值返回，不需要共享所有权或借用。
- `Default`：产生 `State { StartTS: 0 }`；跟踪器单元测试用它测试与具体时间戳无关的 ID、查询、遍历和关闭行为。
- `Debug`、`Eq`、`PartialEq`：支持诊断输出和精确值比较；`migration_aster_unit_test.rs::state_and_cursor_ids_match_go_behavior` 用结构相等验证状态往返。

本文件没有常量、trait、函数、显式 `impl`、条件编译项或私有实现。

## 执行流程

当前已实现的 Rust 数据流如下：

1. 调用者构造 `State`，显式写入 `StartTS`，或通过 `State::default()` 得到零值。
2. `tracker.rs::Tracker::NewCursor` 接收该值；`CursorTracker::NewCursor` 分配游标 ID，并把 `State` 按值写入新的 `CursorHandle::state`。
3. `CursorHandle::GetState` 再按值返回保存的快照。后续修改返回值不会反向修改句柄中的状态。
4. `CursorHandle::Close` 只从跟踪器中删除句柄，不改写或清零 `State`；其他已持有的 `Arc<CursorHandle>` 仍可读取其状态。

Go 的完整业务入口位于 `pkg/session/session.go::execStmtResult.TryDetach`：可分离的只读自动提交结果集创建游标时，将 `SessionVars.TxnCtx.StartTS` 写入 `cursor.State`。Go 的 `pkg/domain/infosync/info.go::InfoSyncer.ReportMinStartTS` 随后遍历会话游标并读取 `GetState().StartTS`，以免 GC 越过仍被游标读取的版本。当前 Rust 搜索只确认了上述状态容器与跟踪器数据流，没有确认这两个生产入口的等价接线。

## 数据与状态

`StartTS` 是 `State` 的全部可变数据面，但 `State` 自身没有内部可变性。由于类型实现 `Copy`，状态在创建句柄时被冻结为一个值快照；跟踪器中不存在更新状态的 API。

关键不变量与边界：

- 字段类型是无符号 64 位整数，不会表达负值。
- `Default` 的 `StartTS` 为 `0`，但本文件不把 `0` 判定为错误，也不赋予特殊控制流。
- 本文件不验证 `StartTS` 是否来自当前事务、是否采用合法 TSO 编码、是否早于 GC safe point，或是否单调。
- `Eq`/`PartialEq` 比较的就是完整 `u64` 位值；没有时间精度归一化。

## 依赖与调用关系

下游依赖为空：`state.rs` 只使用 Rust 标准派生能力，不导入其他 crate 或本地模块。

直接上游关系由 RustCodeGraph 和源码共同确认：

- `pkg/session/cursor/lib.rs` 声明并重导出 `state` 模块。
- `pkg/session/cursor/tracker.rs` 导入 `State`；`Tracker::NewCursor` 接收它，`CursorHandle` 保存它，`CursorHandle::GetState` 返回它。
- `pkg/session/cursor/tracker_test.rs` 使用 `State::default()` 覆盖跟踪器行为。
- `pkg/session/cursor/migration_aster_unit_test.rs` 用非零 `StartTS` 验证创建到读取的值保持。
- `pkg/session/sessmgr/processinfo.rs::ProcessInfo::CursorTracker` 将同一 cursor crate 的跟踪器暴露到会话进程信息，但没有直接读写 `State`。

Cargo 侧，`pkg/session/Cargo.toml`、`pkg/session/sessmgr/Cargo.toml`、根 `Cargo.toml` 和 `pkg/executor/staticrecordset/Cargo.toml` 声明了该 cursor crate。需要注意，`staticrecordset` 的依赖只在 `cfg(windows)` 下启用，且其 `cursorrecordset.rs::CursorHandle` 是独立的关闭接口，并未直接采用这里的 `State`/`CursorHandle` 类型。

## 错误处理与边界

本文件没有可失败操作，也没有 `Result`、`Option`、panic 或错误类型。非法或不合业务上下文的 `StartTS` 会被无条件保存；错误防线必须位于构造者或消费方。

相邻跟踪器的互斥锁中毒会在 `NewCursor`、`GetCursor`、`RangeCursor` 或删除时通过 `expect("cursor map poisoned")` panic，但这不是 `State` 的行为。类似地，句柄关闭、ID 回绕和回调中断属于 `tracker.rs`，不应归因于本文件。

新增字段时，不能只依赖 `Default` 掩盖缺失初始化：所有显式结构字面量、Go 对照逻辑和返回快照的兼容语义都需要复核。若字段包含不可 `Copy` 的资源，现有按值 API 与派生集合也必须同步调整。

## 并发与资源生命周期

`State` 由一个 `u64` 组成，不持有锁、原子量、通道、任务、文件、网络连接或事务句柄。`Copy` 使每次传递都产生独立值，因此类型自身没有数据竞争或释放顺序问题。

资源生命周期由 `tracker.rs` 管理：`CursorTracker` 以 `Mutex<HashMap<...>>` 保存 `Arc<CursorHandle>`，句柄以 `Weak<CursorTracker>` 回指跟踪器；`State` 随 `CursorHandle` 存活。`Close` 删除映射项，但只有最后一个 `Arc` 被释放后，句柄及其 `State` 才销毁。`tracker_test.rs::TestCursorTrackerConcurrentCreateDelete` 和 `migration_aster_unit_test.rs::concurrent_create_and_range_delete_is_safe` 验证的是该容器层并发安全，而非本文件内部同步。

Go 侧 `sync.Map` 中同样按值保存 `State`。Rust 的互斥映射实现与 Go 的 `sync.Map` 迭代可见性不必完全相同，但 `State` 作为不可更新快照的语义一致。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/session/cursor/state.go`](state.go)：Go `type State struct { StartTS uint64 }` 与 Rust `pub struct State { pub StartTS: u64 }` 在字段数量、名称、可见性和位宽上逐项对应。Rust 额外显式派生了值复制、默认值、调试与相等比较能力；这些是语言适配，不改变数据内容。

Go 测试 `pkg/session/cursor/tracker_test.go` 主要使用零值 `State{}`，验证 ID、查询、遍历、关闭和并发。Rust 的 `tracker_test.rs` 对齐这些测试；额外的 `migration_aster_unit_test.rs::state_and_cursor_ids_match_go_behavior` 以 `42`、`84` 验证 `StartTS` 不在跟踪过程中丢失。

迁移状态存在明确差异：Go `session.go::execStmtResult.TryDetach` 会用事务 `StartTS` 创建游标，Go `domain/infosync/info.go::InfoSyncer.ReportMinStartTS` 会读取所有活跃游标的 `StartTS`。当前 Rust 的 `domain/infosync/info.rs::InfoSyncer::ReportMinStartTS` 只取 store 与 infoschema 时间戳的较小值，而 `domain/serverinfo/syncer.rs::NoopMinStartTSReporter` 不执行上报；因此 Rust 文件已对齐数据模型，但完整生产消费链仍未由直接证据证明。

## 扩展指南

若要增加游标状态字段，首要修改点是 `State`，随后必须检查 `tracker.rs::Tracker::NewCursor`、`CursorHandle::state` 和 `CursorHandle::GetState` 的按值快照契约。独立测试应继续放在 `tracker_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件；至少覆盖非默认值从创建到读取不变、默认值语义，以及关闭后已有句柄是否仍允许读取。

若要完成生产接线，应分别在 Rust 会话的结果集分离入口写入真实事务 `StartTS`，并在 min-start-TS 汇总路径读取活跃游标状态；这属于其他文件的实现范围，不应在 `state.rs` 中加入会话或 domain 依赖。接线时应以 Go `execStmtResult.TryDetach` 和 `InfoSyncer.ReportMinStartTS` 为行为基线，并覆盖失败清理、游标关闭、过旧时间戳过滤和 GC 安全边界。

兼容性风险主要来自公开结构字面量：新增必填字段会破坏现有 `State { StartTS: ... }` 调用点。性能风险目前很低；若未来状态变大或失去 `Copy`，频繁的 `NewCursor`/`GetState` 复制成本及 API 变化需要评估。不要把锁或资源所有权塞入这个值对象，除非同步重审句柄生命周期与并发模型。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引覆盖本仓库，并列出 `pkg/session/cursor/state.rs`、`tracker.rs`、`tracker_test.rs` 等文件。
- RustCodeGraph `node --file pkg/session/cursor/state.rs`：确认文件仅有公开 `State` 及 `StartTS` 字段；`node pkg/session/cursor/state.rs::State` 显示 `tracker.rs`、`tracker_test.rs` 的导入边。
- RustCodeGraph `node --file`：阅读 `cursor/lib.rs`、`tracker.rs`、`tracker_test.rs`、`migration_aster_unit_test.rs`、`session/sessmgr/processinfo.rs`、`domain/infosync/info.rs`、`domain/serverinfo/syncer.rs` 与 `executor/staticrecordset/cursorrecordset.rs` 的相关源码。
- Cargo/Go 原文：阅读 `pkg/session/cursor/Cargo.toml`、`pkg/session/cursor/state.go`、`tracker.go`、`tracker_test.go`、`pkg/session/session.go::execStmtResult.TryDetach` 和 `pkg/domain/infosync/info.go::InfoSyncer.ReportMinStartTS`。
- `rg` 交叉检查：确认 Rust 中非测试 `State` 的直接保存/返回点集中在 `tracker.rs`，Cargo 引用来自 session、sessmgr、facade 与 Windows 条件下的 staticrecordset，并确认当前 Rust 生产代码未出现 `State { StartTS: ... }` 构造点。

未运行 Cargo 或代码测试：本任务只新增说明文档，按任务计划以事实核对和文档结构检查替代构建测试。
