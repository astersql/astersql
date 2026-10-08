# `pkg/testkit/testmain/wrapper.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-testkit-testmain`，包边界由 `pkg/testkit/testmain/Cargo.toml` 声明，crate 根是同目录的 `lib.rs`。`lib.rs` 将 `wrapper` 声明为公开模块并以 `pub use wrapper::*` 再导出，因此调用方通常直接从 `astersql_testkit_testmain` 导入 `TestingM` 和 `WrapTestingM`，而不必写出 `wrapper` 模块路径。

它位于测试基础设施而非数据库 SQL 运行主链：作用是把“运行整套测试并产生进程退出码”的能力抽象出来，并允许在底层 runner 完成之后执行一次退出码回调。直接的 Rust 使用证据包括 `pkg/session/test/main_test.rs`、`pkg/session/test/variable/main_test.rs`、`pkg/session/test/bootstraptest/lib.rs` 与 `pkg/session/test/bootstraptest2/main_test.rs`；同 crate 的 `bench.rs` 也复用 `TestingM` 驱动 benchmark 短路路径。

## 核心职责

- `TestingM` 把测试 runner 缩减为单一的 `run(&self) -> i32` 契约，使包装逻辑不依赖某个具体测试框架类型。
- `WrapTestingM` 保存一个底层 runner 和一个可变回调。每次调用包装器的 `run` 时，先且仅先调用一次底层 runner，再把其退出码传给回调，并返回回调结果。
- 当调用者不提供回调时，`WrapTestingM::new` 安装恒等函数，保证退出码原样透传。
- 同名自由函数 `WrapTestingM(...)` 保留 Go 风格入口，内部只转发到构造器，便于迁移代码与 Go `testmain.WrapTestingM` 保持名称和调用形态接近。

本文件不负责启动线程、退出进程、检测 goroutine 泄漏或解析 benchmark 参数。进程退出与 benchmark 参数处理位于相邻的 `bench.rs`；Go 侧的泄漏检查由外部 `goleak.VerifyTestMain` 完成。

## 主要符号

- `pub trait TestingM { fn run(&self) -> i32; }`：公开的最小 runner 接口。退出码语义沿用测试进程约定，通常 `0` 表示成功，但 trait 本身不校验数值范围。
- `impl<T: TestingM + ?Sized> TestingM for &T`：为共享引用提供转发实现。`?Sized` 允许 `&dyn TestingM` 等动态大小 trait 对象参与调用；它也是测试中把 `&runner` 直接交给包装器的基础。
- `pub struct WrapTestingM<'a, M>`：公开包装类型，按值拥有泛型 runner `testing_m: M`，并持有 `RefCell<Box<dyn FnMut(i32) -> i32 + 'a>>`。生命周期 `'a` 允许回调借用调用者作用域内的状态，不强制要求 `'static`。
- `WrapTestingM::new`：公开构造器。`Some(callback)` 原样保存；`None` 转成 `|exit_code| exit_code`。
- `impl<M: TestingM> TestingM for WrapTestingM<'_, M>`：核心行为实现。表达式先求值 `self.testing_m.run()`，再对回调取得可变借用并调用，返回回调产生的退出码。
- `pub fn WrapTestingM(...) -> WrapTestingM<'a, M>`：带 `#[allow(non_snake_case)]` 的 Go 风格工厂函数，与同名类型处于不同命名空间；行为等价于 `WrapTestingM::new`。

文件没有模块级常量、枚举、条件编译项或私有辅助函数。

## 执行流程

1. 调用方实现 `TestingM`，让 `run` 执行测试套件或返回代表套件结果的退出码。
2. 调用方通过 `WrapTestingM::new` 或自由函数 `WrapTestingM` 传入 runner 与可选的 `FnMut(i32) -> i32` 回调。
3. 构造阶段若回调为 `None`，安装恒等回调；否则保留原回调及其借用生命周期。
4. 调用包装器的 `run(&self)` 时，先执行 `self.testing_m.run()`。底层 runner 的返回值成为回调的唯一参数。
5. `RefCell::borrow_mut` 在运行时取得回调的独占可变借用，随后调用 `FnMut`。这样即使外部接口只有 `&self`，回调仍可更新自身捕获状态。
6. 包装器把回调返回的 `i32` 直接交还调用者，不再解释、规范化或覆盖退出码。

`pkg/testkit/testmain/wrapper_test.rs::callback_can_mutate_captured_state_across_runs` 证明同一包装器可多次运行：底层 runner 每次恰好执行一次，回调捕获的计数状态跨调用保留，退出码依次从 `7` 变为 `8`、`9`。`migration_aster_unit_test.rs` 另行验证 `None` 透传以及自定义回调发生在底层 `run` 之后。

## 数据与状态

