# `pkg/util/fastrand/runtime.rs`

## 文件定位

`runtime.rs` 是 `astersql-util-fastrand` crate 的最低层快速随机源。crate 入口 `pkg/util/fastrand/lib.rs` 以 `pub mod runtime` 装配本模块，并通过 `pub use runtime::*` 将唯一公开函数 `Uint32` 提升到 crate 根；同一 crate 的 `random.rs` 再以它为种子或原始随机字来源，构造 `Buf`、`Uint32N` 和 `Uint64N` 等高层 API。

该文件不是数据库 SQL 主链中的独立服务，也不拥有网络、存储或事务资源。它存在的目的，是为需要高频非加密随机值的工具代码提供一个与 Go `pkg/util/fastrand/runtime.go` 同形的 `u32` 入口。`pkg/util/fastrand/Cargo.toml` 将 crate 名定义为 `astersql-util-fastrand`，库入口为 `lib.rs`，并声明运行时依赖 `fastrand = "2"`；根 `Cargo.lock` 当前将其解析为 `fastrand 2.5.0`。

## 核心职责

- 公开 `pub fn Uint32() -> u32`，每次调用返回一个伪随机 32 位无符号整数（`runtime.rs:29-31`）。
- 把具体生成机制委托给外部 `fastrand` crate 的 `fastrand::u32(..)`；全范围 `..` 表示不在本层做有界缩放（`runtime.rs:30`）。
- 为 Go 版本依赖的 `runtime.cheaprand` 能力提供 Rust 迁移边界，使 `random.rs` 不必感知 Go 的 `//go:linkname` 或 Rust 生成器细节。

本文件不负责密码学随机、安全令牌、可复现种子、概率分布校验或有界随机算法。范围缩放与派生序列属于 `random.rs`，而实际随机状态管理属于 `fastrand` 依赖。

## 主要符号

`pub fn Uint32() -> u32` 是文件中唯一的常量、类型、trait、函数或 `impl` 级符号，且没有条件编译分支。

- 可见性：`pub`，并由 `lib.rs:20` 再导出，因此 crate 使用者可从 crate 根调用。
- 输入：无参数；调用者不能从该 API 注入种子或选择范围。
- 输出：完整 `u32` 类型值；函数签名没有 `Result`、`Option` 或借用生命周期。
- 实现：直接返回 `fastrand::u32(..)`，没有包装状态、转换、重试或后处理。

命名保留 Go 导出函数的 `Uint32` 大写形式；`lib.rs:8-13` 在 crate 级允许 `non_snake_case` 等迁移命名，以维持 Go/Rust API 对照。

## 执行流程

一次调用只经过以下路径：

1. 上层从 crate 根再导出或 `random.rs` 的模块导入进入 `Uint32`。
2. `Uint32` 用无界范围 `..` 调用 `fastrand::u32`。
3. 外部 crate 返回 `u32`，本函数不做修改便交还调用者。

本 crate 内已确认的直接上游是：

- `Buf(size)` 在 `random.rs:57` 调用一次 `Uint32`，将结果扩展为 `u64` 并初始化局部 `wyrand`，随后由 `wyrand::Next` 生成缓冲区所需序列。
- `Uint32N(n)` 在 `random.rs:77` 调用一次，并用 64 位乘法的高 32 位把原始值缩放到 `[0,n)`；当 `n == 0` 时结果自然为 0。
- `Uint64N(n)` 在 `random.rs:85-87` 调用两次，将第一个值放入高 32 位、第二个值放入低 32 位，拼成 `u64` 后再按 `n` 缩减。

因此，`runtime.rs` 是高层随机 API 的熵入口，但不实现那些 API 的边界规则。

## 数据与状态

本文件自身不声明静态变量、线程局部变量、堆对象或可变结构体，也不会缓存返回值。函数栈帧中只有外部调用产生并立即返回的 `u32`。

