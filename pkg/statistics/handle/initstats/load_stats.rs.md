# `pkg/statistics/handle/initstats/load_stats.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-initstats` crate，负责计算启动阶段加载统计信息时应使用的并发度。crate 根文件 [`lib.rs`](lib.rs) 将私有模块 `load_stats` 的公开项整体再导出，因此调用方使用的是 crate 级 `GetConcurrency` / `get_concurrency_for`，而不是直接访问模块。工作区总 facade 还在 `pkg/lib.rs` 的 `statistics::handle::initstats` 命名空间转发该 crate。

它只决定“并发度是多少”，不创建线程、不分发任务，也不读取统计表。真正的区间 worker 位于相邻的 `load_stats_page.rs`。Go 完整应用中的对应入口位于 `pkg/statistics/handle/bootstrap.go::Handle.initStatsWithSession`：得到并发度后，将同一个值传给 histogram、TopN 和 bucket 三段并发加载。当前 Rust 搜索只发现 crate 再导出和独立迁移测试使用这些函数，没有发现与 Go `initStatsWithSession` 等价的 Rust 生产调用点；所以该文件现状是已迁移、已测试的并发度策略组件，而不是已验证接入 Rust 启动主链的完整流程。

## 核心职责

1. `get_concurrency_for` 把配置开关和处理器数量映射为稳定的并发度，便于脱离机器环境验证边界。
2. `GetConcurrency` 从全局配置读取 `performance.force_init_stats`，从标准库读取当前进程可用并行度，再委托给纯计算函数。
3. 无论机器报告多少处理器，最终结果都限制在闭区间 `[2, 16]`：强制初始化模式倾向使用更多处理器但预留 2 个；后台/非强制模式仅使用一半，减少对业务负载的影响。

文件不拥有统计缓存、事务、任务队列或进度状态；这些职责不应加入本文件。

## 主要符号

- `pub fn get_concurrency_for(force_init_stats: bool, processors: usize) -> usize`：可确定性测试的纯计算入口。它先用 `isize::try_from` 转换处理器数；若传入值大于 `isize::MAX`，以 `isize::MAX` 继续计算，避免无符号减法和转换溢出。`force_init_stats=true` 时计算 `processors - 2`，否则计算 `processors / 2`，最后以 `clamp(2, 16)` 收敛。
- `pub fn GetConcurrency() -> usize`：面向实际运行环境的公开包装函数。它用 `std::thread::available_parallelism()` 获取可用并行度；探测失败时以 `1` 作为输入，然后读取 `get_global_config().performance.force_init_stats` 并调用 `get_concurrency_for`。
- 模块内没有常量、结构体、枚举、trait、`impl` 或条件编译项。两个函数都经 `lib.rs` 的 `pub use load_stats::*` 导出；大写命名沿用 Go API，crate 根通过 `#![allow(non_snake_case, ...)]` 接受这种命名。

## 执行流程

实际入口 `GetConcurrency` 的流程如下：

1. 请求标准库返回当前进程可使用的非零并行度。
2. 成功时提取 `usize`；失败时退化为 `1`，而不是向调用方返回错误。
3. 从 `astersql-config` 的全局配置快照读取 `performance.force_init_stats`。
4. 调用 `get_concurrency_for`：
   - 强制初始化：先算 `processors - 2`，意图给 GC、统计缓存内部工作和其他系统任务保留资源；
   - 非强制初始化：先算 `processors / 2`，意图降低统计初始化对用户业务的影响；
   - 将结果限制为最少 2、最多 16。
5. 返回 `usize` 并发度，不在本文件内启动任何消费者。

代表性边界由 `migration_aster_unit_test.rs::concurrency_bounds_match_go_for_force_and_background_modes` 固化：强制模式 `(1, 4, 8, 64)` 个处理器分别得到 `(2, 2, 6, 16)`；非强制模式 `(1, 4, 10, 64)` 分别得到 `(2, 2, 5, 16)`。

## 数据与状态

