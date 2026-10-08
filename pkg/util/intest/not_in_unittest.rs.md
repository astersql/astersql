# `pkg/util/intest/not_in_unittest.rs`

## 文件定位

本文件是 `astersql-util-intest` crate 的默认生产构建变体，源码入口为 [`not_in_unittest.rs`](not_in_unittest.rs)。它只定义一个进程级内部测试状态 `InTest`，初始值为 `false`。crate 根 [`lib.rs`](lib.rs) 在既不是当前 crate 单元测试、也未启用 feature `intest` 时编译本模块，并把该静态量再导出为 `astersql_util_intest::InTest`；测试或 `intest` feature 构建改选互斥的 [`in_unittest.rs`](in_unittest.rs)，其同名静态量初始值为 `true`。

该文件不位于 SQL 请求的纵向主链中，而是为多个子系统提供横向的“当前是否按内部测试模式运行”信号。当前 Rust 生产调用包括 [`pkg/expression/scalar_function.rs`](../../expression/scalar_function.rs) 的哈希缓存一致性检查、[`pkg/util/codec/codec.rs`](../codec/codec.rs) 的序列化容量检查、[`pkg/util/rowcodec/common.rs`](../rowcodec/common.rs) 的 keyspace 前缀分支，以及 [`pkg/metrics/metrics.rs`](../../metrics/metrics.rs) 的测试环境初始化跳过逻辑。默认值为假使这些仅供测试的检查或分支在普通生产构建中保持关闭。

## 核心职责

1. 为非 `cfg(test)` 且未启用 `intest` feature 的构建提供 `InTest = false` 的初始状态。
2. 与 `in_unittest.rs` 保持相同的公开类型和符号名，使 [`lib.rs`](lib.rs) 能按编译条件无差别再导出。
3. 使用 `AtomicBool` 而不是不可变常量，使调用方可在运行时临时覆盖测试状态，并允许多个线程无数据竞争地读取。
4. 镜像 Go 的 `//go:build !intest` 变体 [`not_in_unittest.go`](not_in_unittest.go)，同时用 Rust feature/cfg 表达互斥选择。

本文件不判断某个具体行为是否安全、不执行断言，也不自动恢复调用方写入的状态；这些策略均由读取或修改 `InTest` 的上层代码与测试负责。

## 主要符号

- `pub static InTest: std::sync::atomic::AtomicBool`：本文件唯一的模块级符号，以 `AtomicBool::new(false)` 初始化。`pub` 使其可由父模块再导出；[`lib.rs`](lib.rs) 通过 `pub use not_in_unittest::InTest` 暴露 crate 级 API。
- 文件内没有函数、类型、trait、impl、宏或其他常量，也没有运行时初始化函数。源文件顶部的说明明确该变体与 `in_unittest` 互斥编译。

名称沿用 Go 的 `InTest`，不符合通常的 Rust 全大写静态量命名；[`lib.rs`](lib.rs) 的 `#![allow(non_snake_case, non_upper_case_globals)]` 明确允许这一迁移兼容命名。

## 执行流程

该文件本身没有可调用函数，实际流程由编译期选择和调用方的原子操作组成：

1. 编译 `astersql-util-intest` 时，[`lib.rs`](lib.rs) 计算 `cfg(not(any(test, feature = "intest")))`。
2. 条件成立时，编译私有模块 `not_in_unittest`，并以相同条件将其中的 `InTest` 再导出；静态量初始化为 `false`。
3. 条件不成立时，本文件不进入该构建，改由 `in_unittest.rs` 提供初始值为 `true` 的同名静态量。因此两个定义不会同时存在。
4. 下游通过 `InTest.load(Ordering)` 选择普通或测试专用路径；需要模拟另一状态的测试可用 `store` 或 `swap` 临时覆盖。
5. 独立集成测试 [`not_in_unittest_test.rs`](not_in_unittest_test.rs) 先断言初值等于 `cfg!(feature = "intest")`，再执行翻转、跨线程观察、显式恢复和作用域退出恢复。

feature `enableassert` 不参与本文件的选择：只启用 `enableassert` 而未启用 `intest` 时，断言实现可以启用，但 `InTest` 仍由本文件初始化为 `false`。这是 [`lib.rs`](lib.rs) 两组 cfg 条件的直接结果。

## 数据与状态

唯一数据是一个进程级、可变、无租户或会话隔离的布尔原子量。初始值在程序装载静态数据时已确定，没有配置文件、环境变量、网络或存储参与。任何一次 `store`/`swap` 都会影响同一进程内之后读取该静态量的所有线程和所有子系统。

