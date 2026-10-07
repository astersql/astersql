# `pkg/executor/aggfuncs/func_ntile.rs`

## 文件定位

该文件位于 `astersql-executor-aggfuncs` crate，由 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod func_ntile` 公开，提供 SQL 窗口函数 `NTILE(n)` 的 Rust 状态机。它负责在已经知道一个窗口分区总行数的前提下，按行生成从 1 开始的桶号；它不负责解析 SQL、计算窗口边界、读取表达式或向结果 `Chunk` 写列。

当前生产接线需要谨慎区分两层：`builder.rs::build_window_function` 能把 `FunctionName::Ntile` 转换成描述性元数据 `AggImplementation::Ntile { n }`，但全仓 Rust 引用检索没有发现把该枚举变体实例化为本文件 `Ntile` 的生产代码；`Ntile` 的直接调用者目前是 `func_ntile_test.rs` 和 `window_func_test.rs`。因此，本文件已经具有可执行、受测的分桶算法，但尚不能据现有证据宣称它已接入完整 Rust SQL 执行链。

## 核心职责

- `Ntile::new` 固化本次窗口函数的目标桶数，并初始化分区输出游标。
- `Ntile::update` 累加分区行数，计算 `quotient = num_rows / n` 和 `remainder = num_rows % n`。
- `Ntile::next_value` 逐行返回桶号：前 `remainder` 个桶各有 `quotient + 1` 行，其余桶各有 `quotient` 行，因而桶大小之差至多为 1，额外行优先进入靠前桶。
- `Ntile::reset` 为新分区重置行数和输出游标，同时刻意保留上次计算的 `quotient`、`remainder`；下一次合法 `update` 会重算它们。

本文件只维护六个 `u64` 字段，不实现 `aggfuncs.rs::AggFunc`、`Serializer` 或 `SlidingWindowAggFunc`，也没有内存增量、spill、合并部分结果等接口。

## 主要符号

- `pub struct Ntile`：唯一模块级类型，派生 `Clone`、`Debug`、`PartialEq`。字段均为私有，外部只能通过方法遵守状态迁移约束。
  - `n`：目标桶数；构造参数为 `None` 时存为 0。
  - `num_rows`：已由 `update` 累加的当前分区行数。
  - `quotient`、`remainder`：分别是当前 `num_rows / n` 与 `num_rows % n`，仅在 `n != 0` 时更新。
  - `cur_group_idx`：下一次输出使用的桶号，初值和重置值均为 1。
  - `cur_idx`：当前桶内已经输出的行数，初值和重置值均为 0。
- `pub fn new(n: Option<u64>) -> Self`：把缺失参数归一为 0；本函数本身不返回参数错误。
- `pub fn reset(&mut self)`：清零 `cur_idx`、`num_rows`，把 `cur_group_idx` 恢复为 1；不修改 `n`、`quotient`、`remainder`。
- `pub fn update(&mut self, row_count: u64)`：累加批次行数，并在合法桶数下刷新分桶参数。它允许一个分区分多批累计，但输出应在总行数确定后开始。
- `pub fn next_value(&mut self) -> Option<u64>`：输出一个桶号并推进游标；`n == 0` 时返回 `None` 且不改变游标。

文件没有模块级常量、trait、条件编译项或错误类型。

## 执行流程

典型分区流程如下：

1. 上游从常量参数取得桶数，调用 `Ntile::new(Some(n))`。例如 `window_func_test.rs::collect_ntile` 直接传入 `n`。
2. 上游统计分区行数，并调用一次或多次 `update(row_count)`；每次调用都先累加 `num_rows`，随后按累计总数重算基准桶宽和余数。
3. 分区共有 `num_rows` 行时，上游严格调用 `next_value` `num_rows` 次。每次先保存当前 `cur_group_idx` 作为结果，再增加 `cur_idx`。
4. 当前桶目标大小为 `quotient`；若 `cur_group_idx <= remainder`，再加 1。达到该大小时，`cur_idx` 归零且 `cur_group_idx` 加 1。
5. 处理下一分区前调用 `reset`，再用新分区行数调用 `update`。

以 8 行、3 桶为例，`quotient = 2`、`remainder = 2`，前三个状态段大小依次为 3、3、2，输出 `1,1,1,2,2,2,3,3`，这由 `func_ntile_test.rs::ntile_distributes_remainder_to_earlier_buckets` 验证。若桶数大于行数，例如 3 行、5 桶，则 `quotient = 0`、`remainder = 3`，实际三行依次进入桶 1、2、3；`window_func_test.rs::test_window_functions` 覆盖了该情形。

## 数据与状态

`Ntile` 是单分区、顺序消费的可变状态。`n` 在实例生命周期内不变；其余字段描述当前或最近一次分区。算法只使用整数除法、取模、比较和加法，每次 `next_value` 为 O(1)，对象自身占用固定空间，不随行数或桶数分配容器。

关键不变量是：在合法调用协议下，`cur_group_idx` 是下一行的桶号，`cur_idx` 小于当前桶目标大小，所有已输出行数等于此前各桶大小之和加 `cur_idx`。前 `remainder` 个桶比后续非空桶多一行。

`update` 可分批累计，但若在尚未收齐整个分区时调用 `next_value`，之后再次 `update` 会改变桶宽，先前已经输出的桶号不会回算。因此安全的上游协议是“先确定并累计完整分区行数，再输出结果”。同理，`next_value` 不记录输出上限；调用次数超过 `num_rows` 时仍可能继续产生桶号，这属于调用者必须防止的越界消费。

## 依赖与调用关系

本文件没有 `use` 声明，仅依赖 Rust 标准语言能力和 `Option<u64>`，因而不直接使用 `Cargo.toml` 中的任何外部 crate。它通过 `lib.rs::func_ntile` 模块进入 `astersql-executor-aggfuncs` crate；该 crate 的 Cargo 元数据声明 Go 对照包为 `pkg/executor/aggfuncs`。

已验证的上游关系：

- `builder.rs::build_window_function` 读取 `AggFuncDesc.args[0].constant`，形成 `AggImplementation::Ntile { n }`；参数缺失或不是可取的常量时通过 `Option` 传播为构建失败。
- `func_ntile_test.rs` 直接调用 `new`、`update`、`next_value`、`reset`，验证核心状态迁移。
- `window_func_test.rs::collect_ntile` 直接调用 `new`、`update` 和 `next_value`，再由 `test_window_functions` 对照 Go 窗口测试场景。

RustCodeGraph 对目标文件报告的文件级反向引用仅包含 `window_func_test.rs`；补充的全仓文本检索还确认了同 crate 的 `func_ntile_test.rs`。没有发现生产侧对 `Ntile::new` 或 `func_ntile::Ntile` 的调用，也没有发现 `AggImplementation::Ntile` 的消费端。因此 `builder.rs` 与本状态机之间目前是“语义对应但未接线”，不是已验证的直接调用边。

## 错误处理与边界

- `new(None)` 与 `new(Some(0))` 都令 `n == 0`；`next_value` 返回 `None`，对应 Go 实现向结果列追加 SQL `NULL` 的分支。Rust 类型没有区分“NULL 参数”和数值 0。
- `update` 在 `n == 0` 时不会除零，也不会更新 `quotient`、`remainder`。
- 空分区本身不会触发输出；若调用者在 `num_rows == 0` 时错误调用 `next_value`，函数仍可能返回 `Some(1)`，所以空分区约束由上游保证。
- 当 `n > num_rows` 时，前 `num_rows` 个桶各得到一行，其余桶为空；算法不会专门生成空桶。
- `reset` 后、下一次 `update` 前调用 `next_value` 会使用上一个分区遗留的 `quotient`、`remainder`。这是与 Go `ResetPartialResult` 一致且已有 Rust 测试锁定的行为，但正常生命周期不应在此阶段求值。
- 各字段及累加使用 `u64`；`num_rows += row_count`、游标加一在溢出时没有显式错误处理，debug 构建会 panic，release 行为取决于编译溢出设置。现实分区规模通常远低于该上限，但扩展接口时不应把它表述为经过校验的输入。
- 本文件不返回 `Result`，不产生 `AggError`，也不做 SQL 参数合法性诊断；参数检查或错误分类必须发生在描述符/规划层或未来的执行适配层。

## 并发与资源生命周期

`Ntile` 不包含锁、原子变量、引用、任务、通道或外部资源。求值方法需要 `&mut self`，自然要求同一实例按顺序独占推进；若多个窗口分区并行计算，每个分区应拥有独立实例或独立状态，不能共享一个可变游标。

生命周期是“构造一次桶数配置 → 对某分区累计完整行数 → 按行顺序取值 → reset → 下一分区”。`Clone` 会复制所有中间游标和分桶参数，因此克隆体会从完全相同的求值位置继续，但文件没有为跨线程共享或合并克隆状态定义语义。对象无堆分配且无显式清理过程，离开作用域即可释放。

与 Go 完整实现相比，Rust 类型没有 `PartialResult` 的独立分配、`DefPartialResult4Ntile` 内存记账、chunk 所有权或 spill 生命周期；这些不能从本文件推断为已支持。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/aggfuncs/func_ntile.go`：

