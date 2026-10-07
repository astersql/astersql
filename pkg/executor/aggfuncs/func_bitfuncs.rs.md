# `pkg/executor/aggfuncs/func_bitfuncs.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 根由 `pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 通过 `pub mod func_bitfuncs` 公开本模块。它保存 BIT_OR、BIT_XOR、BIT_AND 三种 SQL 位聚合共享的 Rust 累加状态机，直接状态类型是 `BitAggregator`，部分结果类型复用 `aggfuncs.rs` 中的 `PartialResult4BitFunc = u64`。

需要区分“实现存在”和“执行器已接线”：`builder.rs::build` 会把 `FunctionName::{BitOr, BitXor, BitAnd}` 映射成同名 `AggImplementation` 变体，但仓库搜索只发现目标类型被 `func_bitfuncs_test.rs` 与 `go_scenario_coverage_test.rs` 实例化，未发现生产路径把这些变体构造成 `BitAggregator`。因此本文件当前可确认提供位聚合算法与可测试状态，不应据此宣称完整 SQL 执行链已经调用它。

## 核心职责

- 用 `BitAggKind` 表示 OR、XOR、AND 三种运算，并由一个 `BitAggregator` 统一持有运算种类和 `u64` 累积值。
- 为三种运算选择正确单位元：OR/XOR 为 `0`，AND 为 `u64::MAX`；`new` 与 `reset` 保持相同规则。
- `update` 接收一批 `Option<i64>`，跳过 SQL NULL（`None`），将非空有符号值按 Rust 的 `as u64` 转换后逐个累计。
- `merge` 用相同位运算合并同种类的分区部分结果；`slide` 仅为可逆的 XOR 提供移出/移入窗口增量更新。
- 用 `DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE` 暴露部分结果的固定内存大小，和 Go 的内存计量常量对应。

本文件不负责表达式求值、结果写入 chunk、spill 调度或构建器实例化。Rust 的 spill helper 能序列化/反序列化同一个 `PartialResult4BitFunc`，但目标文件本身没有调用这些 helper。

## 主要符号

- `DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE: i64`：`size_of::<PartialResult4BitFunc>()` 的字节数；由于别名当前是 `u64`，测试确认其等于 `size_of::<u64>()`。
- `BitAggKind::{Or, Xor, And}`：决定单位元和每次累计采用的位运算。该枚举实现 `Clone + Copy + Debug + Eq + PartialEq`。
- `BitAggregator { kind, value }`：模块唯一实际聚合器。字段私有，调用方必须经 `new` 创建，并通过 `value` 读取结果。
- `BitOrUint64`、`BitXorUint64`、`BitAndUint64`：三个公开类型别名，均指向 `BitAggregator`；它们不产生不同的 Rust 类型，也不会在类型层阻止为别名传入错误的 `BitAggKind`。
- `BitAggregator::new(kind)`：建立带正确单位元的新状态。
- `reset(&mut self)`：保留 `kind`，恢复对应单位元。
- `value(&self) -> u64`：只读返回当前部分结果。
- `update<I>(values)`：接受任意 `IntoIterator<Item = Option<i64>>`，用 `flatten` 跳过 NULL，再调用私有 `apply`。
- `merge(&mut self, source)`：先断言两端 `kind` 相同，再把 `source.value` 当作一个部分结果应用到目标。
- `slide<I, J>(outgoing, incoming)`：断言当前种类为 XOR，然后依次更新移出集合与移入集合；XOR 自反性使“再异或一次”同时承担撤销和加入。
- `apply(&mut self, value)`：唯一实际分派点，分别执行 `|=`、`^=`、`&=`。

## 执行流程

1. 上游选择 `BitAggKind` 并调用 `new`。AND 从全 1 开始，使第一个有效值不被改变；OR/XOR 从全 0 开始。
2. 普通累计时，上游先把表达式求值结果整理为 `Option<i64>` 迭代器，再交给 `update`。`None` 被过滤；每个 `Some(v)` 转成 `u64` 并进入 `apply`。
3. `apply` 根据固定的 `kind` 修改 `value`。由于三种位运算都满足结合律，批次边界不会改变结果。
4. 分区聚合合并时，目标调用 `merge(source)`。种类一致后，源的单个 `u64` 部分结果按同一运算合入目标；这与逐行合并等价。
5. 滑动窗口仅走 `slide` 的 XOR 分支：先对离开窗口的有效值再次 XOR 以撤销，再 XOR 新进入窗口的有效值。两个阶段都复用 `update` 的 NULL 与转换规则。
6. 结果通过 `value` 读取；需要重用状态时调用 `reset`，而不是新建不同单位元的裸 `u64`。

