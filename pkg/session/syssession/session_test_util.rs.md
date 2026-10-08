# `pkg/session/syssession/session_test_util.rs`

## 文件定位

本文件是 `astersql-session-syssession` crate 的测试观察与注入层。crate 根模块 `pkg/session/syssession/lib.rs` 无条件声明并公开重导出 `session_test_util`，因此这些 API 在普通构建中也存在，但当前仓库内的直接用途是构造测试会话、观察 `Session`/`AdvancedSessionPool` 的私有状态，以及为其他 crate 的测试提供 mock 会话。RustCodeGraph 将本文件识别为 13 个符号，并报告文件级使用方包括 `pkg/session/syssession/session_test_util_test.rs` 与 `pkg/timer/tablestore/sql_test.rs`；精确文本检索还显示同 crate 的 `session_test.rs`、`pool_test.rs` 和 `session_integration_test.rs` 大量调用这些公开辅助方法。

它不是独立会话实现：实际所有权状态机、上下文操作和关闭逻辑位于 `pkg/session/syssession/session.rs`，池的借还与容量约束位于 `pkg/session/syssession/pool.rs`。本文件只通过同 crate 可见字段为测试打开受控入口。`pkg/session/syssession/Cargo.toml` 指定 `lib.rs` 为 crate 根，并用 `package.metadata.porting.go-package = "pkg/session/syssession"` 标明对应 Go 包；本文件自身只使用标准库 `Arc` 和 crate 内部类型，不引入新的外部依赖或 feature。

## 核心职责

1. `NewSessionForTest` 绕过 `AdvancedSessionPool::Get`，直接把测试提供的 `SessionContext` 包装为由新 `Session` 持有的内部会话。
2. `Session` 上的辅助方法暴露上下文句柄、上下文替换入口、关闭/不可复用标志以及操作计数，支持验证 `session.rs` 的所有权、关闭和并发检测状态机。
3. `AdvancedSessionPool::Size` 与 `Capacity` 暴露空闲队列当前长度和配置容量，支持验证 `pool.rs` 的复用、拒收和容量回退行为。

这些函数不实现 SQL 执行、事务清理或会话归还；它们观察或构造的状态分别由 `Session::with_context`、`close_internal`、`AdvancedSessionPool::Put` 等真实路径维护。

## 主要符号

- `pub fn NewSessionForTest(context: Box<dyn SessionContext>) -> Result<Session>`：先调用 `Session::empty()` 分配唯一 session ID，再用 `new_internal_session(context, session.owner())` 创建 `InternalSession`，成功后写入 `session.internal`。创建过程会针对 `Owner::Session` 调用 `SessionContext::on_became_owner`，其错误原样作为 `SessionError` 返回。
- `Session::InternalSctxForTest(&self) -> Result<SharedSessionContext>`：要求 `internal` 为 `Some`，锁住 `InternalSession` 后克隆其中的 `Arc<Mutex<Box<dyn SessionContext>>>`。实现不检查 `Owner`；`Owner::Closed` 后仍可返回上下文，这一点由 `internal_sctx_for_test_remains_available_after_close` 固定。
- `Session::ResetSctxForTest(&self, replace: impl FnOnce(&mut Box<dyn SessionContext>)) -> Result<()>`：要求内部会话存在且 `state.owner == self.owner()`，随后锁住共享上下文并运行一次性闭包。闭包既可原地修改 mock，也可给 `Box` 重新赋值以替换实现。
- `Session::IsInternalClosed(&self) -> bool`：`internal == None` 或内部 owner 为 `Owner::Closed` 时返回 `true`。
- `Session::IsAvoidReuse(&self) -> bool`：存在内部会话时读取 `avoid_reuse`；空壳会话返回 `false`。
- `Session::InuseForTest(&self) -> u64`：读取 `in_use`，无内部会话时返回 `0`。该计数由 `Session::with_context` 在进入/退出操作时维护。
- `Session::UnsafeForTest(&self) -> u64`：读取 `unsafe_count`，无内部会话时返回 `0`。该计数用于观察线程不安全操作的竞争检测。
- `AdvancedSessionPool::Size(&self) -> usize`：锁住 `sessions: Mutex<VecDeque<_>>` 后返回空闲队列长度；它不是已创建或正在借出的会话总数。
- `AdvancedSessionPool::Capacity(&self) -> usize`：直接返回构造时归一化后的 `capacity`，无锁且不会随运行变化。

