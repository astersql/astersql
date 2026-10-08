# `pkg/testkit/stepped.rs`

## 文件定位

[源文件 `pkg/testkit/stepped.rs`](stepped.rs) 属于 `astersql-testkit` crate；`pkg/testkit/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 crate 根，`pkg/testkit/lib.rs` 再以 `pub mod stepped` 暴露本模块。它不是数据库请求主链的一部分，而是测试基础设施：让测试把 SQL 操作、自定义动作和同步点排成一个队列，在独立线程中顺序执行，并由测试主线程精确控制断点放行时机。

当前仓库内 Rust 侧的直接使用证据仅见独立测试 `pkg/testkit/stepped_test.rs`；未发现其他 Rust 调用者。因此本文件目前应理解为已公开但接线范围有限的测试工具，而不能据此宣称 Go 侧所有并发回归场景已经迁移。

## 核心职责

本文件围绕两个公开类型分工：

- `StepController` 实现可跨线程克隆的命名断点协议。工作线程用 `checkpoint` 报告“第几次到达”并等待；控制线程用 `wait` 等待到达，用 `resume` 精确放行一次，用 `stop` 终止所有等待。
- `SteppedTestKit` 负责收集 `Step` 队列、创建后台 `TestKit`、按 FIFO 执行步骤、传播首个失败，并保存最近一次查询产生的 `Result`。

它解决的是确定性测试时序，而不是通用任务调度。所有步骤共享同一个后台 `TestKit`，因此同一队列中的 SQL 操作保留会话连续性；主线程则不直接持有该 `TestKit`，只通过断点、线程等待和结果快照观察执行。

## 主要符号

- `BreakpointState`（私有）：保存 `reached: HashMap<String, usize>`、`released: HashMap<String, usize>` 和全局 `stopped`。两个计数表让同名断点可重复出现，而不是一次性布尔事件。
- `StepController`（公开、`Clone + Default`）：内部是 `Arc<(Mutex<BreakpointState>, Condvar)>`，所有克隆共享同一状态和条件变量。
- `StepController::checkpoint(&self, name: &str) -> TestResult`：递增该名称的到达次数，通知观察者，并等待对应次数被放行；停止时返回 `TestError("stepped execution stopped")`。
- `StepController::wait(&self, name: &str, occurrence: usize)`：等待某名称至少到达指定次数。它不返回是否因 `stop` 提前退出，调用者需依赖整体生命周期判断。
- `StepController::resume(&self, name: &str)`：只允许放行已经到达但尚未放行的一次断点；提前放行、重复放行或停止后放行都会 panic。
- `StepController::stop(&self)`：设置停止标志并唤醒所有条件变量等待者。
- `Step`（私有类型别名）：`FnOnce(&mut TestKit, &StepController) -> TestResult<Option<Result>> + Send` 的装箱闭包。`None` 表示无查询结果，`Some(Result)` 更新结果槽。
- `SteppedTestKit`（公开）：持有数据库抽象、控制器、待执行队列、可选工作线程句柄和共享的最近查询结果。
- `SteppedTestKit::new`：构造尚未启动的实例；此时不创建会话或线程。
- `SteppedMustExec`、`SteppedMustQuery`、`Checkpoint`、`Custom`：只向 `steps` 追加闭包并返回 `&mut Self`，便于链式排队，不会立即执行。
- `Start`：搬走当前步骤队列，清空旧结果，并只启动一次后台线程。
- `WaitBreakpoint`、`Continue`：分别代理 `StepController::wait` 和 `resume`。
- `Wait`：取走并 join 工作线程，返回步骤错误或把线程 panic 映射成 `TestError`。
- `GetResult`、`GetQueryResult`：克隆最近查询结果；后者在尚无已完成查询时 panic。
- `Drop for SteppedTestKit`：先停止控制器，再回收仍存在的工作线程，避免线程永久卡在断点。
- `_keep_result_type`：私有、允许 dead code 的类型锚点；不参与执行流程。

本文件没有模块级常量、trait、条件编译项或异步运行时依赖。

## 执行流程

1. 测试以 `SteppedTestKit::new(database)` 创建空队列。`Database` 用 `Arc<dyn Database>` 保存，允许安全移入后台线程。
2. 测试调用 `SteppedMustExec`、`SteppedMustQuery`、`Checkpoint` 或 `Custom` 排队。SQL 文本和参数由闭包取得所有权，保证步骤满足 `'static + Send`。
3. `Start` 断言此前未启动，使用 `std::mem::take` 将当前队列移入线程，清空 `last_result`，再在线程内用同一 `Database` 构造一个 `TestKit`。
4. 工作线程对 `VecDeque` 执行 `pop_front`，严格按入队顺序调用每个 `FnOnce`。任一步返回错误时，`?` 立即结束线程，后续步骤不再执行。
5. 查询步骤经 `TestKit::Query` 获取行集，调用 `string_rows()` 后构造测试断言用的 `Result`；每个成功查询都会覆盖 `last_result`，最终保留最后一次成功查询结果。
6. 遇到 `Checkpoint(name)` 时，工作线程增加 `reached[name]` 并睡眠。主线程的 `WaitBreakpoint(name, occurrence)` 可等待观察到指定到达次数，随后 `Continue(name)` 增加一次 `released[name]` 并唤醒线程。
7. 主线程用 `Wait` join 并取得整体 `TestResult`；线程完成后仍可用 `GetResult`/`GetQueryResult` 读取共享结果。`pkg/testkit/stepped_test.rs::stepped_query_preserves_the_first_result_for_later_assertion` 覆盖了这一生命周期。

