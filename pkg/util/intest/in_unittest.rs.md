# `pkg/util/intest/in_unittest.rs`

## 文件定位

本文件是 `astersql-util-intest` crate 的“内部测试模式已启用”变体。它不独立决定何时进入测试模式，而是由 [`lib.rs`](lib.rs) 的条件编译分支选择：当当前 crate 满足 `cfg(test)`，或启用 Cargo feature `intest` 时，`mod in_unittest` 被编译并将本文件的 `InTest` 再导出为 crate 的公开 API；其余构建改用 [`not_in_unittest.rs`](not_in_unittest.rs) 中初值为 `false` 的同名定义。

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明：库入口是 `lib.rs`，`intest` 是无附加依赖的开关型 feature。因而本文件位于测试/内部检查基础设施层，而不属于 SQL 请求、规划或存储执行主链；业务模块只在需要测试专用校验或行为分支时读取它暴露的进程级状态。

## 核心职责

本文件只有一项职责：为启用变体提供一个初始值为 `true`、可跨线程安全读取和覆盖的全局状态 `InTest`。它同时保留两种语义：

1. 编译阶段由 `lib.rs` 的 `#[cfg(any(test, feature = "intest"))]` 选择本实现，镜像 Go 的 `//go:build intest` 文件选择。
2. 运行阶段由调用方通过 `AtomicBool::load`、`store` 或 `swap` 观察或临时覆盖状态，镜像 Go 中可赋值的包级变量。

它不负责启用断言、执行断言、解析 feature，也不自动恢复调用方的临时修改；这些职责分别位于 `assert.rs`/`assert_common.rs`、Cargo/`lib.rs` 和具体测试的恢复守卫中。

## 主要符号

- `pub static InTest: std::sync::atomic::AtomicBool`（[`in_unittest.rs`](in_unittest.rs)）：本文件唯一的模块级符号，也是唯一公开 API。静态初始化器 `AtomicBool::new(true)` 在启用变体加载时建立初值，不分配堆内存，不调用其他项目函数。
- `InTest` 在源码中保留 Go 风格命名；[`lib.rs`](lib.rs) 通过 crate 级 `#![allow(non_snake_case, non_upper_case_globals)]` 接受这种兼容命名，并用 `pub use in_unittest::InTest` 将私有模块中的符号提升到 crate 根。
- 本文件没有常量、类型、trait、函数、`impl`、错误类型或额外条件编译项。条件编译位于模块入口 `lib.rs`，因此直接编译本文件内容时看不到选择逻辑。

## 执行流程

典型流程如下：

1. Cargo 为 `astersql-util-intest` 编译库；若该 crate 自身处于单元测试配置，或依赖图为它启用了 `intest` feature，`lib.rs` 选择 `in_unittest` 模块。
2. Rust 初始化 `InTest` 为 `AtomicBool(true)`，`lib.rs` 再导出该静态量。
3. 下游代码按自己的同步需求加载状态。例如 [`pkg/util/rowcodec/common.rs`](../rowcodec/common.rs) 使用 `SeqCst` 读取后决定是否在 next-gen 测试场景自行剥离 keyspace 前缀；[`pkg/util/codec/codec.rs`](../codec/codec.rs) 在测试态记录并校验序列化缓冲区容量；[`pkg/expression/scalar_function.rs`](../../expression/scalar_function.rs) 在测试态重算并核对哈希缓存。
4. 测试可以用 `store`/`swap` 临时切换状态。[`not_in_unittest_test.rs`](not_in_unittest_test.rs) 用 `Drop` 守卫恢复初值，并在线程中重新读取，避免修改泄漏到后续断言。