`get_concurrency_for` 只使用两个按值参数和局部变量，不保留状态，也没有 I/O。将处理器数转换为有符号整数后再减 2，使 `processors=0` 或 `1` 时也不会发生 `usize` 下溢；负的中间结果最终被下界 2 收敛。极大的 `usize` 被替换为 `isize::MAX`，随后又被上界 16 收敛。

`GetConcurrency` 读取两项外部状态：操作系统/运行环境向 `available_parallelism` 报告的可用并行度，以及 `astersql-config` 保存的全局 `Config` 快照。`pkg/config/config.rs::get_global_config` 返回 `Arc<Config>` 的克隆，因而本文件只读配置，不修改全局状态。`force_init_stats` 在 Rust `Performance::default` 中默认为 `true`，但本函数不依赖默认值，始终以调用时的配置快照为准。

## 依赖与调用关系

- 上游导出：`pkg/statistics/handle/initstats/lib.rs` 声明私有 `mod load_stats` 并 `pub use load_stats::*`；根 `Cargo.toml` 以 `facade_statistics_handle_initstats` 引入此 crate，`pkg/lib.rs` 再导出为 `statistics::handle::initstats`。
- 已验证 Rust 调用者：`GetConcurrency -> get_concurrency_for`；`migration_aster_unit_test.rs` 直接调用两个函数。RustCodeGraph 没有给出 `GetConcurrency` 的外部生产调用边，普通 Rust 全仓搜索也仅找到上述测试、crate 根和 facade。因此不能声称 Rust 服务器已经用该值驱动 `RangeWorker`。
- 下游依赖：`GetConcurrency -> std::thread::available_parallelism`、`crate::config::get_global_config`、`get_concurrency_for`；纯计算函数只使用标准整数转换、除法和 `clamp`。
- Cargo 边界：`pkg/statistics/handle/initstats/Cargo.toml` 将 `config-dependency` 映射到 `astersql-config`，`lib.rs::config` 再公开转发它。本文件不直接依赖同 crate 的 `crossbeam-channel`、`anyhow` 或日志桥接；这些是 `load_stats_page.rs` 的职责。
- Go 生产链：`pkg/statistics/handle/bootstrap.go::Handle.initStatsWithSession -> initstats.GetConcurrency`，返回值依次传入 `initStatsHistogramsConcurrently`、`initStatsTopNConcurrently` 和 `initStatsBucketsAndCalcPreScalar`。

## 错误处理与边界

公开 API 不返回 `Result`。处理器探测失败被明确降级为输入 `1`，计算结果仍为下界 `2`。输入为 `0`、`1` 或 `2` 时不会下溢；超过 `isize::MAX` 的输入不会因转换失败而中止；任何输入最终都满足 `2 <= result <= 16`。

唯一未在本文件内恢复的异常来自全局配置实现：`pkg/config/config.rs::get_global_config` 在其读锁中毒时使用 `expect("global config lock poisoned")`，会 panic。正常配置读取不产生可传播错误。函数也不验证“返回 2 是否超过实际可用处理器数”；最小值 2 是与 Go 行为一致的策略约束，而不是硬件容量承诺。

## 并发与资源生命周期

本文件计算并发参数，但自身没有并发资源生命周期：不生成线程、不持有锁守卫、不创建 channel、不等待 worker，也没有清理步骤。`available_parallelism` 只是一次环境查询；`get_global_config` 返回共享配置的 `Arc` 快照，表达式结束后按普通引用计数规则释放本地克隆。

全局配置可在测试或运行期被替换，所以两次 `GetConcurrency` 调用可能观察到不同的 `force_init_stats`。迁移测试用 `serial_test::serial` 串行化配置变更，并通过 `restore_func` 在测试结束时恢复全局配置；这是测试对共享状态的生命周期约束，不是 `GetConcurrency` 自身的同步机制。

## 与 Go 版本的对应关系

Rust 实现直接对应 `pkg/statistics/handle/initstats/load_stats.go::GetConcurrency`：两边均在强制模式使用“处理器数减 2”，非强制模式使用“一半处理器”，然后限制到 `[2, 16]`；配置字段也都为 `Performance.ForceInitStats` / `performance.force_init_stats`。