## 数据与状态

断点协议的核心不变量是：对每个名称始终有 `released <= reached`。`checkpoint` 先确定自己的 `occurrence = reached[name]`，再等待 `released[name] >= occurrence`；因此一次 `resume` 只对应一次实际到达，同名断点的第二次到达不会误用第一次的放行信号。`pkg/testkit/stepped_test.rs::continue_without_a_reached_breakpoint_fails_instead_of_pre_releasing_it` 和 `continue_releases_exactly_one_reached_breakpoint` 分别验证禁止预放行和一次一放行。

`steps` 在 `Start` 时整体移走，启动后再追加的步骤留在原对象中，当前工作线程不会执行它们；同时 `worker.is_none()` 的断言使同一实例不能再次 `Start`，所以这类后加步骤也没有第二次启动入口。安全扩展时应保持“启动前完成排队”的使用约束，或显式重新设计状态机。

`last_result` 是 `Arc<Mutex<Option<Result>>>`。执行步骤时只有查询返回 `Some(Result)` 才覆盖它；执行类、自定义和断点步骤不会清空已有查询结果。`Start` 会先清空上次内容。虽然目前单实例只允许启动一次，清空动作仍把启动边界定义清楚。

## 依赖与调用关系

上游装配关系为 `pkg/testkit/Cargo.toml` → `pkg/testkit/lib.rs::pub mod stepped` → 本文件公开 API。RustCodeGraph 将本文件索引为 228 行，并识别 `SteppedTestKit`、`StepController` 及完整源码；对关键方法执行 callers/callees 查询未返回方法级调用边，仓库文本检索补充确认直接 Rust 调用仅在 `pkg/testkit/stepped_test.rs`。

主要下游依赖均在 `astersql-testkit` crate 内或 Rust 标准库中：

- `crate::testkit::TestKit`：后台会话包装；`Start` 构造它，步骤调用其 `Exec`/`Query`。
- `crate::db_driver::{Database, DbValue}`：注入数据库实现并表示 SQL 参数；测试使用 `pkg/testkit/mockstore` 的 `MockStore` 实现该边界。
- `crate::result::Result`：把查询的字符串行集保存为可继续断言的结果。
- `crate::{TestError, TestResult}`：统一步骤、线程和停止错误。
- `std::thread`、`Arc`、`Mutex`、`Condvar`：提供线程所有权、共享状态和阻塞唤醒；不依赖 Tokio 或其他异步运行时。

