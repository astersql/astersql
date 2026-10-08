# `pkg/util/intest/no_assert.rs`

## 文件定位

`no_assert.rs` 是 `astersql-util-intest` crate 的默认生产构建断言门面。crate 入口 `pkg/util/intest/lib.rs` 在未启用 `test`、`intest`、`enableassert` 任一条件时编译本模块，并将 `Assert`、`AssertNoError`、`AssertNotNil`、`AssertFunc` 与 `EnableAssert` 原名再导出。启用上述任一条件时，入口改为选择同签名的 `pkg/util/intest/assert.rs`，两个实现不会同时编译。

该文件存在的目的，是让生产默认构建保留稳定的内部断言 API，同时避免默认执行这些检查；运行期仍可通过共享的 `EnableInternalCheck` 开关显式启用检查。crate 的包名、入口和 feature 集合由 `pkg/util/intest/Cargo.toml` 定义：库入口是 `lib.rs`，`default` 为空，另有互相独立的 `intest` 与 `enableassert` feature。

## 核心职责

本文件只负责“是否进入断言实现”的门控，不负责判断失败条件、格式化消息或触发 panic：

1. 将本构建变体的 `EnableAssert` 初始化为 `false`。
2. 为四类断言提供与启用版一致的公开函数签名。
3. 每次调用都以 `Ordering::Relaxed` 读取 `EnableInternalCheck`；值为 `false` 时立即返回，值为 `true` 时把原参数交给 `assert_common.rs` 中对应的 `doAssert*`。

因此，“默认禁用”不等于 API 被删除，也不等于永远不能检查。它表示 `EnableAssert` 本身不会让默认变体执行断言，唯一的运行期入口是 `EnableInternalCheck`。

## 主要符号

- `pub static EnableAssert: AtomicBool = AtomicBool::new(false)`：标识当前为禁用断言变体。它被 `lib.rs` 再导出，调用方可读取或写入，但本文件的四个函数并不读取它；修改它不会开启本变体的断言。
- `pub fn Assert(cond: bool, msg_and_args: &[AssertArg])`：内部检查开启时调用 `doAssert`。`cond == false` 的失败处理位于 `assert_common.rs::doAssert`。
- `pub fn AssertNoError(err: Option<&dyn Display>, msg_and_args: &[AssertArg])`：内部检查开启时调用 `doAssertNoError`。Rust 用 `Option<&dyn Display>` 表示“无错误/有可显示错误”。
- `pub fn AssertNotNil<T>(obj: Option<T>, msg_and_args: &[AssertArg])`：内部检查开启时调用泛型 `doAssertNotNil`。Rust 用 `None` 表达 Go 的 nil；传入的 `Option<T>` 按值移动。
- `pub fn AssertFunc(fn_check: Option<fn() -> bool>, msg_and_args: &[AssertArg])`：内部检查开启时调用 `doAssertFunc`。接口只接受无捕获、可退化为函数指针的 `fn() -> bool`，不接受一般捕获闭包。

本文件没有类型、trait、`impl`、宏或自身的条件编译项；选择本文件的条件编译声明在 `lib.rs`。

## 执行流程

四个函数遵循相同流程：调用者经 `lib.rs` 的再导出进入 API；函数用 relaxed load 读取 `assert_common::EnableInternalCheck`；关闭时无条件返回，开启时调用对应的共同实现。

共同实现的后续行为可由 `pkg/util/intest/assert_common.rs` 核验：`doAssert` 检查布尔条件；`doAssertNoError` 在 `Some(err)` 时追加错误文本；`doAssertNotNil` 检查 `Option::is_some()`；`doAssertFunc` 先检查函数指针存在，再调用并检查返回值。失败最终经 `doPanic` 生成以 `assert failed` 开头的消息并 panic。

短路是本文件的重要行为：当 `EnableInternalCheck == false` 时，错误不会被格式化，`Option<T>` 不会被共同实现检查，传给 `AssertFunc` 的函数也不会被调用。因此默认路径除了原子读取、参数传递/销毁之外不产生断言副作用。