- Rust `Ntile.n` 对应 Go `ntile.n`；Rust 其余五个字段合并了 Go `partialResult4Ntile` 的状态。
- `new` 的初始化对应 Go `AllocPartialResult` 创建 `curGroupIdx: 1`，但 Rust 不返回 `DefPartialResult4Ntile` 内存增量。
- `reset` 逐字段对应 `ResetPartialResult`，两边都保留 `quotient` 与 `remainder`。
- `update(row_count)` 对应 `UpdatePartialResult(rowsInGroup)`：Go 从切片长度取得增量，Rust 由调用者传入行数；两边都累计后重算商和余数，返回路径都没有动态内存增长。
- `next_value` 对应 `AppendFinalResult2Chunk` 的分桶和游标推进部分；Go 直接向 `Chunk` 追加 `uint64` 或 NULL 并返回 `error`，Rust 只返回 `Option<u64>`。
- Go `ntile` 嵌入 `baseAggFunc`，由 `builder.go::buildNtile` 构造并通过 `BuildWindowFunctions` 接入 `AggFunc` 执行链；Rust builder 当前只返回 `AggImplementation::Ntile { n }` 元数据，未发现到本文件类型的生产实例化。

Go 行为测试 `window_func_test.go` 验证 4 行/3 桶得到 `1,1,2,3`，3 行/5 桶得到 `1,2,3`；Rust `window_func_test.rs` 镜像了这两例。Go `func_ntile_test.go::TestMemNtile` 还验证固定部分结果大小与更新内存增量；Rust 本文件没有相应内存记账接口，所以不能把该覆盖视为等价实现。