源码注释说明 Rust 迁移使用 `fastrand` 的线程局部生成器代替 Go runtime 的廉价随机源（`runtime.rs:16-23`）。据此，生成器状态由依赖库管理，而不是由本模块共享或加锁管理。调用结果没有稳定序列承诺；`migration_aster_unit_test.rs:57-61` 只要求连续采样能够出现不同值，并未固定具体输出向量。

`Uint32` 的结果在下游有两类状态用途：作为 `Buf` 的局部 `wyrand` 初始种子，或作为 `Uint32N`/`Uint64N` 的一次性原始值。它不写入数据库状态，也不跨调用保存业务状态。

## 依赖与调用关系

下游依赖只有 `fastrand::u32`。`pkg/util/fastrand/Cargo.toml:10-11` 声明 `fastrand = "2"`，根 `Cargo.lock` 记录当前精确版本为 2.5.0；`rand = "0.8"` 仅是开发依赖，供 `random_test.rs` 的标准随机对照使用，不参与 `Uint32` 实现。

模块内调用关系为 `lib.rs` 再导出 `runtime::Uint32`，随后 `random.rs:24` 导入它，并由 `Buf`、`Uint32N`、`Uint64N` 调用。精确源码搜索还确认 `random_test.rs` 的并行基准路径和 `migration_aster_unit_test.rs` 的非常量检查直接调用该符号。

RustCodeGraph 将 `runtime.rs` 索引为 2 个节点（文件与函数），能精确定位 `runtime.rs::Uint32`，但对该符号执行 `callers` 和 `callees` 均返回空数组；因此上述调用边使用精确源码引用补证，而没有把空图误解为“无人调用”。仓库范围搜索目前只发现 crate 外的 `pkg/util/misc_test.rs:199` 调用高层 `astersql_util_fastrand::Buf`，未发现生产 Rust 文件直接从 crate 根调用 `Uint32`；这表明当前可见的主要消费路径仍是本 crate 的高层 API。

## 错误处理与边界

`Uint32` 是无参数、无显式失败返回值的薄包装；本层没有可传播错误、恢复分支或日志。它也没有输入边界需要验证，并以 Rust `u32` 类型固定输出宽度。

调用者不能依赖单次调用一定与前次不同，也不能把输出当作密码学安全随机数；现有测试只验证非恒定性和派生 API 的范围/桶覆盖行为，不构成安全性或统计质量证明。若依赖库内部行为或版本改变，函数签名仍可能兼容，但序列、性能和线程行为可能变化。

与本文件相邻但属于下游的边界包括：`Uint32N(0) == 0`，以及 `Uint64N(0)` 使用 wrapping mask 分支而不报错；这些由 `random.rs` 实现并由 `migration_aster_unit_test.rs:35-44` 验证，不能归因于 `Uint32` 自身。

## 并发与资源生命周期

本函数不创建线程、任务、锁、通道、文件描述符或异步 future，也没有显式初始化和清理阶段。源码迁移注释将其描述为基于线程局部生成器的无锁快速随机入口，所以并发调用不通过本模块中的共享互斥量串行化。

`random_test.rs:29-44` 按 `thread::available_parallelism()` 创建作用域线程，`benchmark_fast_rand` 在每个工作线程反复调用 `Uint32`（`random_test.rs:86-94`），为当前并行可调用性提供直接测试路径。线程和作用域的生命周期由测试框架管理，随机生成器状态的生命周期由 `fastrand` 依赖管理；本文件没有可手工释放的资源。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/util/fastrand/runtime.go` 只声明 `func Uint32() uint32`，并借助 `//go:linkname Uint32 runtime.cheaprand` 将符号绑定到 Go runtime；空导入 `unsafe` 是启用 `go:linkname` 所需。Rust 没有对应的链接属性，因此 `runtime.rs` 保留相同的零参数/32 位返回 API，但以 `fastrand::u32(..)` 提供真实函数体。