空输入或全 NULL 输入不会调用 `apply`，因此返回单位元：OR/XOR 为 0，AND 为 `u64::MAX`。这一行为由 `func_bitfuncs_test.rs::bit_aggregates_empty_null_and_reset_states_match_go` 固定。

## 数据与状态

聚合器只有两个按值字段：不可通过公开 API 修改的 `kind` 和一个 `u64` 的 `value`。没有行缓存、NULL 计数、堆分配、借用外部上下文或额外“是否见过非空值”标记。`Option<i64>` 只存在于输入边界，状态中不保存 NULL。

负 `i64` 通过 `as u64` 按二进制位模式转换。例如 `-1_i64` 变为 `u64::MAX`；独立测试用该值确认三种聚合都遵循位模式而非数值范围检查。`merge` 不复制源聚合器，也不改变源状态，只读取其 `kind` 与 `value`。

`DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE` 只描述 `PartialResult4BitFunc`（一个 `u64`）的大小，不是整个 `BitAggregator` 的 `size_of`，也不包含任何执行器或容器开销。相邻 `spill_serialize_helper.rs::serialize_bit_func` 和 `spill_deserialize_helper.rs::deserialize_bit_func` 以 `u64` 编解码同一部分结果类型。

## 依赖与调用关系

直接依赖很窄：模块只导入 `crate::aggfuncs::PartialResult4BitFunc` 和标准库 `std::mem::size_of`，没有直接使用 `Cargo.toml` 中的外部 crate。crate 入口 `lib.rs` 公开模块，并在测试配置下挂载 `func_bitfuncs_test`。

已核实的上游关系如下：

- `builder.rs::build` 将三种 `FunctionName` 分类成 `AggImplementation` 位聚合变体，但没有引用 `BitAggregator`；这是元数据选择关系，不是已证实的实例化调用边。
- `func_bitfuncs_test.rs` 直接调用 `new`、`update`、`value`、`merge`、`slide`、`reset`，覆盖本文件全部公开方法和内存常量。
- `go_scenario_coverage_test.rs::{test_merge_partial_result4_bit_funcs,test_mem_bit_func}` 额外验证 OR 的分区合并以及 XOR/NULL 场景。

下游关系均在本模块内部结束于 `apply`。RustCodeGraph 对 `BitAggregator` 的符号查询只返回本文件定义和上述两个测试模块的导入；结合 `rg` 全仓引用结果，当前没有证据表明生产执行器直接调用该类型。若后续补齐接线，应从 `AggImplementation::{BitOr,BitXor,BitAnd}` 的消费端追踪，而不能只修改构建阶段的枚举映射。

## 错误处理与边界

`new`、`reset`、`value`、`update` 都不返回 `Result`；输入已被简化成完成求值的 `Option<i64>`，所以表达式求值错误不在本文件处理。`None` 被静默跳过，这是 SQL NULL 语义而不是错误。

两个契约违规会触发 panic：`merge` 合并不同 `BitAggKind`，以及对非 XOR 聚合调用 `slide`。这适合暴露内部接线错误，但意味着不可信输入边界不能直接依赖这些方法返回可恢复错误。三个公开类型别名不提供静态种类约束，调用者仍必须正确传入 `BitAggKind`。

空集结果采用单位元而不是 `Option<u64>`。这与当前 Go 实现的分配/reset 状态一致，但调用者若需要区分“没有有效行”和“有效行恰好聚合成单位元”，无法从本状态单独判断。整数转换不做溢出报错或符号拒绝，而是保留二进制补码位模式。

## 并发与资源生命周期

`BitAggregator` 不含锁、原子变量、任务、通道、文件句柄或显式堆资源。它是 `Copy` 值类型；并行分区应各自持有独立实例，完成后通过 `merge` 汇总，避免共享可变引用。Rust 的借用规则要求 `update`、`merge`、`slide`、`reset` 独占 `&mut self`，但该类型没有声明跨线程同步协议。