本文件没有模块级常量、类型、trait 或条件编译项；全部九个可调用项都是公开 API，其中八个是既有类型的 inherent method。

## 执行流程

构造测试会话的主流程是：测试创建 `Box<dyn SessionContext>` → `NewSessionForTest` 创建空 `Session` → 以该 session 的 `Owner::Session(id)` 调用 `new_internal_session` → `on_became_owner` 成功后形成两层共享状态（内部会话及上下文）→ 返回可直接执行 `Session` 代理方法的句柄。该路径不经过池工厂、空闲队列或 `Pool` owner。

状态观察流程都从 `Session.internal` 开始。关闭判断、不可复用标志和两个计数在持有内部状态锁时读取；上下文读取则在锁内克隆 `Arc` 后返回；上下文重置先确认调用对象仍是 owner，再在内部状态锁保护下取得并锁住 context，最后执行调用方闭包。

池观察流程不改变池：`Size` 在队列锁内读取 `VecDeque::len`，所以可用于断言 `Put` 后是否缓存了会话；`Capacity` 返回 `NewAdvancedSessionPool` 将非法输入归一化之后的固定值。`pool_test.rs::test_new_session_pool` 覆盖正常容量、零、负数和超过 `PoolMaxSize` 时的结果。

## 数据与状态

本文件观察的核心数据来自 `session.rs::InternalSession`：`context` 是共享可变上下文，`owner` 表示池、某个 `Session` 或已关闭，`in_use` 是正在执行的操作数，`unsafe_count` 是线程不安全操作的竞争检测计数，`avoid_reuse` 决定归还时能否重新入池。`NewSessionForTest` 初始化出的值由 `new_internal_session_impl` 设为 `in_use = 0`、`unsafe_count = 0`、`avoid_reuse = false`，owner 则是新 session 自身。

`InternalSctxForTest` 返回的是 `Arc` 克隆而非上下文快照，因此调用方看到同一个 `SessionContext`，且句柄生命周期可超过 `Session::Close`；逻辑关闭不会使已经克隆的 `Arc` 失效。`ResetSctxForTest` 修改的也是这个共享对象，而不是复制品。

`AdvancedSessionPool::sessions` 只保存空闲的 `SharedInternalSession`。借出的会话已经被 `pop_front` 移出队列，所以 `Size` 不包含在用对象；`capacity` 是队列上限，不代表已预分配对象数，工厂仍按需执行。

## 依赖与调用关系

上游方面，`lib.rs` 通过 `pub use session_test_util::*` 将所有符号暴露为 crate API。`session_test_util_test.rs` 直接验证关闭后获取 context 以及关闭后拒绝 reset；`session_test.rs` 使用构造函数和状态计数验证关闭、panic、owner、不可复用与线程不安全操作；`pool_test.rs` 和 `session_integration_test.rs` 用 `Size`/`Capacity` 验证池行为；`pkg/timer/tablestore/sql_test.rs::session_with_calls` 用 `NewSessionForTest` 构建跨 crate 的 `MockPool` 测试会话。

下游方面，`NewSessionForTest` 依赖 `Session::empty`、`Session::owner` 与 `new_internal_session`；后者会进入 `SessionContext::on_became_owner`。其余 session 辅助方法直接读取 `Session.internal` 及 `InternalSession` 字段。池辅助方法直接读取 `AdvancedSessionPool.sessions` 和 `capacity`。RustCodeGraph 的同名函数调用解析未能完整解析 inherent method 调用，并将通用 `Mutex::expect` 错配到其他同名节点，因此调用点以图的文件使用关系配合精确 `rg` 结果核对，而没有把这些错误边当成业务依赖。