RustCodeGraph 的文件级关系还报告 `pkg/ddl/testutil/operator.rs`、`pkg/server/tests/standby/standby_test.rs`、`pkg/session/txn.rs`、`pkg/testkit/mocksessionmanager.rs` 和 `pkg/testkit/stepped_test.rs` 使用本文件；逐符号文本核验只在最后一个文件找到 `SteppedTestKit`/`StepController` 调用，前四项应视为索引的文件级共享符号关系，而不是已确认的本 API 调用者。

## 错误处理与边界

- SQL 和自定义步骤返回 `TestResult`，工作线程用 `?` 保留第一个错误并中止后续步骤；`Wait` 将该结果交给测试线程。
- 工作线程自身 panic 时，`JoinHandle::join` 的错误被转换为 `TestError("stepped testkit worker panicked")`，原 panic 载荷不保留。
- 未调用 `Start` 就调用 `Wait` 返回 `TestError("stepped testkit not started")`；重复 `Start` 则通过断言 panic。
- `resume` 对未到达、已经完全放行或已停止的断点 panic，防止信号被“预存”后意外放行未来断点。
- `GetQueryResult` 在没有已完成查询结果时 panic；需要探测式访问时应使用返回 `Option<Result>` 的 `GetResult`。
- 三处互斥锁和条件变量操作都使用 `expect`；持锁线程 panic 导致 poison 后，后续访问也会 panic。这符合测试工具快速暴露错误的定位，但不适合作为容错生产同步原语。
- `wait(name, occurrence)` 在 `stopped` 时静默返回，且 `occurrence == 0` 会立即成功。调用者不能把其返回本身解释为断点必然到达；正常测试应使用从 1 开始的次数并结合 `Wait` 的最终结果。
- 没有内置超时。若测试既不 `Continue`、也不 drop 控制器，显式 `Wait` 会无限等待；Go 版本的十秒通道超时并未移植。

## 并发与资源生命周期

每个 `SteppedTestKit` 最多创建一个 OS 工作线程。步骤在该线程中串行运行，不并行修改后台 `TestKit`。主线程和工作线程共享的只有 `StepController` 状态与 `last_result`，二者都受互斥锁保护；`Condvar` 等待均置于 `while` 循环中，正确处理虚假唤醒。

`checkpoint` 在更新到达次数后调用 `notify_all`，让等待该名称/次数的主线程重新检查谓词；`resume` 和 `stop` 同样通知全部等待者。所有名称共用一个条件变量，简单可靠，但大量独立断点并发时会产生无关唤醒；当前测试工具规模下这是明确的性能取舍。

