# `pkg/executor/staticrecordset/cursorrecordset.rs`

## 文件定位

本文件属于 `astersql-executor-staticrecordset` crate，是静态结果集与会话游标句柄之间的生命周期适配层。crate 入口 `pkg/executor/staticrecordset/lib.rs` 将 `cursorrecordset` 声明为公开模块并重新导出其符号；`pkg/executor/staticrecordset/Cargo.toml` 通过 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/executor/staticrecordset`。

它位于 SQL 执行结果向会话/协议层交付的边界：底层 `RecordSet` 负责字段、按 chunk 拉取和执行器关闭，游标句柄负责会话侧游标资源。包装后的对象仍以 `Box<dyn RecordSet>` 暴露，因此调用者不需要知道结果集是否绑定游标。

当前 Rust 仓库中 `WrapRecordSetWithCursor` 的直接使用仅出现在独立测试 `cursorrecordset_test.rs` 和 `integration_test.rs`，没有发现生产 Rust 调用点。已接入的完整生产主链仍可在 Go 版本 `pkg/session/session.go::execStmtResult.TryDetach` 中看到：成功分离结果集后创建 cursor，再调用同名包装函数。因而本文不会把 Go 的生产接线误写成 Rust 已完成接线。

## 核心职责

文件只承担三项职责，均由 `impl RecordSet for cursorRecordSet` 体现：

1. 将 `CursorHandle` 与一个既有 `RecordSet` 绑定到同一个所有权对象中。
2. 原样转发 `Fields`、`Next`、`NewChunk` 和测试执行器访问，不改变数据读取语义。
3. 在显式 `Close` 时严格先关闭 cursor，再关闭底层 record set，并只把底层 `RecordSet::Close` 的结果返回给调用方。

它不创建游标、不执行 SQL、不缓存行、不实现 chunk 分配策略，也没有 `Drop` 自动收尾。底层数据与执行器行为来自 `pkg/executor/staticrecordset/recordset.rs` 或其他 `RecordSet` 实现。

## 主要符号

- `pub trait CursorHandle: Send`：本文件对会话游标能力的最小抽象。唯一方法 `fn Close(&mut self)` 没有返回值，与 Go 的 `cursor.Handle.Close()` 契约一致。`Send` 允许句柄随包装对象跨线程转移，但没有声明 `Sync`。
- `pub struct cursorRecordSet`：私有包装类型，分别以 `Box<dyn CursorHandle>` 和 `Box<dyn RecordSet>` 独占 cursor 与结果集。字段不公开，外部只能通过构造函数和 `RecordSet` trait 操作。
- `RecordSet::Fields`：调用 `self.record_set.Fields()`，返回值和复制/克隆成本完全由底层实现决定。
- `RecordSet::Next`：把 `RecordContext` 与可变 `Chunk` 原样交给底层 `Next`，不捕获或转换错误。
- `RecordSet::NewChunk`：把可选 `ChunkAllocator` 原样交给底层实现，不在本层分配缓存。
- `RecordSet::Close`：先执行 `self.cursor.Close()`，再执行并返回 `self.record_set.Close()`。
- `RecordSet::GetExecutor4Test`：要求底层返回 `Some(&dyn Executor)`，否则以 `expect("wrapped record set does not expose GetExecutor4Test")` panic；成功时再包装为 `Some`。
- `pub fn WrapRecordSetWithCursor(...) -> Box<dyn RecordSet>`：公开构造入口，把两个 trait object 移入私有结构并擦除具体类型。

本文件没有模块级常量、枚举、泛型参数、异步函数或条件编译项。

## 执行流程

构造阶段由 `WrapRecordSetWithCursor` 接收已经存在的 cursor 和 record set，取得二者所有权，构造 `cursorRecordSet`，再作为统一的 `Box<dyn RecordSet>` 返回。

读取阶段完全采用装饰器式转发：调用者先通过 `Fields` 获得列元信息，通过 `NewChunk` 获得输出缓冲，再反复调用 `Next(ctx, req)`；包装层不检查关闭状态、不保存行数，也不修改上下文或 chunk。`integration_test.rs::cursor_recordset_forwards_fetch_and_closes_cursor_first` 验证了字段、chunk 行数和内部执行器暴露均沿此路径保留。

关闭阶段的顺序是本文件唯一新增的运行时语义：

1. `cursorRecordSet::Close` 无条件调用 `CursorHandle::Close`。
2. cursor 关闭完成后才调用底层 `RecordSet::Close`。
3. 返回底层关闭结果；cursor API 没有可传播的错误。

独立 Rust 集成测试记录的事件顺序为 `executor.next`、`cursor.close`、`executor.close`。Go 的 `execStmtResult.TryDetach` 则说明该包装发生在 detached record set 和 cursor 都创建成功之后；若后续 `finishStmt` 失败，Go 调用方走单独清理分支，而不是返回包装对象。

## 数据与状态

包装对象自身只有两个所有权字段，没有派生状态：`cursor` 是会话侧资源句柄，`record_set` 是查询结果及执行器资源的所有者。二者都采用 `Box<dyn ...>` 动态分派，从而允许不同游标和结果集实现组合。

本层不维护 `closed` 标志。每次调用包装层的 `Close` 都会再次调用 cursor 的 `Close`；底层是否幂等取决于具体 `RecordSet`。仓库的 `staticRecordSet::Close` 使用 `Option::take` 确保执行器只关闭一次，但该性质不能推广到任意传入的 `RecordSet`，cursor 是否支持重复关闭也由其实现负责。

同理，本层不拥有独立的字段副本、执行上下文或 chunk；这些状态都保留在底层结果集中。`GetExecutor4Test` 返回的引用生命周期受 `&self` 约束，且不会转移执行器所有权。

## 依赖与调用关系

直接依赖分为两类：

- 外部 crate `astersql_executor_internal_exec::executor::{Chunk, Executor}`：分别用于 `Next`/`NewChunk` 的缓冲类型和测试钩子的返回 trait。`Cargo.toml` 将其声明为唯一无条件依赖，路径为 `../internal/exec`。
- 同 crate 的 `recordset::{ChunkAllocator, RecordContext, RecordSet, Result, ResultField}`：定义包装器必须实现的统一结果集协议及其参数、返回类型。

`lib.rs` 通过 `pub use cursorrecordset::*` 导出 `CursorHandle` 和 `WrapRecordSetWithCursor`。RustCodeGraph 将目标文件标记为被 `pkg/executor/adapter_test.rs`、`cursorrecordset_test.rs`、`integration_test.rs` 使用；进一步的精确 callers/callees 查询没有返回调用边。仓库文本核验显示真正调用构造函数的 Rust 代码只有后两份测试，`adapter_test.rs` 并未出现该符号，因此不能依据文件级“used by”关系声称存在生产调用。

Go 生产对照中，上游为 `pkg/session/session.go::execStmtResult.TryDetach`，其先调用 `GetCursorTracker().NewCursor`，再调用 `staticrecordset.WrapRecordSetWithCursor(cursorHandle, detachedRS)`；下游则是 `cursor.Handle.Close` 与 `sqlexec.RecordSet` 的各方法。Rust 当前尚没有与该 Go 上游等价的生产接线证据。

## 错误处理与边界

`Fields` 和 `NewChunk` 没有错误通道；`Next` 原样返回底层 `Result<()>`；`Close` 只返回底层结果集的关闭错误。`integration_test.rs::cursor_recordset_returns_wrapped_close_error` 验证 `Error::Other("close failed")` 不被吞掉或改写。

cursor 的 `Close` 没有返回值，所以包装层无法报告游标关闭失败，也没有重试或日志逻辑。关闭顺序还有一个明确边界：如果某个 `CursorHandle::Close` 实现发生 panic，底层 `record_set.Close()` 不会执行，因为本文件没有 `catch_unwind`。相反，底层关闭返回普通错误时，cursor 已经先完成关闭。

`GetExecutor4Test` 是刻意严格的测试接口：如果被包装的实现沿用 `RecordSet` 默认值 `None`，该方法会 panic。`cursorrecordset_test.rs::get_executor_for_test_panics_when_wrapped_recordset_has_no_executor` 固化了这一行为，扩展时不应擅自改成静默返回 `None`。

对象离开作用域时 Rust 会释放两个 `Box`，但本文件没有 `Drop` 实现，因此“析构”不等同于调用业务层 `Close`。需要资源关闭语义的调用者必须显式调用 `RecordSet::Close`。

## 并发与资源生命周期

`CursorHandle: Send` 且 `RecordSet: Send`，所以 `cursorRecordSet` 可以被转移到另一个线程执行；两项 trait 都不要求 `Sync`，本文件也没有锁或内部同步，不能据此支持多个线程同时共享调用。所有会改变读取或关闭状态的方法都要求 `&mut self`，由 Rust 借用规则保证同一时刻只有一个可变访问者。

生命周期从 `WrapRecordSetWithCursor` 取得所有权开始，到显式 `Close` 和最终析构结束。正确资源顺序是先释放会话 cursor，再释放底层执行器/结果集，这一点既由实现行序保证，也由 `cursor_recordset_forwards_fetch_and_closes_cursor_first` 的事件断言覆盖。

Go 集成测试 `TestCursorWillBeClosed` 验证关闭 detached record set 后 cursor tracker 中不再有该游标；`TestCursorWillBlockMinStartTS` 进一步表明打开的 cursor 会维持较早的 `StartTS`，关闭后才允许最小时间戳推进。这是游标及时关闭的重要系统级影响，但它是 Go 生产链的证据，Rust 测试目前只验证本包装器的调用顺序。`TestFinishStmtError` 还验证 Go 上游在包装后的收尾失败路径上主动清理 cursor 和 detached record set。

## 与 Go 版本的对应关系

Rust 文件逐项复刻 `pkg/executor/staticrecordset/cursorrecordset.go`：私有 `cursorRecordSet` 同样持有 cursor 和 record set；`Fields`、`Next`、`NewChunk` 均直接转发；`Close` 都先关 cursor 再返回底层关闭错误；`WrapRecordSetWithCursor` 都返回统一 `RecordSet` 接口。

类型层面的差异主要来自语言：Go 使用 `cursor.Handle`、`sqlexec.RecordSet` 接口和 `context.Context`，Rust 在本 crate 定义最小 `CursorHandle` trait，并使用 `Box<dyn RecordSet>`、`&RecordContext` 与 `&mut Chunk`。Go 的 `GetExecutor4Test` 通过类型断言调用额外接口，底层不支持时同样会 panic；Rust 用 `Option` 加 `expect` 明确表达该前置条件。

Go 文件用编译期断言 `var _ sqlexec.RecordSet = &cursorRecordSet{}` 检查接口实现；Rust 的 `impl RecordSet for cursorRecordSet` 已由类型系统直接检查。Go 生产入口已经接到 `session.go::execStmtResult.TryDetach`，Rust 侧目前只有测试调用，因此实现语义对齐不代表应用主链接线已经对齐。

## 扩展指南

- 若增加一个新的结果集操作，先在 `recordset.rs::RecordSet` 定义契约，再在 `cursorRecordSet` 中保持透明转发，并同步 Go 的 `cursorrecordset.go`；测试应放在独立的 `cursorrecordset_test.rs` 或 `integration_test.rs`，不要内嵌到生产源文件。
- 若修改关闭逻辑，必须保留“cursor 先于 record set”以及“底层错误原样返回”两项兼容契约，并扩展事件顺序和错误传播测试。若希望支持 cursor 关闭错误，需要同时改变 `CursorHandle` 与 Go 对照 API，属于跨层契约变更，不能只改本文件。
- 若要保证重复 `Close` 不会重复触发 cursor，需要在包装层新增明确状态并决定首次/后续错误语义；当前实现没有这一保证，不能仅依赖 `staticRecordSet` 的幂等性。
- 若增加自动析构关闭，需评估 `Drop` 无法返回错误、panic 安全以及显式 `Close` 后重复调用的影响；性能上还要避免在 `Fields`/`Next` 热路径引入复制、锁或额外分配。
- 若将其接入 Rust 会话主链，应以 Go 的 `execStmtResult.TryDetach` 为行为基线，覆盖成功返回、`finishStmt` 失败后的双资源清理、cursor 对最小 `StartTS` 的约束，并确认 crate 依赖方向不会形成环。
- `GetExecutor4Test` 只用于测试诊断；扩展生产能力时不要借此绕过 `RecordSet` 抽象。若改动其 panic 契约，需同步 `cursorrecordset_test.rs`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件，目标文件在索引中。
- RustCodeGraph `files --filter pkg/executor/staticrecordset`：确认同目录 Rust/Go 源、测试和模块入口。
- RustCodeGraph `node --file pkg/executor/staticrecordset/cursorrecordset.rs --offset 1 --limit 260`：读取全部 74 行，核对 `CursorHandle`、`cursorRecordSet`、六个 trait/构造方法及无条件编译事实。
- RustCodeGraph `query CursorRecordSet --limit 20 --json`：核对 Rust/Go 同名类型及方法；对 `WrapRecordSetWithCursor`、`Close`、`Next` 执行精确 callers/callees 查询，未获得符号级调用边。
- RustCodeGraph `node`：读取 `cursorrecordset_test.rs`、`integration_test.rs`、`recordset.rs`，核对测试钩子 panic、转发、关闭顺序、错误传播以及底层 `RecordSet` 契约。
- 文件核验：`pkg/executor/staticrecordset/Cargo.toml`、`lib.rs`、`cursorrecordset.go`、`integration_test.go`、`pkg/session/session.go`；目标包不存在 `doc.go`。
- 文本搜索：Rust 中构造函数只有三处测试调用；Go 中生产调用位于 `pkg/session/session.go:3245`。这支持“Rust 尚无生产接线”的限制说明。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核唯一新增产物、源码链接、事实与推断边界。