在完整应用主链中，本文件不参与正常的系统会话借还；真实链路是 `AdvancedSessionPool::Get`/`Put` 与 `Session` 代理方法。本文件只是测试侧旁路，允许在不搭建池或不公开内部字段的情况下重现和观测该链路。

## 错误处理与边界

- `NewSessionForTest` 会传播 `on_became_owner` 返回的错误。它调用的是 `new_internal_session`（`close_on_failure = false`），因此本层不会在初始化失败时显式调用 `SessionContext::close`；测试上下文若要求失败清理，应在自身生命周期或调用方中明确处理。
- `InternalSctxForTest` 和 `ResetSctxForTest` 在 `internal == None` 时返回 `SessionError("internal session is closed")`。注意普通 `Session::Close` 只把 owner 置为 `Closed`，不会把 `internal` 改为 `None`，所以前者关闭后仍成功。
- `ResetSctxForTest` 在 owner 不匹配（包括 `Owner::Closed` 或已转交池）时返回 `SessionError("session is not owned by the caller")`，并保证不调用替换闭包；独立测试用会 panic 的闭包证明这一边界。
- 四个只读状态方法对没有内部会话的默认值不同：`IsInternalClosed` 为 `true`，`IsAvoidReuse` 为 `false`，两个计数为 `0`。
- 所有 `Mutex::lock` 都使用 `expect`；锁中毒会 panic，而不是转换成 `SessionError`。`Size` 也遵循同一规则。
- 本文件不捕获替换闭包的 panic；panic 会传播并使所持 context mutex 中毒，随后访问可能再次 panic。因此测试替换闭包应保持短小且避免 panic。

## 并发与资源生命周期

`InternalSession` 与 `SessionContext` 分别由 `Arc<Mutex<_>>` 管理。只读辅助方法在短临界区内读取或克隆；`InternalSctxForTest` 返回的共享句柄要求调用方自行锁定 context。`ResetSctxForTest` 的锁顺序是先内部状态、后 context，并在闭包执行期间同时保持两把锁；闭包不得重入需要锁同一 session 内部状态或 context 的方法，否则可能自锁。

`Session::Close` 可在 `in_use > 0` 时先把 owner 标记为 `Closed`，待最后一个真实操作退出后再关闭底层 context。`InuseForTest` 因而可在“逻辑已关闭、资源尚未最终关闭”的窗口返回非零；`session_test.rs::test_internal_session_close` 验证了该时序。`UnsafeForTest` 则显示线程不安全操作的冲突计数，`test_internal_session_un_thread_safe_operations` 验证竞争调用被拒绝以及首个操作退出后计数归零。

