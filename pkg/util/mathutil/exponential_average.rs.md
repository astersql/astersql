# [`pkg/util/mathutil/exponential_average.rs`](exponential_average.rs)

## 文件定位

本文件实现 `astersql-util-mathutil` crate 中的指数移动平均（EMA）状态机。crate 根 `pkg/util/mathutil/lib.rs` 以私有模块 `mod exponential_average` 挂载它，再公开重导出 `ExponentialMovingAverage` 和 `NewExponentialMovingAverage`；因此其他 crate 从 mathutil 的公开门面使用它，而不是直接访问本模块。

当前可确认的生产接线位于 `pkg/util/cpu/cpu.rs`：`ObserverState` 持有一个 EMA，`NewCPUObserver` 用 `factor=0.95`、`warmup_window=10` 创建它，采样线程约每 100 ms 将瞬时 CPU 使用率交给 `Add`，再用 `Get` 取得平滑值并发布到原子变量和 Prometheus 指标。该文件是通用数值工具，不负责采样、调度或指标上报。

## 核心职责

- `NewExponentialMovingAverage` 校验平滑因子并创建零初始化状态。
- `ExponentialMovingAverage::Add` 在预热期累计样本并计算算术平均，预热结束后切换为指数平滑。
- `ExponentialMovingAverage::Get` 返回最近一次算出的值，不推进状态。

核心不变量来自 `pkg/util/mathutil/exponential_average.rs`：合法 `factor` 应满足 `0 < factor < 1`；当 `count < warmup_window` 时 `value = sum / count`；否则 `value = old_value * (1 - factor) + sample * factor`。算法只保留常量大小状态，不保存历史样本序列。

## 主要符号

- `pub struct ExponentialMovingAverage`：公开类型，但五个字段均为模块私有。
  - `value: f64`：当前算术平均或 EMA，初始为 `0.0`。
  - `sum: f64`：预热样本之和；进入指数阶段后不再更新。
  - `factor: f64`：新样本权重，构造时写入且此后不变。
  - `warmup_window: isize`：使用算术平均的样本数阈值。
  - `count: isize`：已纳入预热累计的样本数；达到窗口后停止增长。
- `pub fn NewExponentialMovingAverage(factor: f64, warmup_window: isize) -> Box<ExponentialMovingAverage>`：Go 风格命名的公开构造函数。拒绝 `factor <= 0.0` 或 `factor >= 1.0`，其余字段按零值初始化。
- `pub fn ExponentialMovingAverage::Add(&mut self, value: f64)`：唯一的状态修改入口，`&mut self` 保证一次调用期间独占访问。
- `pub fn ExponentialMovingAverage::Get(&self) -> f64`：只读访问当前值。

文件没有 trait、模块级常量、条件编译项或内部辅助函数。`pkg/util/mathutil/lib.rs` 的 crate 级 `#![allow(non_snake_case)]` 允许这些为对齐 Go API 而保留的名称。

## 执行流程

1. 调用者通过 `NewExponentialMovingAverage` 传入 `factor` 和预热窗口；非法有限边界因子立即 panic，合法值生成装箱的零状态。
2. 每次 `Add(sample)` 先比较 `count` 与 `warmup_window`。
3. 若仍在预热期，依次增加 `count`、把样本加入 `sum`，最后以 `sum / count` 更新 `value`。这个顺序保证第一次合法预热采样的除数是 1。
4. 若预热已经结束，不再修改 `count` 或 `sum`，直接按 `old_value * (1-factor) + sample * factor` 更新 `value`。
5. `Get` 原样返回 `value`。构造后尚未添加样本时结果为 `0.0`。

`pkg/util/mathutil/migration_aster_unit_test.rs::exponential_average_matches_go_warmup_and_decay` 给出可复核的短流程：窗口为 2 时，样本 10、20 的结果依次为 10、15，第三个样本 30 切换到 EMA 后得到 27。`pkg/util/mathutil/exponential_average_test.rs::test_exponential` 则以 100 个固定样本验证最终值转换为 `i64` 后为 3886。