包装器只有两份状态：按值存储的 `testing_m`，以及堆分配、类型擦除后的回调。泛型 `M` 可以是拥有型 runner，也可以借助 `TestingM for &T` 实现保存共享引用，因此是否拥有底层资源取决于调用者传入的具体类型。

回调使用 `Box<dyn FnMut...>`，可在运行时接纳不同闭包类型，并允许闭包修改捕获状态。`RefCell` 提供内部可变性，把 Rust 的可变借用检查从编译期延后到调用时；这正是 `TestingM::run` 只接收 `&self` 而回调需要 `FnMut` 的桥接点。文件中没有全局变量、缓存、锁、通道或事务状态。

退出码只是 `i32` 值：本文件不假定只有 `0/1`，也不保证回调保留原值。调用方可以原样返回、调整或完全替换底层退出码，因此“清理之后仍保留失败状态”是回调自身必须维持的契约。

## 依赖与调用关系

直接标准库依赖只有 `std::cell::RefCell`；`Cargo.toml` 没有为该 crate 声明普通外部依赖或 feature。`lib.rs` 负责公开模块和再导出，根 workspace 的 `Cargo.toml` 将 `pkg/testkit/testmain` 纳入 workspace，并提供 `facade_testkit_testmain` 路径别名。

主要调用边为：

- 上游构造：`pkg/session/test/main_test.rs` 和 `pkg/session/test/variable/main_test.rs` 用自由函数 `WrapTestingM` 包装固定退出码 runner，并在回调中模拟等待异步清理；`pkg/session/test/bootstraptest/lib.rs::wrap_bootstraptest_runner` 将该包装进一步封装为 bootstrap 测试 harness。
- 下游执行：`WrapTestingM::run` 调用 `M::run`，随后调用保存的 `FnMut`；自由函数 `WrapTestingM` 调用 `WrapTestingM::new`。
- 同 crate 消费：`pkg/testkit/testmain/bench.rs::benchmark_exit_code` 接收 `&dyn TestingM`，检测到非空 benchmark 过滤条件时调用 `testing_m.run()`；`ShortCircuitForBench` 再把该返回值交给 `std::process::exit`。
- Go 生产对照：多个 Go `TestMain`（例如 `pkg/session/main_test.go` 与 `pkg/statistics/main_test.go`）把 `testmain.WrapTestingM(...)` 交给 `goleak.VerifyTestMain`，回调用于等待清理或生成测试数据，然后保留原退出码。

RustCodeGraph 的文件节点显示 `wrapper.rs` 被 5 个 Rust 文件使用：`pkg/session/test/bootstraptest/lib.rs`、`pkg/session/test/bootstraptest/main_test.rs`、`pkg/session/test/bootstraptest2/main_test.rs`、`pkg/session/test/main_test.rs`、`pkg/session/test/variable/main_test.rs`。同名 `run` 在仓库中高度重载，图查询无法可靠消歧到该 trait 方法，因此上述边同时用这些源文件中的显式导入和调用进行了核验。

## 错误处理与边界

API 不使用 `Result`，也没有自行捕获 panic：底层 runner、回调或回调中的清理逻辑若 panic，panic 会直接向上传播，包装器不会生成替代退出码。回调返回任何 `i32` 都会被接受。

`RefCell` 的运行时借用规则是重要边界：正常顺序调用每次只持有一个可变借用；若未来通过间接结构在回调尚未返回时重入同一包装器并再次借用同一个回调，会触发运行时 panic。扩展时不应把该类型描述为可重入包装器。

默认回调只在参数为 `None` 时安装；不存在“回调失败后回退到原退出码”的分支。底层 `run` 先执行，因此若它 panic，回调不会运行。这与当前 Go 表达式 `m.callback(m.TestingM.Run())` 的求值顺序一致。

## 并发与资源生命周期

本文件不创建线程或异步任务。`RefCell` 不是线程同步原语，`WrapTestingM` 因其回调字段通常不具备 `Sync`，设计用途是单线程测试入口顺序执行，而不是多个线程同时调用 `run`。若未来确需并发共享，应先明确语义并采用锁等同步机制，不能仅添加 `Send`/`Sync` 约束掩盖运行时借用问题。

