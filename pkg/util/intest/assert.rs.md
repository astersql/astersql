# `pkg/util/intest/assert.rs`

## 文件定位

本文件是 `astersql-util-intest` crate 的“启用断言”实现，源码入口为 [`assert.rs`](assert.rs)。它不是独立业务子系统，而是供数据库内部代码表达不可违背前置条件和状态不变量的轻量门面。crate 根 [`lib.rs`](lib.rs) 在 `cfg(test)`、feature `intest` 或 feature `enableassert` 任一成立时编译并再导出本模块；默认生产构建则选择 [`no_assert.rs`](no_assert.rs)。[`Cargo.toml`](Cargo.toml) 声明了这两个空 feature，且未声明外部依赖。

该文件位于 SQL 主链之外，但其公开 API 被多个子系统横向使用。例如 [`pkg/expression/scalar_function.rs`](../../expression/scalar_function.rs) 用 `AssertNotNil` 保护求值上下文，[`pkg/util/rowcodec/decoder.rs`](../rowcodec/decoder.rs) 用 `Assert` 检查解码不变量，[`pkg/table/tblctx/buffers.rs`](../../table/tblctx/buffers.rs) 用 `AssertNotNil` 检查语句缓冲区。因此它的作用是尽早暴露内部编程错误，而不是把用户输入错误转换成可恢复的 SQL 错误。

## 核心职责

1. 通过全局原子开关 `EnableAssert` 表示当前编译变体默认启用断言。
2. 为 `Assert`、`AssertNoError`、`AssertNotNil`、`AssertFunc` 提供统一的公开入口。
3. 每次调用都读取 `EnableAssert` 与公共开关 `EnableInternalCheck`；只要任一为真，就把检查委托给 [`assert_common.rs`](assert_common.rs) 中对应的 `doAssert*` 实现。
4. 在两个开关都为假时直接返回，不求值 `AssertFunc` 收到的函数，也不触发失败消息构造。

本文件刻意不负责 panic 文本格式化、`AssertArg` 类型转换或具体失败条件；这些职责集中在 `assert_common.rs`，让启用版和禁用版共享完全相同的检查语义。

## 主要符号

- `pub static EnableAssert: AtomicBool`：启用变体的断言开关，初始值为 `true`。调用方和测试可以在运行时用原子操作切换它。
- `pub fn Assert(cond: bool, msg_and_args: &[AssertArg])`：检查布尔条件。门控通过后调用 `doAssert`；条件为假时由公共实现 panic。
- `pub fn AssertNoError(err: Option<&dyn Display>, msg_and_args: &[AssertArg])`：以 `None` 表示无错误，以 `Some(&dyn Display)` 表示错误；后者会由 `doAssertNoError` 生成包含错误文本的 panic。
- `pub fn AssertNotNil<T>(obj: Option<T>, msg_and_args: &[AssertArg])`：以 `Some`/`None` 表达非空/空值，泛型值只为判空而被消费，不要求 `T` 实现额外 trait。
- `pub fn AssertFunc(fn_check: Option<fn() -> bool>, msg_and_args: &[AssertArg])`：接受可空的无捕获函数指针；公共实现先验证函数存在，再调用并验证返回值。

上述函数与静态量均使用 Go 风格名称；[`lib.rs`](lib.rs) 通过 crate 级 `allow(non_snake_case, non_upper_case_globals)` 明确允许这一命名，以保持迁移 API 的对应关系。

## 执行流程

四个公开函数遵循同一条短路径：

1. 分别以 `Ordering::Relaxed` 读取 `EnableAssert` 和 `EnableInternalCheck`。
2. 对两个结果执行逻辑或。若均为假，函数不再访问参数并正常返回。
3. 若任一为真，调用同名语义的公共实现：`Assert -> doAssert`、`AssertNoError -> doAssertNoError`、`AssertNotNil -> doAssertNotNil`、`AssertFunc -> doAssertFunc`。
4. 检查成功时公共实现返回；检查失败时，经 `doPanic` 和 `assertionFailedMsg` 构造以 `assert failed` 开头的消息并 panic。

`AssertFunc` 有额外顺序保证：`doAssertFunc` 先拒绝 `None`，再调用函数。函数返回 `false` 时按断言失败处理；函数自身 panic 时没有捕获层，原 panic 原样向上传播。独立测试 [`assert_test.rs`](assert_test.rs) 的 `test_assert_func` 覆盖了这三条失败路径。