两者保持的语义是：面向高频工具用途、返回无显式错误的 `uint32`/`u32` 快速随机值，并作为 `random.go`/`random.rs` 高层算法的底层来源。实现来源并不相同：Go 序列和状态由 Go runtime 决定，Rust 序列和状态由 `fastrand` 2.x 决定，因此不应期待跨语言逐值一致。

Go `random_test.go` 通过 `BenchmarkFastRand` 并行调用 `Uint32`，并通过 `Uint32N` 的范围和桶覆盖间接观察随机源；Rust `random_test.rs` 保留相同测试意图。Rust 额外的 `migration_aster_unit_test.rs:57-61` 明确检查该源不是恒定值。Go `main_test.go` 还有公共测试初始化和 goroutine 泄漏检查；Rust `main_test.rs` 仅提供 `Once` 驱动的幂等空初始化，不能视为 Go 泄漏检查的等价实现。

## 扩展指南

若要替换底层生成器或改变线程模型，最小修改点是 `runtime.rs::Uint32`，同时需要：

- 保持 `pub fn Uint32() -> u32` 的 API，除非已审计 `lib.rs` 再导出以及 `random.rs` 的全部调用点。
- 在独立测试文件中扩展 `migration_aster_unit_test.rs` 的源性质回归，并保留 `random_test.rs` 的并行调用覆盖；不要把测试嵌入生产源文件。
- 重新验证 `Buf` 的种子路径、`Uint32N` 的乘法缩放和 `Uint64N` 的双采样拼接，避免只测试包装函数而遗漏派生行为。
- 评估兼容风险：不能承诺与 Go `runtime.cheaprand` 逐值一致；若新增可复现需求，应另设可注入种子的 API，而不是暗中改变这个运行时随机入口。
- 评估性能与并发风险：避免引入全局互斥、系统熵源的逐次阻塞或每次调用的堆分配；当前调用者把该函数用于高频路径和并行基准。
- 若升级 `fastrand` 主版本，同步检查 `pkg/util/fastrand/Cargo.toml` 与根 `Cargo.lock`，并复核上游 crate 对范围语法和线程局部状态的契约。

若只是新增有界分布、缓冲区格式或确定性 PRNG 功能，应优先接入 `random.rs` 的对应高层符号，而不是让 `runtime.rs` 承担超出“原始 `u32` 来源”的职责。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳为 `1791342965170`；`files --filter pkg/util/fastrand` 确认本 crate 的 10 个已索引 Go/Rust 文件；`node --file pkg/util/fastrand/runtime.rs` 和 `node pkg/util/fastrand/runtime.rs::Uint32` 确认唯一函数及其三行实现；对该符号的 `callers --json`、`callees --json` 均得到 `[]`，随后以精确源码搜索补足调用边。
- 生产源码：`pkg/util/fastrand/runtime.rs`、`pkg/util/fastrand/lib.rs`、`pkg/util/fastrand/random.rs`。
- crate 与依赖：`pkg/util/fastrand/Cargo.toml`、根 `Cargo.toml` 的 workspace/facade 项、根 `Cargo.lock` 中的 `astersql-util-fastrand 0.1.0` 与 `fastrand 2.5.0` 条目。
- Go 对照：`pkg/util/fastrand/runtime.go`、`pkg/util/fastrand/random.go`。
- 独立测试：`pkg/util/fastrand/migration_aster_unit_test.rs`、`pkg/util/fastrand/random_test.rs`、`pkg/util/fastrand/main_test.rs`，以及 Go 对照 `random_test.go`、`main_test.go`。
- 精确引用搜索：`rg --glob '*.rs' 'Uint32\\('` 确认本 crate 的生产调用点和测试调用点；`rg` 搜索 crate 名确认当前仓库外层只在 `pkg/util/misc_test.rs` 直接调用高层 `Buf`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行固定 11 章结构检查，并人工确认没有把未解析的图边、统计性质或密码学安全性写成已验证事实。