## 扩展指南

若只修改分桶规则或游标生命周期，应集中修改 `Ntile::update`、`Ntile::next_value` 或 `Ntile::reset`，并同步独立测试 `func_ntile_test.rs`；不得把测试嵌入生产源文件。至少覆盖整除、有余数、`n > num_rows`、`n == 0`、多批 `update`、分区 `reset` 和禁止超量消费的调用契约，同时同步核对 Go `func_ntile.go` 与 `window_func_test.go`，避免偏离移植语义。

若要完成生产接线，应在消费 `builder.rs::AggImplementation` 的执行器工厂中新增 `Ntile` 实例化与完整窗口生命周期适配，而不能仅修改本文件。适配层需要决定如何先获得完整分区行数、如何把 `Option<u64>` 写成 SQL 值/NULL、如何报告内存、以及是否实现 `AggFunc` 或专门窗口 trait；这些属于本文件当前能力之外。还应新增从描述符构建到执行输出的集成级 Rust 测试，确认 `AggImplementation::Ntile { n }` 不再只是元数据。

性能上应保持每行 O(1) 和固定内存；兼容性上必须保留 1 起始桶号、余数优先前桶、`n > 行数` 时一行一桶，以及 NULL/0 参数边界。若改变 `reset` 清理商和余数的策略，必须同步评估现有测试锁定的 Go 兼容行为。

## 验证依据

- RustCodeGraph `status`：索引包含目标工程；目标文件被完整索引为 87 行、6 个符号。
- RustCodeGraph `node --file pkg/executor/aggfuncs/func_ntile.rs`：核对 `Ntile` 六个字段及 `new`、`reset`、`update`、`next_value` 的完整实现。
- RustCodeGraph `query Ntile` 与 `files --filter pkg/executor/aggfuncs`：定位 Rust/Go 类型、builder 枚举、独立测试及模块范围。
- RustCodeGraph 对 `builder.rs`、`lib.rs`、`func_ntile_test.rs`、`window_func_test.rs` 的文件节点：核对模块公开、构建元数据和测试调用链。符号级 `callers/callees` 查询未返回可用边，因此使用文件反向引用及全仓 `rg` 补齐调用证据，并明确记录未接线结论。
- `pkg/executor/aggfuncs/Cargo.toml`：核对 crate 名、`lib.rs` 入口及 Go 包映射；目标文件自身没有外部依赖。
- `pkg/executor/aggfuncs/func_ntile.go`、`builder.go`、`func_ntile_test.go`、`window_func_test.go`：核对 Go 的完整 `AggFunc` 生命周期、builder 接线、内存测试和输出案例。
- `pkg/executor/aggfuncs/builder.rs`：核对 `FunctionName::Ntile` 到 `AggImplementation::Ntile { n }` 的描述符构建路径；全仓 Rust 检索未发现该变体的生产消费端。
- 交付结构检查使用任务指定命令，要求本文档存在且恰好包含全部 11 个固定二级标题。本任务为纯文档分析，按计划不运行 Cargo。