## 数据与状态

本文件唯一自有状态是进程内全局 `EnableAssert: AtomicBool`。它在启用模块中初始为 `true`，没有持久化、会话隔离或租户隔离；修改会影响同一进程内之后的所有调用。另一个参与门控的 `EnableInternalCheck` 定义在 [`assert_common.rs`](assert_common.rs)，其初始值由 `cfg!(test | intest | enableassert)` 决定。

消息参数以借用切片 `&[AssertArg]` 传入。`AssertArg` 由公共模块定义，仅承载字符串、整数、无符号整数、浮点数和布尔值，并为 `%s`、`%d`、`%v`、`%+v` 的 Go 风格格式化提供数据。门面不会复制消息切片；只有失败路径才由公共实现创建最终 `String`。

`AssertNotNil` 会取得 `Option<T>` 的所有权。对于资源型 `T`，检查结束时值会正常析构；该 API 不返回原对象，因此需要继续使用对象的调用方通常应传入 `Some(&value)`，如 `buffers.rs` 所示。

## 依赖与调用关系

直接标准库依赖只有 `std::fmt::Display` 以及 `std::sync::atomic::{AtomicBool, Ordering}`。直接 crate 内依赖为 `assert_common::{AssertArg, EnableInternalCheck, doAssert, doAssertFunc, doAssertNoError, doAssertNotNil}`。

RustCodeGraph 对目标文件的索引显示其由 `lib.rs`、`assert_test.rs`、`migration_aster_unit_test.rs`、`pkg/expression/scalar_function.rs`、`pkg/table/tblctx/buffers.rs` 等文件使用。调用图聚合结果还确认：

- `Assert` 的上游包含 rowcodec 解码、SEM 兼容判断、表达式哈希检查及本 crate 测试；仓库搜索另见 table session、ranger、chunk、memory tracker 等调用点。
- `AssertNotNil` 的主要生产调用集中在 `scalar_function.rs` 的标量/向量求值入口，并由 `buffers.rs` 使用。
- `AssertNoError` 与 `AssertFunc` 当前 Rust 调用边主要来自 `assert_test.rs` 和 `migration_aster_unit_test.rs`；不能据此推断未来生产调用者不存在。

下游边固定指向公共 `doAssert*`。panic 消息的进一步下游为 `doPanic -> assertionFailedMsg -> sprintf`，均位于 `assert_common.rs`。本文件不依赖网络、存储、SQL 上下文或异步运行时。

## 错误处理与边界

这里的失败契约是 panic，不是 `Result`。因此只应检查代表程序缺陷的内部不变量，不应直接承接可预期的用户错误、I/O 错误或兼容性分支。关闭两个开关会跳过所有检查，这也意味着调用方的正确性不能依赖断言产生业务副作用。

关键边界如下：

- `Assert(false, ...)` 在门控开启时 panic；`Assert(true, ...)` 不构造失败消息。
- `AssertNoError(None, ...)` 通过；`Some(err)` 的额外文本为 `error is not nil: {err}`，只要求错误实现 `Display`。
- `AssertNotNil(None, ...)` panic，任何 `Some` 均通过，包括 `Some(false)`、`Some(0)` 和持有空内容的值。
- `AssertFunc(None, ...)` 在调用前 panic；`Some(fn)` 返回假则断言失败；被调函数自身的 panic 不会被转换。
- `AssertFunc` 的参数类型是 `fn() -> bool`，不接受捕获环境的闭包；若扩展为泛型闭包，需要同时评估 API 兼容性与禁用路径是否仍保证不执行回调。
- 原子读取使用 `Relaxed`，只保证开关值读写本身无数据竞争，不提供其他内存的发布/获取顺序。

## 并发与资源生命周期

两个开关都是 `AtomicBool`，因此并发读写不会造成 Rust 数据竞争。`Relaxed` 足以支持“是否执行诊断检查”的独立标志，但调用切换开关时，各线程可能在不同调用时刻观察到不同值；代码没有承诺跨线程同步切换瞬间，也没有把开关用作其他状态的同步屏障。

公开函数不创建线程、任务、通道、锁、事务或文件句柄。成功路径的资源生命周期止于一次同步函数调用；失败路径通过栈展开传播 panic。测试 [`assert_test.rs`](assert_test.rs) 使用进程级 `TEST_LOCK: Mutex<()>` 串行修改全局开关，说明测试若并行切换这两个原子量会彼此干扰。生产扩展若需要临时切换开关，应提供可恢复旧值的作用域机制并考虑 panic 清理，而不是依赖裸 `store` 成对出现。