两处实现存在运行时来源和类型层面的差异：Go 使用 `runtime.GOMAXPROCS(0)` 并返回 `int`，它反映 Go 运行时的并行执行上限；Rust 使用 `std::thread::available_parallelism()` 并返回 `usize`，它由标准库根据当前环境估算。这两个值通常表达相近意图，但不能假定在容器、CPU 亲和性或运行时人为限额下始终相同。Rust 还额外提供 `get_concurrency_for` 以隔离环境输入并覆盖边界测试，以及显式处理探测失败和 `usize -> isize` 转换失败。

Go 侧 `pkg/statistics/handle/handletest/initstats/init_stats_test.go` 验证完整统计初始化的并发加载效果、内存限制、表删除和按表 ID 初始化等行为，但没有直接断言 `GetConcurrency` 的所有数值边界。Rust 的独立 `migration_aster_unit_test.rs` 则直接覆盖两种模式的上下界和全局配置读取；其余 worker 生命周期测试针对相邻 `load_stats_page.rs`，不应误归为本文件逻辑。

## 扩展指南

- 调整并发公式、上下界或极端输入规则时，应优先修改 `get_concurrency_for`，并同步扩展 `migration_aster_unit_test.rs::concurrency_bounds_match_go_for_force_and_background_modes`；不要把测试内嵌回生产文件。
- 改变配置来源时，应同步更新 `GetConcurrency`、`pkg/statistics/handle/initstats/Cargo.toml` 的依赖边界及 `get_concurrency_reads_the_migrated_global_config`。涉及全局配置的测试必须继续串行并恢复配置，避免污染其他测试。
- 若要把此策略接入 Rust 启动加载主链，应在真正拥有 histogram/TopN/bucket worker 的上游模块调用 `GetConcurrency`，而不是让本文件承担任务调度；接线测试应放在对应上游 crate 的独立测试文件中，并验证该值实际控制消费者数量。
- 若追求与 Go 更严格的运行时等价，需要先确认 `available_parallelism` 与目标部署环境的 `GOMAXPROCS(0)` 语义差异，特别是容器配额、CPU 亲和性和人为运行时限额。直接替换探测 API可能改变性能与兼容性。
- 修改最大值会影响统计加载峰值 CPU、内存和下游数据库请求数；修改最小值会影响低核环境。任何变化都应同时核对 Go 文件，除非明确记录两种实现有意分叉。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `11467` 个文件；目标目录列出 `load_stats.rs`、`load_stats.go`、`load_stats_page.rs`、crate 根和迁移测试。
- RustCodeGraph `node --file pkg/statistics/handle/initstats/load_stats.rs`：确认文件共 47 行、两个函数的完整实现，并显示该文件被 `migration_aster_unit_test.rs` 使用。
- RustCodeGraph `query/node`：确认 `get_concurrency_for` 位于第 24 行、`GetConcurrency` 位于第 42 行，调用边为 `GetConcurrency -> get_concurrency_for`；未返回生产侧上游调用边。
- 读取的 Rust/Cargo 路径：`pkg/statistics/handle/initstats/load_stats.rs`、`pkg/statistics/handle/initstats/lib.rs`、`pkg/statistics/handle/initstats/Cargo.toml`、`pkg/statistics/handle/initstats/migration_aster_unit_test.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/config/config.rs`。
- 读取的 Go 路径：`pkg/statistics/handle/initstats/load_stats.go`、`pkg/statistics/handle/bootstrap.go`、`pkg/statistics/handle/handletest/initstats/init_stats_test.go`。其中 `bootstrap.go` 提供真实 Go 生产调用链，测试文件提供完整初始化的行为背景。
- 全仓 `rg` 核验：Rust 中 `get_concurrency_for` / 本 crate `GetConcurrency` 的使用仅见迁移测试和函数内部调用；Go 中精确找到 `pkg/statistics/handle/bootstrap.go:944` 的 `initstats.GetConcurrency()` 生产调用。
- 本任务是纯文档分析，按计划不运行 Cargo；文档结构由任务指定的 11 标题检查命令验证。