本文件只规定初值与存储类型，不规定内存顺序。当前已核对的生产调用通常以 `Ordering::SeqCst` 读取；[`not_in_unittest_test.rs`](not_in_unittest_test.rs) 的 `load`、`store`、`swap` 也使用 `SeqCst`。因此实际同步强度由每次调用指定，而不是由静态量声明决定。该标志本身不携带其他业务数据，不应被当作发布或保护其他共享状态的锁。

[`assert_common.rs`](assert_common.rs) 中的 `EnableInternalCheck` 是另一个独立原子开关。测试确认运行时改写 `InTest` 不会重新计算或重置 `EnableInternalCheck`；后者的初值由其自身的编译期 cfg 决定。

## 依赖与调用关系

本文件的唯一直接依赖是标准库 `std::sync::atomic::AtomicBool` 及其 `new(false)` 构造；没有第三方 crate、内部模块调用或错误类型。[`Cargo.toml`](Cargo.toml) 将本目录声明为 `astersql-util-intest` 库，库入口是 `lib.rs`，feature 为 `intest` 与 `enableassert`，并将 `not_in_unittest_test.rs` 声明为独立集成测试。

上游首先是 [`lib.rs`](lib.rs) 的条件模块声明和再导出。再导出后的代表性读取关系包括：

- [`pkg/expression/scalar_function.rs`](../../expression/scalar_function.rs)：已有哈希缓存时，在测试模式下重算并检查缓存一致性。
- [`pkg/util/codec/codec.rs`](../codec/codec.rs)：测试模式下记录并验证序列化缓冲区不会意外扩容。
- [`pkg/util/rowcodec/common.rs`](../rowcodec/common.rs)：决定 nextgen key 是否应由当前层移除 keyspace 前缀。
- [`pkg/metrics/metrics.rs`](../../metrics/metrics.rs)：测试或 `intest` 模式下跳过 gRPC channelz collector 设置。

仓库中还有多个 Cargo manifest 以路径依赖引用该 crate；根 [`Cargo.toml`](../../../Cargo.toml) 也以 `facade_util_intest` 注册它。RustCodeGraph 能定位目标静态量，但对其精确 `callers` 查询未在执行窗口内返回；上述调用关系因此由已索引的目标文件/符号信息与精确仓库引用搜索交叉核对。搜索结果中的同名 trait 方法、Go 变量、注释或 `pkg/util/sqlkiller` 自有常量不是本静态量的 Rust 调用边。

## 错误处理与边界

静态量初始化、原子读取和原子写入不返回 `Result`，本文件没有错误传播、日志或 panic 路径。真正的错误或 panic 行为属于使用该标志的下游分支，例如 codec 的测试专用容量不变量失败会 panic，不能归因于本文件自身。

需要特别保持以下边界：

- “默认值为假”只适用于本模块实际入选的构建；`cfg(test)` 或 feature `intest` 下使用的是另一个定义且默认值为真。
- `InTest` 是运行时可写状态，不是对当前是否由 `cargo test` 启动的不可变事实；读取结果可能已被测试或其他调用方覆盖。
- 该状态是全进程共享而非线程局部、请求局部或租户局部。临时改写若未恢复，会污染后续测试和并发执行。
- `enableassert` 与 `intest` 是不同 feature；打开断言不等于进入测试模式。
- 原子性只避免对这个布尔值本身的数据竞争，不保证依赖多个全局开关的复合状态具有事务一致性。

## 并发与资源生命周期

`AtomicBool` 允许各线程并发 `load`、`store` 或 `swap`，不会产生 Rust 数据竞争。当前独立测试通过 `std::thread::spawn` 证明一个线程翻转后，另一个线程能以 `SeqCst` 读取到翻转值；随后用局部 `Restore(bool)` 的 `Drop` 实现在作用域退出时恢复初始值，即使测试作用域因正常提前退出或栈展开离开也会执行恢复。

本文件不创建线程、任务、锁、通道、事务、文件或网络资源，也没有析构逻辑。静态量的生命周期覆盖整个进程。由于测试可能并行运行，仅有原子访问并不能防止两个测试的逻辑干扰；新增会改写该标志的测试应使用独立测试文件中的恢复守卫，并在需要时用测试级互斥机制串行化，而不能只依赖成对的裸 `store`。

## 与 Go 版本的对应关系

直接 Go 对照是 [`not_in_unittest.go`](not_in_unittest.go)：其 `//go:build !intest` 对应 Rust `cfg(not(any(test, feature = "intest")))` 的默认分支，两者都把 `InTest` 初始化为 `false`。启用变体分别位于 [`in_unittest.go`](in_unittest.go) 和 [`in_unittest.rs`](in_unittest.rs)，初值都为 `true`。

