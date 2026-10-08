# `pkg/sessiontxn/failpoint.rs`

## 文件定位

[`failpoint.rs`](failpoint.rs) 属于 `astersql-sessiontxn` crate。crate 根文件 [`lib.rs`](lib.rs) 以私有模块 `mod failpoint` 装入它，再通过 `pub use failpoint::*` 公开本文件的常量、类型别名、trait 和函数。因此，它是会话事务层面向故障注入测试的公共观测/断言适配层，而不是注册或触发 failpoint 的运行时。

本文件把 Go `pkg/sessiontxn/failpoint.go` 依赖的 `sessionctx.Context.Value/SetValue`、事务管理器查询和 channel hook 抽象成 Rust trait。当前 Rust 仓库中这些 API 的直接调用集中在 `pkg/sessiontxn/*_test.rs`；未发现会话、执行器或隔离级别 Rust 生产代码调用它们。完整生产注入位置仍可在 Go 的 `pkg/session/session.go`、`pkg/executor/{adapter,compiler}.go` 和 `pkg/sessiontxn/isolation/*.go` 中观察到。因此这里已经具备可测试的辅助行为，但尚不能表述为已接入 Rust SQL 执行主链。

## 核心职责

文件承担四类职责：

1. 定义跨模块约定的字符串键和断点名，例如 `AssertRecordsKey`、`AssertTxnInfoSchemaKey`、三个 TSO 计数键和两个 `BreakPoint*` 常量。
2. 用 `SessionValueStore` 将动态类型的会话值存取收敛为一个小接口，并用 `TxnAssertionContext` 补充 InfoSchema 断言所需的本地临时表身份观测。
3. 提供测试断言与记账函数：`RecordAssert`、`AssertTxnManagerInfoSchema`、`AssertTxnManagerReadTS`、锁错误计数和 TSO/重试计数。
4. 通过 `ExecTestHook` 从会话值中取得一次性闭包，在最多十秒内等待并执行，以支持断点式测试同步。

这些函数以 panic/assert 反馈测试不变量被破坏，不提供业务错误恢复。它们记录的是会话局部的测试观测，不负责事务创建、重试决策、TSO 请求或 InfoSchema 构造本身。

## 主要符号

- 键与断点常量：`AssertRecordsKey`、`AssertTxnInfoSchemaKey`、`AssertTxnInfoSchemaAfterRetryKey`、`BreakPointBeforeExecutorFirstRun`、`BreakPointOnStmtRetryAfterLockError`、`TsoRequestCount`、`TsoWaitCount`、`TsoUseConstantCount`、`CallOnStmtRetryCount`、`AssertLockErr`。其字符串值与 Go 对照文件一致，属于跨调用点契约；仅修改常量名而不修改字符串值不改变会话存储协议，修改字符串值则必须同步所有生产注入点和测试。
- 动态值类型：`SessionValue = Box<dyn Any>`；`AssertRecords = HashMap<String, SessionValue>` 保存任意断言载荷；`LockErrorRecords = HashMap<String, i64>` 按错误名计数；`TestHook = Receiver<Box<dyn FnOnce() + Send>>` 表示单消费者、一次性回调通道。
- `SessionValueStore`：公开 `Value`、`ValueMut`、`SetValue`。读取以借用形式返回 `dyn Any`，调用者必须按约定类型 `downcast`；写入键要求 `&'static str`，保证本文件常量和长期存储键的生命周期安全。
- `TxnAssertionContext: SessionValueStore + TxnManagerContext`：提供 `LocalTemporaryTablesIdentity` 与 `TxnInfoSchemaLocalTemporaryTablesIdentity`。默认都返回 `None`，使不支持本地临时表观测的上下文仍可使用普通 InfoSchema 版本断言。
- `HookKey`：把 `str` 或 `String` 统一转换为拥有所有权的键。`ExecTestHook` 因而可接受字符串切片和字符串对象。
- `RecordAssert`：惰性创建 `AssertRecords`，随后按名称插入；同名写入覆盖旧值。
- `AssertTxnManagerInfoSchema`：检查可选的本地临时表身份，并依次校验显式 `expected` 与会话中 `AssertTxnInfoSchemaKey` 保存的期望版本。
- `AssertTxnManagerReadTS`：经 `GetTxnManager(sctx).GetStmtReadTS()` 获取当前语句读时间戳，取值失败或不匹配均 panic。
- `AddAssertEntranceForLockError`：惰性创建或替换错误类型的 `LockErrorRecords`，再对指定错误名加一。
- `increment_u64` 及 `TsoRequestCountInc`、`TsoWaitCountInc`、`TsoUseConstantCountInc`：三个公开函数共享私有的 `u64` 加一逻辑。
- `OnStmtRetryCountInc`：独立使用 `i64` 保存重试次数，以对应 Go 的 `int` 语义形状。
- `ExecTestHook`：键缺失或值类型不是 `TestHook` 时直接返回；类型正确时最多等待十秒，收到闭包后消费并执行。