`Size` 与 `AdvancedSessionPool::Put`/`Get` 使用同一个队列 mutex，因此单次读数一致，但锁释放后会立即过时，不能作为无锁控制流程的先决条件。`Capacity` 是构造后不变字段，可并发读取。所有这些辅助函数都不拥有额外线程、任务、通道或事务，也不负责回滚、注销或关闭资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/syssession/session_test_util.go`。`NewSessionForTest`、`InternalSctxForTest`、`ResetSctxForTest`、`IsInternalClosed`、`Size` 和 `IsAvoidReuse` 保留了 Go 侧用途，但因 Rust 的所有权和同步模型有以下差异：

- Go `NewSessionForTest` 返回 `*Session`，Rust 返回拥有值 `Session`；两者都把新内部会话的 owner 设为外层 session，并传播初始化错误。
- Go `InternalSctxForTest` 在内部锁内复制接口值后直接返回；Rust 克隆 `SharedSessionContext` 并返回 `Result`。两者都不做 owner 校验。Rust 独立测试明确要求 `Close` 后仍可取得句柄。
- Go `ResetSctxForTest` 的闭包接收旧 context 并返回替代 context；Rust 闭包接收 `&mut Box<dyn SessionContext>`，通过原地修改表达相同替换能力。两者都在持有内部锁时验证 owner 并执行替换。
- Go `IsInternalClosed` 委托内部对象的 `IsClosed`，Rust 直接比较 `Owner::Closed`，并额外把 `internal == None` 视为关闭。Go `IsAvoidReuse` 委托内部方法，Rust直接读取标志。
- Go 池用有缓冲 channel，`Size` 读取 channel 长度；Rust 池用 `Mutex<VecDeque<_>>`，`Size` 在锁内读取队列长度。
- `Capacity`、`InuseForTest`、`UnsafeForTest` 没有同名 Go 公共辅助函数。Go 测试位于同包内，可直接检查 `se.internal.inUse` 和 `se.internal.unsafe`；Rust 测试是独立模块，因字段私有而通过这两个 getter 观察。`Capacity` 则让 Rust 测试验证构造归一化结果，而无需公开池字段。

因此这些额外 Rust 方法是移植测试可观察性的局部接线，不代表新增生产行为。

## 扩展指南

新增内部状态时，优先让真实行为留在 `session.rs` 或 `pool.rs`，只有独立测试确实无法通过公开行为验证时才在本文件增加最小只读观察器。新增方法应明确：空壳 session 的返回值、`Owner::Closed` 与非 owner 的区别、是否可能暴露关闭后的资源，以及锁中毒和闭包 panic 的处理方式。

若扩展 context 注入，最可能修改 `ResetSctxForTest`；必须保持 owner 校验先于闭包调用，并在 `session_test_util_test.rs` 中添加成功替换、非 owner/关闭拒绝和闭包是否执行的独立回归测试。不要把测试放回生产源文件。闭包若需要调用 session API，应先重新设计锁边界，避免当前“内部状态锁 → context 锁”下的重入死锁。

若扩展池指标，应区分空闲数、在用数、累计创建数和容量，不能把 `Size` 误改为总数；同步更新 `pool_test.rs` 和必要的 `session_integration_test.rs`。若改变 Go 对照 API，需同时检查 `session_test_util.go`、`session_test.go` 与 `pool_test.go`，避免 Rust 测试便利接口偏离原测试意图。性能风险主要来自扩大锁内工作或高频调用克隆 `Arc`；兼容风险主要是更改错误文本、关闭后读取语义或空壳默认值。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件且本仓库已索引；`files --filter pkg/session/syssession` 确认目标、模块入口和相关测试；`node --file pkg/session/syssession/session_test_util.rs` 读取目标 1–128 行并给出文件使用关系；`query` 核对九个公开函数/方法；`callers`/`callees` 用于检查调用边，但 inherent method 和通用同名函数存在解析缺口，故未采信错配边。
- Rust 实现：`pkg/session/syssession/session.rs` 的 `SessionContext`、`InternalSession`、`new_internal_session`、`close_internal`、`Session::empty`、`Session::with_context`、`Close`、`AvoidReuse`；`pkg/session/syssession/pool.rs` 的 `AdvancedSessionPool`、`NewAdvancedSessionPool`、`Get`、`Put`。
- crate 边界：`pkg/session/syssession/Cargo.toml` 与 `pkg/session/syssession/lib.rs`。
- Go 对照：`pkg/session/syssession/session_test_util.go`、`session.go`、`pool.go`；计数语义还由 `pkg/session/syssession/session_test.go` 中对 `internal.inUse`/`internal.unsafe` 的断言核对。
- 独立 Rust 测试：`pkg/session/syssession/session_test_util_test.rs`；相关行为覆盖还包括 `session_test.rs`、`pool_test.rs`、`session_integration_test.rs` 和跨 crate 使用方 `pkg/timer/tablestore/sql_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的结构命令，要求目标文档存在且恰好包含上述 11 个固定二级标题；同时人工复核文档只描述当前源码事实，并明确记录 RustCodeGraph 的解析限制。