## 数据与状态

本文件自身持有一个进程级 `AtomicBool`：`EnableAssert`，初值恒为 `false`。真正参与门控的是定义在 `assert_common.rs` 的另一个进程级 `AtomicBool`：`EnableInternalCheck`；其初值由 `cfg!(any(test, feature = "intest", feature = "enableassert"))` 决定。在能选择 `no_assert.rs` 的默认配置中，这个表达式为 `false`，故两个开关初始均关闭。

`msg_and_args` 是借用切片，元素类型 `AssertArg` 在共同模块中定义，可保存字符串、有/无符号整数、浮点数和布尔值。当前文件不读取该切片。`AssertNoError` 借用错误对象；`AssertNotNil` 获取 `Option<T>` 的所有权；`AssertFunc` 复制可空函数指针。文件内没有缓存、集合、事务状态或持久化数据。

## 依赖与调用关系

上游装配关系是 `pkg/util/intest/lib.rs -> no_assert.rs`：`lib.rs` 通过 `#[cfg(not(any(test, feature = "intest", feature = "enableassert")))]` 声明模块并再导出五个公开符号。RustCodeGraph 的文件节点也记录 `no_assert.rs` 被 `lib.rs` 使用。

下游关系是四个门面函数分别委托 `assert_common::{doAssert, doAssertNoError, doAssertNotNil, doAssertFunc}`，并读取同模块的 `EnableInternalCheck`；标准库依赖只有 `std::fmt::Display` 与 `std::sync::atomic::{AtomicBool, Ordering}`。Cargo 清单没有普通依赖或开发依赖。

仓库中的实际 Rust 调用以 crate 别名 `intest` 为主。例如 `pkg/expression/scalar_function.rs` 调用 `AssertNotNil` 并读取 `EnableAssert`，`pkg/table/tblsession/table.rs` 调用 `Assert`，`pkg/util/rowcodec/decoder.rs` 调用 `Assert`。这些调用最终落到启用版还是本文件，取决于全局 Cargo feature 合并与当前编译目标；源码调用点本身不绑定某个变体。RustCodeGraph 对 `Assert*` 这类同名符号的精确 callers/callees 消歧不完整，因此调用点使用仓库 `rg` 搜索核验，不能把图中同名结果当作本文件独占调用边。

## 错误处理与边界

本文件没有 `Result` 返回值，也不吞并或转换错误。内部检查关闭时，即使 `cond` 为假、`err` 为 `Some`、`obj` 为 `None` 或 `fn_check` 为空/返回假，调用也正常返回。内部检查开启时，失败语义完全由共同实现决定并以 panic 报告；`AssertFunc` 所调用函数自身的 panic 不会被本层捕获。

需要注意三个边界：第一，写 `EnableAssert.store(true, ...)` 不会改变本文件行为，因为门面函数只读取 `EnableInternalCheck`；第二，`AssertNotNil(Some(value), ...)` 只判断 `Option` 外层，不能自动识别 `Some` 内部自定义的“空”语义；第三，`AssertNoError` 只要求错误实现 `Display`，不保留具体错误类型或错误链。

## 并发与资源生命周期

两个全局开关均为 `AtomicBool`，所以并发读写不会产生数据竞争。本文件使用 `Ordering::Relaxed`：它只保证开关值本身的原子性，不为调用者的其他内存读写建立 happens-before 顺序。该选择适合“是否执行诊断检查”的独立布尔门控；扩展时不应把开关读写当作发布其他状态的同步屏障。

函数不创建线程、任务、锁、通道、文件、网络连接或事务。开启检查后，临时格式化字符串与 panic payload 的生命周期由共同实现和 Rust 展开机制管理。现有 `assert_test.rs` 用 `Mutex` 串行化全局开关修改，这是测试隔离措施，不是本文件的运行期锁。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/intest/no_assert.go`。Go 用 `//go:build !intest && !enableassert` 选择默认变体，将 `EnableAssert` 设为 `false`，并且四个公开函数都只在 `EnableInternalCheck` 为真时调用同名共同实现；Rust 的 `lib.rs` cfg 与本文件逐项镜像这一结构。