## 执行流程

记账类函数都遵循“读取并尝试向下转型—缺失或类型错误则从默认状态重建—更新—写回”的流程。`RecordAssert` 和 `AddAssertEntranceForLockError` 首先确保对应键下是正确的 map，再通过 `ValueMut` 原地更新；计数函数则读取旧数值、计算 `old_or_zero + 1`，然后用 `SetValue` 替换保存值。Rust 独立测试验证了同名断言覆盖、分类计数，以及错误类型值被重置为 1 的分支。

`AssertTxnManagerInfoSchema` 先读取并克隆会话中保存的 `InfoSchemaRef`，以结束对 `sctx` 的不可变借用。若会话报告本地临时表身份，它再向事务 InfoSchema 查询对应身份并要求两者相等。最后，它按“显式期望、已存期望”的顺序遍历两个非空值，每次通过 `GetTxnManager` 取得当前事务 InfoSchema 并比较 `SchemaMetaVersion()`；两个期望都存在时两者都必须匹配。

`AssertTxnManagerReadTS` 调用事务管理器的 `GetStmtReadTS`。错误通过 `unwrap_or_else` 转为带上下文的 panic；成功后比较期望时间戳。`txn_context_test.rs` 证明该断言可穿插在 `NewTxn`、`OnStmtStart`、`OnStmtCommit`、`OnStmtEnd` 生命周期之间，但这只是 mock manager/provider 上的调用组合，不代表 Rust executor 已安装相同 failpoint。

`ExecTestHook` 先通过 `HookKey::String` 取得自有键，再读取 `TestHook`。存在时调用 `recv_timeout(Duration::from_secs(10))`；收到 `FnOnce` 后立即执行，超时或通道断开均走同一 panic 分支。每个 receiver 消费一个回调，函数本身不循环。

## 数据与状态

所有状态都由调用者实现的 `SessionValueStore` 持有，本文件没有全局可变状态。字符串常量只是键，不保存计数或断言结果。由于值使用 `Any`，键和值类型形成运行时协议：`AssertRecordsKey` 对应 `AssertRecords`，`AssertLockErr` 对应 `LockErrorRecords`，三个 TSO 键对应 `u64`，`CallOnStmtRetryCount` 对应 `i64`，hook 键对应 `TestHook`，`AssertTxnInfoSchemaKey` 对应 `InfoSchemaRef`。

错误类型值的处理并不完全一致：记录 map 和计数器会被静默替换/重置；`ExecTestHook` 对错误类型视为空操作；`AssertTxnManagerInfoSchema` 对错误类型的已存期望也忽略，因为 `downcast_ref` 返回 `None`。这是对 Go 类型断言失败分支的刻意对应。`RecordAssert` 在初始化后若实现违反“刚写入即可按同一类型读回”的接口契约，会在 `expect` 处 panic；`AddAssertEntranceForLockError` 同理。

`InfoSchemaRef` 被克隆后用于版本比较，不改变 InfoSchema。临时表通过 `Option<usize>` 表示对象身份而非内容相等；只有会话侧身份为 `Some` 时才查询事务侧身份。测试 `failpoint_test.rs` 专门证明会话没有本地临时表时不会调用事务侧身份观测。

## 依赖与调用关系