## 数据与状态

每个实例独立维护五个标量，空间复杂度为 `O(1)`，每次 `Add` 和 `Get` 的时间复杂度也为 `O(1)`。预热期间，`sum`、`count` 和 `value` 同步变化；指数阶段只有 `value` 变化，`sum` 与 `count` 保留预热结束时的值。

浮点运算遵循 Rust `f64`/IEEE 754 语义，不做舍入、饱和或有限性检查。样本为 `NaN` 或无穷时会按浮点规则传播到结果。构造函数的两个比较不能拒绝 `NaN factor`，因为与 `NaN` 的有序比较均为 false；这是从当前实现直接推出的边界，现有测试没有把它声明为受支持契约。`warmup_window <= 0` 不会进入预热分支，而会从初始值 `0.0` 直接执行 EMA；构造函数同样不拒绝这种输入。

## 依赖与调用关系

下游方面，本文件只使用 Rust 原生的 `Box`、`f64` 算术与 panic 机制，没有第三方 crate、系统调用、I/O 或跨模块函数调用。`pkg/util/mathutil/Cargo.toml` 表明 crate 名为 `astersql-util-mathutil`，库入口是 `lib.rs`；其唯一列出的开发依赖是 `astersql-testkit-testsetup`，本实现本身未使用它。`formal-crate`、`intest`、`enableassert` feature 也不改变本文件的实现。

上游方面，`pkg/util/mathutil/lib.rs` 公开重导出类型和构造函数。仓库搜索确认 Rust 生产代码 `pkg/util/cpu/cpu.rs` 在 `ObserverState` 中保存该类型，并在 `NewCPUObserver`、后台采样循环中分别调用构造函数、`Add` 和 `Get`；`pkg/util/cpu/Cargo.toml` 通过路径依赖 `../mathutil` 接入该 crate。RustCodeGraph 的目标文件节点还记录了 `pkg/util/mathutil/migration_aster_unit_test.rs` 对构造函数的两条测试调用边。宽泛的 `Add`/`Get` 名称在图查询中存在大量同名噪声，因此生产调用关系同时用限定路径的仓库搜索核验。

## 错误处理与边界

构造时若 `factor >= 1.0` 或 `factor <= 0.0`，函数以固定消息 `factor must be (0, 1)` panic，不返回 `Result`。`pkg/util/mathutil/exponential_average_test.rs::test_exponential_rejects_invalid_factor` 和迁移测试 `exponential_average_rejects_invalid_factor_like_go` 均验证 `factor=1.0` 的 panic 及消息。

实现没有对 `warmup_window`、输入样本或中间结果做显式校验。特别地，非正窗口会绕过算术平均阶段；`NaN factor` 会绕过构造检查；极大值、无穷或 `NaN` 样本可能产生非有限结果。正常的正窗口不会发生 `count=0` 除法，因为进入预热分支后先递增 `count`。若未来要收紧这些边界，需先确认 Go 兼容要求和 CPU 观测调用者的行为，避免把当前 panic/浮点传播语义静默改成另一种错误协议。

## 并发与资源生命周期

类型内部没有锁、原子量、通道或后台任务；方法注释明确它本身不提供线程安全。`Add(&mut self)` 在 Rust 类型系统中要求可变独占借用，`Get(&self)` 只读，但跨线程共享仍应由调用者提供同步。实际生产调用者 `pkg/util/cpu/cpu.rs` 把 EMA 放在 `Arc<Mutex<ObserverState>>` 内，由采样线程持锁更新和读取，这个锁属于 CPU observer，而不属于 EMA。