包装器按值拥有 `M` 和回调：构造后两者共同存活，包装器销毁时按 Rust 正常析构释放。生命周期 `'a` 保证被回调借用的外部状态至少活到包装器不再使用；`wrapper_test.rs` 通过内层作用域先销毁包装器，随后读取 `callback_runs`，展示了这一借用边界。文件本身没有显式 `Drop`、文件句柄、网络连接或后台任务需要清理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/testkit/testmain/wrapper.go`。两侧的核心顺序相同：保存一个实现 runner 接口的对象；调用其 `Run/run`；把得到的整数交给回调；返回回调结果；空回调被替换为恒等函数。

类型映射如下：Go 的 `goleak.TestingM` 对应 Rust 本地 trait `TestingM`；Go 私有结构体 `testingM` 对应 Rust 公开泛型结构体 `WrapTestingM<'a, M>`；Go `func(int) int` 对应 Rust `Box<dyn FnMut(i32) -> i32 + 'a>`；Go 的 `nil` 对应 Rust 的 `None`。

两侧也存在明确差异：

- Go 包装器嵌入 `goleak.TestingM` 并返回私有指针类型，面向 `goleak.VerifyTestMain`；Rust crate 没有 `goleak` 依赖，而是定义最小本地 trait，以适配 Rust 测试 harness 和相邻 benchmark 逻辑。
- Rust 类型按值泛化 `M`，同时通过 `TestingM for &T` 支持借用 runner；Go 通过接口值保存底层对象。
- Rust 回调是 `FnMut` 并由 `RefCell` 管理内部可变性，可安全表达带借用生命周期的可变捕获；Go 闭包的可变捕获无需显式容器。
- 当前 Rust 使用点主要是可执行的迁移/契约测试，并不等同于 Go `testing.M` 的进程级 `TestMain` 接线。文档不据此声称 Rust 已实现 goroutine 泄漏检查。

## 扩展指南

- 若要改变包装顺序、退出码变换或默认行为，优先修改 `WrapTestingM::new` 和 `impl TestingM for WrapTestingM`，并同步扩充独立测试 `pkg/testkit/testmain/wrapper_test.rs`；不要把测试代码内嵌回 `wrapper.rs`。
- 若新增 runner 适配方式，应保持 `TestingM` 的最小接口，先检查 `bench.rs::benchmark_exit_code` 及所有 crate 根再导出使用者，避免只满足 wrapper 而破坏 benchmark 的 trait 对象调用。
- 若需要错误返回，应评估将 `i32` 契约改成 `Result` 对现有所有实现者和 Go 语义的兼容影响；这不是局部无损修改。
- 若需要并发或重入回调，应明确锁粒度、panic/poisoning 行为与回调执行次数，并新增独立并发测试。当前 `RefCell` 方案只承诺顺序、非重入调用。
- 若加入“始终执行”的清理语义，应注意当前回调只在底层 `run` 正常返回后运行；要覆盖 panic 类似 finally/defer 的需求，需要单独设计 unwind 边界，不能从现有实现推断已经支持。
- 保持 Go 对照时，应核对 `wrapper.go` 以及代表性 `TestMain`（如 `pkg/session/main_test.go`、`pkg/statistics/main_test.go`），尤其确认回调仍在测试套件之后执行，并按预期保留或改变退出码。

兼容风险集中在公开 trait 签名、自由函数的 Go 风格名称及 `lib.rs` 的再导出；性能方面每次 `run` 有一次动态回调分派和一次 `RefCell` 运行时借用，适合测试入口的低频调用，不应未经测量扩展到热路径。

## 验证依据

- 源码与模块边界：`pkg/testkit/testmain/wrapper.rs`、`pkg/testkit/testmain/lib.rs`、`pkg/testkit/testmain/Cargo.toml`、根 `Cargo.toml`。
- Rust 行为测试：`pkg/testkit/testmain/wrapper_test.rs` 验证 `FnMut` 捕获状态跨调用变化以及 runner 调用次数；`pkg/testkit/testmain/migration_aster_unit_test.rs` 验证 `None` 恒等回调和“先 runner、后回调”的退出码变换；`pkg/session/test/bootstraptest2/main_test.rs` 验证清理回调不吞掉退出码。
- Rust 直接使用：`pkg/session/test/bootstraptest/lib.rs`、`pkg/session/test/bootstraptest/main_test.rs`、`pkg/session/test/main_test.rs`、`pkg/session/test/variable/main_test.rs`、相邻 `pkg/testkit/testmain/bench.rs`。
- Go 对照与真实用途：`pkg/testkit/testmain/wrapper.go`，以及 `pkg/session/main_test.go`、`pkg/session/test/main_test.go`、`pkg/statistics/main_test.go` 等对 `goleak.VerifyTestMain(testmain.WrapTestingM(...))` 的调用。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/testkit/testmain` 确认该目录 8 个已索引 Go/Rust 文件；`node --file pkg/testkit/testmain/wrapper.rs --offset 1 --limit 260` 返回完整 64 行源码、9 个符号及 5 个 Rust 使用文件；`query TestingM --limit 20` 与 `query WrapTestingM --limit 20` 定位 Rust trait/结构体/工厂函数及 Go 对照符号。对高度重载的 `run`，调用图未提供可用消歧结果，改用直接源文件证据核验，没有把模糊结果当作事实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核没有修改 Rust、Go、Cargo 或只读 `plan.md`。