正常回收路径是 `Wait` 取走句柄并 join。若调用者忘记 `Wait`，`Drop` 会先 `stop`，使卡在 `checkpoint` 的步骤返回错误，再 join 工作线程。若工作线程卡在数据库调用或不响应控制器的自定义步骤中，`Drop` 仍可能阻塞；`stop` 只影响本模块的断点等待，不能取消任意 SQL/闭包。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/testkit/stepped.go`。两版共同目标是：在后台执行 SQL 命令，在确定的断点处把控制权交给测试线程，并保留查询结果。但具体协议存在重要差异：

- Go 的 `NewSteppedTestKit(t, store)` 内部长期持有一个 `TestKit`，每次 `SteppedMustExec`/`SteppedMustQuery` 立即用 goroutine 启动单个命令；Rust 先把多个 `Step` 排入 `VecDeque`，显式 `Start` 后由一个线程顺序执行。
- Go 用 `SetBreakPoints` 启用 `pkg/util/breakpoint` failpoint，把通知函数写入 session，并用 `ch1`/`ch2` 双通道在前后台握手；Rust 不启用 failpoint，而是要求队列显式插入 `Checkpoint`，或由 `Custom` 闭包直接调用 `StepController`。
- Go 的 `Continue()` 针对当前 `cmdStopAt`，并配有 `ExpectIdle`、`ExpectStopOnBreakPoint`、`ExpectStopOnAnyBreakPoint` 状态断言；Rust 的 `Continue(name)` 由调用者传名称，以 reached/released 次数校验是否合法，没有这些断言门面。
- Go 通道操作具有 `defaultChanTimeout` 十秒超时，并在 defer 中禁用 failpoint、清理 session 回调；Rust 条件变量没有超时，也没有 failpoint/session 清理，资源回收重点是 `stop + join`。
- Go 同一对象还支持同步 `MustExec`/`MustQuery`；Rust 文件只提供排队版本。Go 的调用证据包括 `pkg/sessiontxn/txn_context_test.go`、`pkg/executor/update_test.go` 和 `pkg/executor/statement_ru_plan_walk_integration_test.go`，这些场景目前不能仅凭本文件认定已迁移到 Rust。

因此 Rust 实现是面向相同测试意图的独立同步模型，而不是 Go API 的逐项等价复刻。扩展时应以 Go 行为作为兼容参照，同时为 Rust 的队列状态机补足独立测试。

## 扩展指南

- 新增一种预定义步骤时，优先在 `SteppedTestKit` 上增加只负责封装 `Step` 的排队方法，并保持 `TestResult<Option<Result>>` 约定；对应测试应放在独立的 `pkg/testkit/stepped_test.rs`，不要嵌回生产源文件。
- 若要接入真实执行路径中的自动断点，应先明确如何把 `StepController` 注入 session/执行器；可参考 Go 的 `breakpoint.NotifyBreakPointFuncKey` 和 failpoint 清理流程，但不能只复制 API 名称而省略启停、失败和析构语义。
- 若增加超时或取消，应同时覆盖 `checkpoint`、`wait`、`Wait` 与 `Drop`，并定义超时后是否允许继续使用对象。当前单一 `stopped` 标志是不可逆状态。
- 若允许动态追加步骤或多次 `Start`，必须重做 `worker` 与 `steps` 的状态机；当前实现会把启动时队列整体移走，重复启动被禁止。
- 若改变结果保留策略，要明确“最后一次成功查询”是否仍为不变量，并同步更新 `GetResult`/`GetQueryResult` 及 `stepped_query_preserves_the_first_result_for_later_assertion`。
- 兼容风险集中在 Go/Rust 时序语义差异、提前放行是否允许、同名断点次数、错误传播和超时；性能风险主要是每实例一个 OS 线程、全断点共用 `notify_all`，以及 `Drop` 的同步 join。

## 验证依据

- 源码：`pkg/testkit/stepped.rs`，核对了全部 228 行以及 `BreakpointState`、`StepController`、`Step`、`SteppedTestKit`、所有方法和 `Drop` 实现。
- crate 装配：`pkg/testkit/Cargo.toml` 的 `[lib]` 与依赖声明；`pkg/testkit/lib.rs` 的 `pub mod stepped` 和独立 `#[path = "stepped_test.rs"] mod stepped_test`。
- Rust 测试：`pkg/testkit/stepped_test.rs`，覆盖非法预放行、一次放行一次到达、后台查询结果在线程回收后仍可断言。
- Go 对照：`pkg/testkit/stepped.go`；Go 调用样本为 `pkg/sessiontxn/txn_context_test.go`、`pkg/executor/update_test.go`、`pkg/executor/statement_ru_plan_walk_integration_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`explore "pkg/testkit/stepped.rs SteppedTestSuite run_test_in_steps run_test_in_steps_with_breakpoints"` 返回目标文件源码；`node --file pkg/testkit/stepped.rs --offset 1 --limit 320` 返回完整 228 行并列出文件级关系；`query SteppedTestKit --kind struct` 和 `query StepController --kind struct` 定位 Rust/Go 符号。对 `SteppedTestKit::new`、`SteppedTestKit::Start`、`StepController::checkpoint` 执行 callers/callees 未得到方法级边，因此调用范围另以仓库文本检索核验，没有把缺失图边推断成调用事实。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令、链接/路径人工复核和工作区差异检查验收。