两端的主要差异如下：

- Go 使用包级可变 `bool`；Rust 使用 `AtomicBool`，所有读写必须显式选择内存顺序，因而可以无数据竞争地跨线程访问。
- Go 由 build tag `intest` 选择文件；Rust 由 Cargo feature `intest` 选择，并额外让当前 crate 的 `cfg(test)` 自动选择真值变体。
- Go 调用方可直接读写 `intest.InTest`；Rust 调用方必须调用 `load`、`store` 或 `swap`。
- Go 的 [`assert_common.go`](assert_common.go) 在包初始化时根据 `InTest || EnableAssert` 设置 `EnableInternalCheck`；Rust 的 [`assert_common.rs`](assert_common.rs) 直接用编译期 cfg 初始化该独立原子量。运行时改写 Rust `InTest` 不会触发重新初始化，这一点由 `not_in_unittest_test.rs` 显式验证。

Go 的 [`assert_test.go`](assert_test.go) 在 `intest` 测试构建中断言 `InTest` 为真；Rust 除对应的单元测试外，专门用独立集成测试覆盖依赖 crate 不继承 `cfg(test)` 的现实，以及默认/feature 两种初值和运行时覆盖语义。

## 扩展指南

若只新增 `InTest` 的读取点，应通过 crate 再导出的 `astersql_util_intest::InTest`（或现有 Cargo 别名）访问，不要绕过 [`lib.rs`](lib.rs) 直接依赖私有模块。先判断该行为确实是诊断、测试注入或 Go 兼容分支，避免让生产正确性依赖一个可被全局改写的测试标志；读取时选择与附近状态同步需求匹配的 `Ordering`。

若修改初值、类型或 cfg 选择，必须同步检查 [`in_unittest.rs`](in_unittest.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml) 和 Go 的 `in_unittest.go`/`not_in_unittest.go`，保持两个 Rust 变体具有完全相同的公开类型。回归测试应继续放在独立的 [`not_in_unittest_test.rs`](not_in_unittest_test.rs)，至少覆盖默认构建与 `intest` feature 构建的初值、跨线程可见性、运行时翻转、恢复以及不影响 `EnableInternalCheck`；不要把测试逻辑嵌入源文件。

若要提供作用域式覆盖 API，应把恢复行为设计为 panic 安全的守卫，并解决并行测试间的全局状态竞争。若要把状态细化为线程或请求级别，则会改变现有所有调用方可见的进程级语义，必须评估 Go 对齐、API 兼容性以及跨线程任务传播成本，不能只改本文件。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/util/intest`：确认目标源、互斥变体、crate 入口、Go 对照和独立测试均在索引中。
- RustCodeGraph `node --file pkg/util/intest/not_in_unittest.rs --offset 1 --limit 200` 与 `node 'not_in_unittest.rs::InTest'`：确认文件共 22 行，唯一业务符号位于第 22 行，签名为初始值 `false` 的公开 `AtomicBool`。
- RustCodeGraph `query InTest --kind constant --json`：区分 `not_in_unittest.rs::InTest`、`in_unittest.rs::InTest` 和仓库内其他同名符号。精确 `callers` 查询持续无输出后终止，因此没有把图缺失误写为“无调用者”，而是用定点引用搜索补证。
- 已读 Rust 证据：[`not_in_unittest.rs`](not_in_unittest.rs)、[`in_unittest.rs`](in_unittest.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`not_in_unittest_test.rs`](not_in_unittest_test.rs)、[`assert_common.rs`](assert_common.rs)。
- 已读调用方证据：[`pkg/expression/scalar_function.rs`](../../expression/scalar_function.rs)、[`pkg/util/codec/codec.rs`](../codec/codec.rs)、[`pkg/util/rowcodec/common.rs`](../rowcodec/common.rs)、[`pkg/metrics/metrics.rs`](../../metrics/metrics.rs)，并用 `rg` 区分真实原子读取、同名符号、注释和 Go 引用。
- 已读 Go 对照：[`not_in_unittest.go`](not_in_unittest.go)、[`in_unittest.go`](in_unittest.go)、[`assert_common.go`](assert_common.go)、[`assert_test.go`](assert_test.go)。
- 人工复核结论：本文件只提供默认为假的进程级原子状态；cfg 保证真假变体互斥；运行时覆盖不会重算 `EnableInternalCheck`；文件自身无错误、异步、I/O、锁或事务生命周期。
- 本任务为纯文档分析，依照计划未运行 Cargo；结构验证使用任务指定命令，结果在交付报告中记录。