## 与 Go 版本的对应关系

直接对照文件是 [`assert.go`](assert.go)。Go 的 `//go:build intest || enableassert` 对应 Rust `lib.rs` 中的 `cfg(any(test, feature = "intest", feature = "enableassert"))`；Rust 额外把单元测试构建纳入启用变体。两端都定义默认为真的 `EnableAssert`，都以 `EnableAssert || EnableInternalCheck` 门控四个 API，并把实际工作委托给公共实现。

API 形状因语言类型系统而不同：

- Go 的变参 `...any` 在 Rust 中变为 `&[AssertArg]`，因此 Rust 当前只覆盖 `AssertArg` 枚举支持的基础消息类型。
- Go 的 `error` 在 Rust 中变为 `Option<&dyn Display>`。
- Go 的 `any` 加反射可识别 nil interface 和 typed nil 指针；Rust 用 `Option<T>` 在类型层显式表达空值。调用方必须把可能为空的值建模为 `Option`，不存在对任意 `T` 的反射判空。
- Go 的 `func() bool` 可为 nil；Rust 对应 `Option<fn() -> bool>`，但 Rust 函数指针不能捕获环境。
- Go 公共实现可通过 failpoint 在初始化时同时打开两个开关；当前 Rust `assert_common.rs` 没有对应的运行时 failpoint 注入，只按 cfg 初始化 `EnableInternalCheck`。这是当前实现差异，不应在本文档中描述为已支持。

Go 测试 [`assert_test.go`](assert_test.go) 与 Rust [`assert_test.rs`](assert_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 共同锚定真假条件、nil、错误文本、函数 panic 透传和消息格式。Go 还显式覆盖 typed nil 指针；Rust 的等价边界由 `None::<Box<Foo>>` 表达。

## 扩展指南

新增断言种类时，应保持门面与公共实现分离：在本文件增加同形公开入口，在 [`no_assert.rs`](no_assert.rs) 增加 API 对称项，在 [`assert_common.rs`](assert_common.rs) 实现真实检查，并在 [`lib.rs`](lib.rs) 的两个再导出分支同步符号。测试逻辑必须继续放在独立文件中，优先扩展 [`assert_test.rs`](assert_test.rs)；涉及 Go 迁移契约时同步扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 并核对 Go 的 `assert.go`、`assert_common.go`、`assert_test.go`。

修改开关语义时应特别检查：启用版与禁用版是否仍有一致公开 API；两个开关的逻辑或是否保持；关闭时回调和消息格式化是否完全不执行；并发切换是否需要强于 `Relaxed` 的顺序。修改消息参数时，应同步评估 `AssertArg` 的转换、`Display`、格式动词兼容性和失败路径分配成本。

安全扩展不应把可恢复错误改成 panic，也不应让调用方依赖断言副作用。若要支持捕获闭包、任意错误类型或更广的 Go `fmt` 语义，应先明确破坏性 API 影响，再用独立回归测试覆盖启用、禁用、双开关和被调函数 panic 四类边界。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/util/intest`：确认本 crate 的 Rust/Go 源文件、独立测试和模块入口均已索引。
- RustCodeGraph `node --file pkg/util/intest/assert.rs`：核对 `EnableAssert`、四个公开函数、双原子开关和全部直接下游调用。
- RustCodeGraph `explore "pkg/util/intest/assert.rs symbols callers callees"`：核对 `Assert`、`AssertNoError`、`AssertNotNil`、`AssertFunc` 的调用者集合；精确 `callers/callees` 子命令在本次 30 秒执行窗口内未返回结果，因此调用边又用已索引文件摘要及仓库定点搜索交叉验证。
- 已读 Rust 证据：[`assert.rs`](assert.rs)、[`assert_common.rs`](assert_common.rs)、[`lib.rs`](lib.rs)、[`assert_test.rs`](assert_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 已读边界与 Go 对照：[`Cargo.toml`](Cargo.toml)、[`assert.go`](assert.go)、[`assert_common.go`](assert_common.go)、[`assert_test.go`](assert_test.go)。
- 人工复核的核心不变量：启用模块默认打开 `EnableAssert`；任一开关开启即执行公共检查；双开关关闭即跳过；失败统一 panic；本文件没有异步、I/O、锁或事务生命周期。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验证使用任务指定命令，结果见交付报告。