语义上的类型映射为：Go `error` 对应 Rust `Option<&dyn Display>`，Go `any` 的 nil 检查对应 Rust `Option<T>`，Go `func() bool` 的 nil/调用检查对应 Rust `Option<fn() -> bool>`，Go `...any` 对应受限的 `&[AssertArg]`。因此 Rust 不是动态类型的一比一复刻：消息参数和可断言函数的类型范围更窄，但开关条件、短路行为和共同失败路径保持一致。

`pkg/util/intest/assert.go` / `assert.rs` 是互斥的启用版对照：它们额外读取初值为真的 `EnableAssert`。不要把启用版测试观察到的“修改 `EnableAssert` 可控制检查”误写为默认 `no_assert.rs` 的行为。

## 扩展指南

若新增一种断言 API，应同时更新 `no_assert.rs`、`assert.rs`、`assert_common.rs`、`lib.rs` 的再导出以及 Go 对照实现：默认版只用 `EnableInternalCheck` 门控，启用版保持 `EnableAssert || EnableInternalCheck` 门控，实际失败逻辑集中在共同模块。两变体的公开签名必须一致，否则相同调用方会随 feature 组合出现编译差异。

测试逻辑应继续放在独立文件，而不是嵌入生产源文件。共同失败语义和启用版开关行为应扩展 `pkg/util/intest/assert_test.rs` 与 `migration_aster_unit_test.rs`；默认变体的行为应增加或扩展独立集成测试，使库在非 `cfg(test)` 方式编译后验证：两开关默认关闭、写 `EnableAssert` 不触发检查、写 `EnableInternalCheck` 才触发检查，以及关闭时 `AssertFunc` 不调用函数。修改全局原子开关的测试必须保存并恢复旧值，并串行化并发测试以免互相污染。

兼容风险主要是 cfg 条件或门控开关漂移导致生产环境意外 panic；正确性风险是两变体签名/共同实现不一致；性能风险是默认热路径新增格式化、分配或强内存序。扩展时应保持关闭路径只有一次廉价原子读取，并确认 workspace feature 合并不会意外选择启用版。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含目标文件；`node --file pkg/util/intest/no_assert.rs` 核对 5 个公开符号和全部 62 行源码；文件节点报告唯一装配者为 `pkg/util/intest/lib.rs`。
- RustCodeGraph 源码节点：`pkg/util/intest/lib.rs` 核对互斥 cfg 与再导出；`assert_common.rs` 核对四条共同失败路径、`EnableInternalCheck` 初值和 panic/message 行为；`assert.rs` 核对启用版同签名 API 与双开关差异。
- crate 边界：`pkg/util/intest/Cargo.toml` 核对包名、`lib.rs` 入口、空默认 feature、`intest`/`enableassert` feature，以及无普通依赖。
- Go 对照：`pkg/util/intest/no_assert.go` 核对默认 build tag、`EnableAssert = false` 与仅由 `EnableInternalCheck` 门控；`assert.go` 核对启用版差异；`assert_test.go` 核对 nil、错误、函数和消息行为。
- Rust 测试：`pkg/util/intest/assert_test.rs` 和 `migration_aster_unit_test.rs` 覆盖共同断言语义、消息与开关切换；`not_in_unittest_test.rs` 以集成测试方式核对默认 feature 下公开全局状态可跨线程观察。由于 crate 单元测试的 `cfg(test)` 会选择 `assert.rs`，现有测试不直接编译并执行 `no_assert.rs` 的四个门面函数，这是当前验证边界而非已覆盖结论。
- 调用点补证：对 `*.rs` 搜索 `intest::Assert*`、`EnableAssert`、`EnableInternalCheck`，核对该 crate 在表达式、表、行编解码等模块的使用；RustCodeGraph 同名调用边无法稳定区分 `assert.rs` 与 `no_assert.rs`，故未声称这些调用点必然在所有构建中落到本文件。