标准库依赖仅有 `Any`、`HashMap`、`mpsc::Receiver` 和 `Duration`。crate 内依赖为 `GetTxnManager`、`TxnManagerContext` 与 `InfoSchemaRef`；它们通过 `crate` 根的再导出使用。`Cargo.toml` 将本 crate 命名为 `astersql-sessiontxn`，库入口为 `lib.rs`，并声明 `astersql-infoschema` 等路径依赖；本文件没有 feature 条件或条件编译项。

RustCodeGraph 对主要符号的查询显示：`RecordAssert`、InfoSchema/read-TS 断言、计数函数和 `ExecTestHook` 的 Rust 调用边都落在 `txn_context_test.rs`、`txn_rc_tso_optimize_test.rs`、`failpoint_test.rs` 或 `sessiontxn_aster_unit_test.rs`。`lib.rs` 在 `cfg(test)` 下装载这些独立测试，并公开再导出本文件 API。直接仓库搜索没有找到目标 API 在 Rust 会话/执行器生产文件中的调用。

Go 主链提供迁移参照：`session/session.go` 和 `executor/{adapter,compiler}.go` 在编译、建执行器、重建计划和悲观锁重试点调用断言/记录函数；`sessiontxn/isolation/{base,readcommitted,repeatable_read}.go` 在实际 TSO 与重试分支递增计数；`executor/adapter.go` 使用两个断点名和 retry 后 InfoSchema 键。这些 Go 边是 Rust 后续接线的目标证据，不是当前 Rust 已完成接线的证据。

## 错误处理与边界

这是测试辅助模块，失败策略是尽快 panic：InfoSchema 版本、临时表身份或 ReadTS 不匹配时使用 `assert_eq!`；取得 ReadTS 返回错误时显式 panic；等待 hook 超时或 sender 断开时也 panic。它不返回 `Result`，因此不应在需要优雅恢复的线上业务路径中直接作为校验机制。

动态类型存储的边界值得特别注意：错误类型的计数值会导致下一次计数从 1 开始，而不是报错或保留旧值；错误类型的 hook/已存 InfoSchema 期望会被忽略。若测试需要检测错误类型，应在调用前后直接检查 `SessionValueStore`，不能依赖这些函数报错。

计数使用普通 `+ 1`，没有饱和或显式溢出处理；其设计面向短生命周期测试计数。`ExecTestHook` 的十秒是硬编码上限，且“超时”和“通道断开”产生相同错误文字。`Receiver` 不是可克隆的广播机制，一个成功调用只消费一个 `FnOnce`；若还要执行后续 hook，必须继续持有 sender 并再次排队。

## 并发与资源生命周期

本文件不创建线程或异步任务，也不加锁。状态一致性完全依赖 `SessionValueStore` 实现及调用约束：修改函数需要 `&mut C`，Rust 借用规则阻止同一上下文在安全代码中被同时可变访问；`ExecTestHook` 只需 `&C`，但其 `Receiver::recv_timeout` 会阻塞当前线程最多十秒。

`TestHook` 中的闭包要求 `Send`，允许发送端来自其他线程；闭包是 `FnOnce`，执行后资源随闭包消费。receiver 由会话值存储拥有，本函数只借用它，不移除键。sender 全部释放时等待会立即返回断开错误并 panic，而不是等满十秒。

InfoSchema 引用的生命周期由 `InfoSchemaRef` 自身管理，本函数只克隆引用并读取版本。本地临时表只比较调用者提供的身份数字；身份的有效性、复用风险及对象生命周期由 `TxnAssertionContext` 实现负责。

## 与 Go 版本的对应关系

Rust 函数和字符串值逐项对应 `pkg/sessiontxn/failpoint.go`。`SessionValueStore` 对应 Go `sessionctx.Context.Value/SetValue`；`Box<dyn Any>` 对应 `any`；`AssertRecords`、`LockErrorRecords` 分别对应 `map[string]any`、`map[string]int`；`HookKey` 对应 Go 的 `fmt.Stringer` 键；`TestHook` 对应 `chan func()`。

核心行为保持一致：map 惰性初始化、同名覆盖、类型断言失败后重建/归零、两个 InfoSchema 期望都要检查、ReadTS 错误转 panic、hook 类型不匹配时忽略，以及十秒等待上限。Rust 使用 `i64` 近似 Go `int` 的重试/锁错误计数，使用 `u64` 对齐 TSO 计数。