构造函数返回 `Box`，所有权由调用者持有；例如 `NewCPUObserver` 立即以 `*Box` 将值移入 `ObserverState`。实例没有自定义 `Drop`，离开作用域时按 Rust 常规规则释放；也没有外部资源需要关闭。EMA 的生命周期不控制 CPU observer 工作线程，线程启停由 `Observer::Start`/`Stop` 管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/mathutil/exponential_average.go`。Rust 保留了 Go 的公开类型名、构造函数名、`Add`/`Get` 方法名、五项状态、非法 factor panic 文本、预热更新顺序及 EMA 公式。字段类型作了语言对应：Go 的 `float64`/`int` 对应 Rust 的 `f64`/`isize`；Go 返回指针，Rust 返回 `Box`；Go 方法接收指针，Rust 用 `&mut self`/`&self` 表达可变和只读访问。

Go 测试 `pkg/util/mathutil/exponential_average_test.go::TestExponential` 与 Rust 的 `exponential_average_test.rs::test_exponential` 使用相同的 100 个样本、`factor=0.8`、窗口 2，并断言截断后的结果为 3886。Rust 另外覆盖了非法 factor 和简短的预热/衰减中间值。Go 与 Rust 的 CPU observer 也都以 `0.95, 10` 创建 EMA，并在周期采样后执行 `Add`/`Get`；这证明该实现已经接入相同的实际用途，而不只是孤立工具。

## 扩展指南

- 修改平滑公式、预热定义或状态字段时，首先保持 `Add` 中“递增计数、累计、求平均”的顺序，并同步对照 `pkg/util/mathutil/exponential_average.go`；若有意偏离，应明确记录兼容性理由。
- 修改构造约束或错误协议时，应扩充独立测试 `pkg/util/mathutil/exponential_average_test.rs`，至少覆盖 `factor` 的 0、1、区间内值、`NaN`，以及负数、零和正数预热窗口；不要把测试内嵌回生产源文件。
- 修改数值行为时，应保留固定序列回归，并在 `pkg/util/mathutil/migration_aster_unit_test.rs` 中维护 Go 对齐样例；同时检查 `pkg/util/cpu/cpu.rs` 的 CPU 指标平滑效果。
- 若要支持并发共享，不应在不评估热路径成本的情况下直接给每次 `Add`/`Get` 增加内部锁；当前调用者已在更大的 `ObserverState` 临界区内同步，再加锁可能重复同步并改变性能。
- 若增加重置、批量添加或可配置窗口等 API，需要在 `pkg/util/mathutil/lib.rs` 决定是否公开重导出，并保持 `Cargo.toml` 无不必要依赖。

兼容风险主要是 Go/Rust 数值结果和 panic 行为漂移；性能风险主要是把当前常量时间、无分配的更新路径改成保存样本或额外同步；正确性风险集中在预热切换点、非有限浮点数及非正窗口。

## 验证依据

- 源码与模块边界：`pkg/util/mathutil/exponential_average.rs`、`pkg/util/mathutil/lib.rs`、`pkg/util/mathutil/Cargo.toml`。
- Rust 直接调用与测试：`pkg/util/cpu/cpu.rs`、`pkg/util/cpu/Cargo.toml`、`pkg/util/mathutil/exponential_average_test.rs`、`pkg/util/mathutil/migration_aster_unit_test.rs`。
- Go 对照：`pkg/util/mathutil/exponential_average.go`、`pkg/util/mathutil/exponential_average_test.go`、`pkg/util/cpu/cpu.go`。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/util/mathutil` 列出目标实现、模块入口及独立测试；目标文件节点确认 71 行源码和五个符号；精确 `NewExponentialMovingAverage` 节点确认定义及迁移测试的两条调用边。生产 CPU 调用因跨 crate 图结果不完整，另以限定路径 `rg` 和源码核验。
- 人工复核：文档分别说明了文件存在的目的、构造/预热/衰减流程、状态不变量、CPU observer 上游接线、无内部并发资源的边界，以及安全扩展时应同步的 Go 文件和独立 Rust 测试。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定的章节计数命令验证文档存在且恰有 11 个固定二级标题。