生命周期从 `new` 开始，经任意次数的更新/合并/滑动和读取，可由 `reset` 原地复用，最终按普通值自动释放。`slide` 不保存窗口行，只消费调用者提供的两个迭代器；因此窗口边界、行获取和批次资源都由上游负责。部分结果 spill 的缓冲生命周期属于相邻 serialize/deserialize helper，而不是本聚合器。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_bitfuncs.go`。两端共同使用 `uint64/u64` 部分结果，OR/XOR 的分配和 reset 单位元为 0，AND 覆盖为最大 `uint64`；三种更新都跳过 NULL，把 `int64` 转为无符号位模式后累计，并以同一运算合并部分结果。XOR 是唯一实现滑动窗口撤销的种类。

实现形态存在重要差异：Go 用 `baseBitAggFunc` 实现 partial-result 分配、reset、最终写 chunk、spill 序列化/反序列化，并用 `bitOrUint64`、`bitXorUint64`、`bitAndUint64` 三个不同结构实现 `AggFunc`；Rust 目标文件将三者合并为带 `kind` 的轻量状态机，三个名称只是类型别名。Go 的 `UpdatePartialResult` 自行从 `chunk.Row` 求值并传播表达式错误，Rust `update` 只接收已求值的 `Option<i64>`，因此尚不等价于完整 Go `AggFunc` 接口。

Go `builder.go::{buildBitOr,buildBitXor,buildBitAnd}` 会直接返回具体 `AggFunc`；Rust `builder.rs::build` 当前只产生 `BuiltAggFunc` 元数据。Go 测试 `func_bitfuncs_test.go::{TestMergePartialResult4BitFuncs,TestMemBitFunc}` 的主要意图由 Rust 独立测试覆盖；Rust 还显式覆盖单位元、NULL/reset、负数位模式和 XOR slide。Go 的 spill 行为另由 `spill_helper_test.go::TestPartialResult4BitFunc` 覆盖，不能把它算作目标 Rust 类型自身的调用证据。

## 扩展指南

新增或修改位聚合语义时，应优先在 `BitAggKind`、`new/reset` 的单位元规则以及 `apply` 的单次运算分派处保持一致；若运算支持可逆滑动，再审查 `slide` 的数学恒等式，不能默认复用 XOR 的“移出再应用一次”。新增种类还必须同步检查 `merge` 的同类约束、`PartialResult4BitFunc` 是否仍能承载状态，以及 spill 编解码格式。

若目标是让 SQL 执行器实际运行本实现，需要补齐 `AggImplementation` 消费端到 `BitAggregator` 的生产接线，并同时处理表达式求值、错误传播、最终结果写入、内存计量与 spill 生命周期；只在 `builder.rs` 增加枚举映射不足以证明运行时可用。类型安全要求较高时，可考虑用不同新类型或专用构造器替代当前三个等价别名，但这属于 API 设计变更，需评估现有调用者。

测试必须继续放在独立文件 `pkg/executor/aggfuncs/func_bitfuncs_test.rs`，不要内嵌到生产源文件。至少同步覆盖：每种单位元、NULL/全 NULL、负数位模式、分区合并、错误种类合并的 panic、非 XOR slide 的 panic，以及 XOR 多行移出/移入。若补生产接线，还需在 builder/执行器的独立测试中证明从函数描述符到实际累计结果的端到端路径，并与 Go 的 `func_bitfuncs_test.go`、必要时 `spill_helper_test.go` 对齐。

## 验证依据

- Rust 源码：`pkg/executor/aggfuncs/func_bitfuncs.rs`（`BitAggKind`、`BitAggregator`、三个别名及其全部方法）。
- crate 与模块边界：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/lib.rs`；本包不存在 `doc.go`。
- 公共部分结果与相邻能力：`aggfuncs.rs::PartialResult4BitFunc`、`spill_serialize_helper.rs::serialize_bit_func`、`spill_deserialize_helper.rs::deserialize_bit_func`。
- Rust 构建元数据：`builder.rs::{FunctionName,AggImplementation,build}`；它证明 BIT_* 可被分类，但没有证明 `BitAggregator` 的生产实例化。
- Rust 独立测试：`func_bitfuncs_test.rs` 的四个测试，以及 `go_scenario_coverage_test.rs::{test_merge_partial_result4_bit_funcs,test_mem_bit_func}`。
- Go 对照：`func_bitfuncs.go`、`builder.go::{buildBitOr,buildBitXor,buildBitAnd}`、`func_bitfuncs_test.go::{TestMergePartialResult4BitFuncs,TestMemBitFunc}`，以及 spill 场景 `spill_helper_test.go::TestPartialResult4BitFunc`。
- RustCodeGraph：`status` 显示索引包含目标仓库；`node --file` 读取了目标、crate 入口、builder 与测试；`query BitAggregator`、`query BitAggKind` 和 `query DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE` 核对定义及测试侧引用。`files --filter` 未命中目标路径，因此对 Cargo、Go 对照及全仓引用使用了 `rg`/文件读取补证。
- 未运行 Cargo 或代码测试：任务是纯文档分析，验证采用上述源码/调用证据与任务指定的章节结构检查。