有两点实现形态差异。第一，Go 直接从具体 `sessionctx.Context` 取 `LocalTemporaryTables` 并断言事务 InfoSchema 是 `SessionExtendedInfoSchema`；Rust 通过 `TxnAssertionContext` 的两个身份方法解耦具体类型，默认实现允许上下文声明“无可观测本地临时表”。第二，Go channel 传递 `func()`，Rust receiver 传递 `Box<dyn FnOnce() + Send>`，把“一次消费”和跨线程传递约束写进类型。

迁移完成度也有差异：Go 生产文件已经在 executor/session/isolation failpoint 中调用这些辅助函数；Rust 当前只有 crate 导出和独立测试调用。`AssertTxnInfoSchemaAfterRetryKey` 与两个断点常量在 Rust 中保留了协议名，但尚未发现对应 Rust 生产注入点。

## 扩展指南

新增观测键时，应在本文件定义唯一的 `&'static str` 常量，明确对应值类型，并在独立测试文件中覆盖“缺失、正确类型、错误类型、重复调用”分支；不要把测试模块内嵌回生产源文件。若该键来自 Go 移植，必须核对字符串字面值和 Go 的类型断言失败语义。

新增事务断言时，优先扩展窄 trait 暴露必要观测，而不是让本文件依赖完整 session 实现。需要事务管理器状态时复用 `TxnManagerContext`/`GetTxnManager`；涉及本地临时表时同步 `TxnAssertionContext` 的 mock。断言应明确它只适用于测试，错误消息要指出具体不变量。

把现有 helper 接入 Rust 主链时，最可能修改的是未来对应 Go `session/session.go`、`executor/{adapter,compiler}.go` 与 `sessiontxn/isolation/*.go` 的 Rust 实现。接线必须同时增加独立 Rust 回归测试，证明调用发生在真实编译、建执行器、重试或 TSO 分支；不能只靠直接调用 helper 的单元测试声称主链已覆盖。

修改 `ExecTestHook` 时应保留无键/错误类型不阻塞、单次消费和有界等待三个性质。若引入异步 channel 或可配置超时，需要评估阻塞线程、闭包 `Send` 边界和 Go 测试协议兼容性。修改计数类型、键字面值或错误类型策略都可能破坏跨模块测试约定，需同步所有调用点与 `txn_rc_tso_optimize_test.rs`。

## 验证依据

- 源码与模块边界：`pkg/sessiontxn/failpoint.rs`、`pkg/sessiontxn/lib.rs`、`pkg/sessiontxn/Cargo.toml`。目标包不存在 `pkg/sessiontxn/doc.go`。
- Go 对照：`pkg/sessiontxn/failpoint.go`；生产调用证据来自 `pkg/session/session.go`、`pkg/executor/adapter.go`、`pkg/executor/compiler.go`、`pkg/sessiontxn/isolation/base.go`、`readcommitted.go`、`repeatable_read.go`。
- Rust 独立测试：`pkg/sessiontxn/failpoint_test.rs` 验证无本地临时表时不访问事务侧身份；`txn_context_test.rs` 验证记录覆盖、InfoSchema 双期望/身份断言、ReadTS 和生命周期组合；`txn_rc_tso_optimize_test.rs` 验证计数、错误类型重置、分类锁错误、hook 空操作/执行和重试记账；`sessiontxn_aster_unit_test.rs` 提供附加的公开契约覆盖。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query` 同时定位 Go/Rust 的 `RecordAssert`、`AssertTxnManagerInfoSchema`、`ExecTestHook`；`explore` 给出的 Rust blast radius 只包含上述独立测试，而 Go `RecordAssert`/`AssertTxnManagerInfoSchema` 指向 `session/session.go::runStmt`。文件过滤命令未命中目标路径，但精确符号查询和 `node --file` 能读取目标、Go 对照及测试。
- 人工检索：对全部相关符号进行 Rust/Go 仓库搜索，确认 Rust 生产目录没有调用，Go executor/session/isolation 存在真实调用；此结论描述的是当前检出状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证本文档存在且恰好包含 11 个固定二级标题，并检查 Git diff 只包含本文档和任务文件的预期删除。