需要特别区分 `cfg(test)` 的作用域：Cargo 集成测试会把库作为普通依赖编译，所以 [`not_in_unittest_test.rs`](not_in_unittest_test.rs) 的期望初值是 `cfg!(feature = "intest")`；而挂在 `lib.rs` 下的单元测试会使本 crate 满足 `cfg(test)`，因此 [`assert_test.rs`](assert_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 断言初值为 `true`。

## 数据与状态

`InTest` 是进程内单例原子布尔量，初值为 `true`。状态不绑定线程、请求、会话或事务，也没有持久化表示；一次 `store` 会影响同一进程中此后读取该静态量的所有代码。

原子内存序由每个调用点显式选择，本文件不强制排序策略。现有关键读取和切换测试主要使用 `Ordering::SeqCst`；个别与其关联但独立的开关会使用 `Relaxed`。原子类型保证读取/写入本身无数据竞争，但不能替调用方保证跨测试的语义隔离。修改全局值的测试应像 `not_in_unittest_test.rs`、`rowcodec/common_test.rs` 那样保存并恢复旧值；若测试可能并行修改同一开关，还需像 `assert_test.rs` 对相关全局断言开关所做的那样在测试层串行化。

`InTest` 与 `EnableAssert`、`EnableInternalCheck` 是不同状态。`not_in_unittest_test.rs` 明确验证覆盖 `InTest` 不会重新初始化 `EnableInternalCheck`，扩展代码不能假设这些开关会联动。

## 依赖与调用关系

下游依赖仅为 Rust 标准库的 `std::sync::atomic::AtomicBool`；初始化过程没有项目内被调用函数，因此不存在普通函数 callee。

上游装配边是 `lib.rs -> in_unittest::InTest -> crate 根 InTest`。RustCodeGraph 对目标文件的节点读取显示该文件共 22 行并只有该静态符号，同时给出 `pkg/util/codec/codec.rs`、`pkg/util/rowcodec/common.rs`、`pkg/util/intest/assert_test.rs`、`pkg/util/intest/migration_aster_unit_test.rs`、`pkg/util/intest/not_in_unittest_test.rs` 等使用文件。精确文本复核确认实际行为调用点通过原子方法读取或修改它；静态量不是函数，因而这些关系是数据读取/写入边，而非传统函数调用边。

crate 外调用方必须依赖 `astersql-util-intest`，并在希望依赖 crate 采用启用变体时传播 `intest` feature；仅仅让调用方自身处于 `cfg(test)` 不会自动让依赖 crate 获得相同 cfg。[`pkg/metrics/metrics.rs`](../../metrics/metrics.rs) 因此同时检查本 crate 的 `cfg!(test)` 与依赖导出的 `InTest`，其注释直接记录了这一边界。

## 错误处理与边界

本文件没有 `Result`、panic 分支或可恢复错误，静态初始化也不会失败。边界风险来自状态使用方式，而不是本文件的控制流：

- 未启用 `intest` feature 且不是本 crate 单元测试时，公开的同名符号来自 `not_in_unittest.rs`，初值为 `false`；不能仅凭源文件存在便断言运行时一定为 `true`。
- `true` 只代表内部测试变体/可覆盖状态，不提供测试隔离，也不证明所有断言开关均已启用。
- 运行时覆盖是全局副作用。测试若 panic 前没有可靠恢复守卫，可能污染同一进程中的其他测试。
- 原子值只能表达布尔状态，没有嵌套覆盖计数；多个并发调用方各自执行“保存—修改—恢复”时，后恢复者可能覆盖另一方的更新。
- 下游把该值用于改变业务辅助路径时，应保持生产默认路径不受影响，并分别验证 `true`、`false` 两种值，而不能只验证编译成功。

## 并发与资源生命周期

`AtomicBool` 使跨线程读写免于数据竞争，无需锁，也没有任务、通道、文件句柄、网络连接或事务资源。静态量的生命周期覆盖整个进程；不存在析构阶段，也不会在测试结束后自动重置。

[`not_in_unittest_test.rs`](not_in_unittest_test.rs) 的证据表明，`swap` 后由新线程读取可观察到变更，`Drop` 恢复器则把资源生命周期问题转化为作用域生命周期管理。该模式适合单个修改者；若新增测试并行覆盖 `InTest`，仍应使用包级互斥锁或避免并行，因为原子安全不等于多个测试修改全局语义时彼此独立。

## 与 Go 版本的对应关系

直接 Go 对照是 [`in_unittest.go`](in_unittest.go)：`//go:build intest` 选择文件，包级 `var InTest = true` 提供可读写状态。Rust 以 `lib.rs` 的 `cfg(any(test, feature = "intest"))` 和本文件的 `AtomicBool::new(true)` 对应这两部分语义。禁用侧分别是 Go 的 [`not_in_unittest.go`](not_in_unittest.go)（`//go:build !intest`、初值 `false`）和 Rust 的 [`not_in_unittest.rs`](not_in_unittest.rs)。

两版的主要差异是：Go 直接读写普通 `bool`，Rust 调用方必须明确使用原子 `load`/`store`/`swap` 及内存序；Rust 因而支持无数据竞争的跨线程访问，但仍需测试层管理全局状态恢复。Rust 还把 crate 自身的 `cfg(test)` 视为启用条件，使单元测试无需额外 feature 即得到 `true`；跨 crate 测试则仍需 feature 传播或像具体调用方那样结合自己的 `cfg!(test)`。

Go 行为调用点与 Rust 对照一致：`rowcodec/common.go`/`.rs` 用它区分 keyspace 前缀处理，`codec/codec.go`/`.rs` 用它启用测试期容量校验，`expression/scalar_function.go`/`.rs` 用它启用哈希一致性检查。本文只据此说明已接线的直接语义，不推断所有 Go 调用点均已完整移植。

## 扩展指南

若只新增一个测试态行为，通常不应修改本文件；应在调用模块读取 crate 根导出的 `InTest`，保持生产默认分支，并在该模块的独立测试文件中覆盖两种运行时值。测试修改全局值时应保存旧值并用 RAII 守卫恢复，必要时加互斥锁，不能把测试逻辑内嵌进生产源文件。

若要改变“何时初始为 true”，修改入口应是 `lib.rs` 的互斥 `cfg` 分支和 `Cargo.toml` 的 feature 定义，并同步检查 `not_in_unittest.rs`；必须保证两个定义恰有一个被编译和再导出。若要改变状态类型或公开 API，则需同步本文件、禁用变体、所有 `.load/.store/.swap` 调用点以及 Go 对照语义，重点评估跨线程兼容性和全局状态污染风险。

最接近的独立验证位置是：

- [`not_in_unittest_test.rs`](not_in_unittest_test.rs)：公开 API、两种 feature 初值、运行时覆盖、跨线程观察和恢复；
- [`assert_test.rs`](assert_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)：本 crate 单元测试配置下初值为 `true`；
- [`pkg/util/rowcodec/common_test.rs`](../rowcodec/common_test.rs)：业务消费者在 `InTest=true/false` 下的差异。

性能上，新增热路径读取会产生原子操作成本；正确性上，最大的风险是 feature 未传播、错误理解 `cfg(test)` 的 crate 边界，以及测试未恢复全局状态。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/intest` 列出本文件、入口、Go 对照和相关测试；`node --file pkg/util/intest/in_unittest.rs --offset 1 --limit 200` 确认文件全貌与唯一静态符号；`node --file pkg/util/intest/lib.rs --offset 1 --limit 200` 确认互斥 cfg 和再导出。一次宽泛 callers/explore 查询超时，因此没有用其未返回结果支撑结论，静态量的数据使用边改由索引的 used-by 列表和精确 `rg` 交叉验证。
- Rust 源与配置：[`in_unittest.rs`](in_unittest.rs)、[`not_in_unittest.rs`](not_in_unittest.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- Rust 测试：[`assert_test.rs`](assert_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`not_in_unittest_test.rs`](not_in_unittest_test.rs)、[`pkg/util/rowcodec/common_test.rs`](../rowcodec/common_test.rs)。
- Go 对照：[`in_unittest.go`](in_unittest.go)、[`not_in_unittest.go`](not_in_unittest.go)、[`assert_test.go`](assert_test.go)、[`pkg/util/rowcodec/common.go`](../rowcodec/common.go)、[`pkg/util/codec/codec.go`](../codec/codec.go)、[`pkg/expression/scalar_function.go`](../../expression/scalar_function.go)。
- 已人工复核：文件存在原因是为启用构建变体提供可覆盖的测试态标志；运行方式是入口条件编译、静态初始化、调用方原子读取/写入；安全扩展要求保持启用/禁用变体对称、正确传播 feature，并隔离和恢复进程级测试状态。
