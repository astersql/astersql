# `pkg/util/collate/ucadata/data.rs`

## 文件定位

本文件属于 `astersql-util-collate-ucadata` crate（见 [`Cargo.toml`](Cargo.toml)），是 UCA（Unicode Collation Algorithm）生成表与排序规则实现之间共享的哨兵常量定义。crate 根 [`lib.rs`](lib.rs) 以 `pub mod data` 装配本模块，并通过 `pub use data::*` 将常量提升到 `ucadata` crate 根，因此上层可写作 `ucadata::LongRune8`。

文件不保存实际 Unicode 权重表，也不执行生成过程。权重数据分别位于 `unicode_ci_data_generated.rs`（Unicode 4.0.0）和 `unicode_0900_ai_ci_data_generated.rs`（Unicode 9.0.0）；生成逻辑位于 `generator/main.rs`。本文件末尾的命令只是再生成说明，不会在编译或运行时自动执行。

## 核心职责

核心职责只有一个：把数值 `0xFFFD` 定义为 `MapTable4` 中的长权重哨兵。当一个码点的排序权重不能装进单个 `u64`（最多四个 `u16` 权重单元）时，生成器把该哨兵写入主表，并把最多八个权重单元拆成两个 `u64` 保存到 `LongRuneMap`。消费者读到哨兵后，必须改查长表，而不能把 `0xFFFD` 当作普通排序权重。

文件同时提供 Rust 风格的 `LONG_RUNE_8` 和 Go 兼容名称 `LongRune8`，避免迁移代码与 Go 对照时重复定义或改变数值语义。它还记录 Go 与 Rust 两套表生成入口，明确生成行为由工具链显式触发。

## 主要符号

- `pub const LONG_RUNE_8: u64 = 0xFFFD`：规范的 Rust 常量名，也是哨兵值的单一数值来源。类型采用 `u64`，与生成表 `MapTable4` 的元素和两段长权重类型一致。
- `pub const LongRune8: u64 = LONG_RUNE_8`：面向 Go 迁移代码的公开别名；`#[allow(non_upper_case_globals)]` 只豁免命名风格，不改变可见性或行为。
- 文件没有类型、trait、函数、`impl` 或条件编译项，也没有可变静态数据。

## 执行流程

本文件自身没有运行时控制流；其值参与以下数据流程：

1. `generator/main.rs::cet::insertWeights` 过滤零权重。非零权重不超过四个时直接压入 `MapTable4`；为五至八个时将 `LongRune8` 写入主表，并把前四个和后四个权重分别压入 `LongRuneMap` 的两个 `u64`。
2. `generator/main.rs::cet::calcImplicitWeight` 遇到需要两段表示的隐式权重时，同样在主表写入 `LongRune8`，并把两段值写入长表。
3. 生成后的表由排序规则读取。`unicode_0900_ai_ci_impl.rs::convertRuneUnicodeCI0900` 将主表值与 `ucadata::LongRune8` 比较；命中后按码点二分查找 `long_rune_map`，返回两段权重。Unicode 4.0.0 的 `unicode_0400_ci_impl.rs::unicode0400Impl::GetWeight` 使用数值相同的本地 `longRune`，命中后调用 `DUCET0400Table::long_rune_weight`。
4. 未命中哨兵时，消费者直接返回主表中的单段权重，第二段为零。

## 数据与状态

`LONG_RUNE_8` 与 `LongRune8` 都是编译期 `u64` 常量，不分配内存、不持有状态，别名不会产生第二份运行时存储。`0xFFFD` 可装入 `u16`，但在这里使用 `u64` 是为了能与主表元素直接比较；`data_1_aster_unit_test.rs::long_rune_8_matches_go_sentinel_and_weight_storage` 固定了数值、别名相等和 `u16` 范围这三个约束。

关键不变量是：任何写入哨兵的码点都必须在对应 `LongRuneMap` 中拥有条目，且长表必须满足消费者采用的查找布局。0900 表使用按码点排序的切片以支持二分查找；0400 生成表的 `long_rune_weight` 也按有序键二分查找。`unicode_ci_data_generated_3_aster_unit_test.rs` 检查 0400 长表键严格递增，`unicode_0900_ai_ci_data_test.rs` 检查 0900 长权重首段非零。

## 依赖与调用关系

本文件不导入任何 Rust 项，也没有函数调用下游依赖；依赖关系属于常量引用和数据协议。`ucadata/lib.rs` 是直接模块入口和重导出点。直接使用 `LongRune8` 的运行时代码是 `pkg/util/collate/unicode_0900_ai_ci_impl.rs::convertRuneUnicodeCI0900`；生成器 `pkg/util/collate/ucadata/generator/main.rs` 使用其自身 `generator/magic.rs::LongRune8`，该常量必须与本文件保持同值。

RustCodeGraph 对 `data.rs` 显示一个文件级常量节点，并能定位测试 `data_1_aster_unit_test.rs::long_rune_8_matches_go_sentinel_and_weight_storage`；常量的数据引用未形成普通函数 caller/callee 边。因此调用链还通过文本引用及消费者源码核验。上层业务位置是排序规则的字符到 UCA 权重转换：生成期把长权重编码为“主表哨兵 + 长表数据”，查询期再解码为两段权重。

## 错误处理与边界

常量定义本身不返回错误。边界由写入者和读取者维护：生成器只接受最多八个非零权重，超过八个时 `cet::insertWeights` 执行 `panic!("unreachable")`；五至八个权重走长表，零至四个权重留在主表。

读取端将“发现哨兵却找不到长表项”视为生成数据损坏。0400 的 `GetWeight` 与 0900 的 `convertRuneUnicodeCI0900` 都通过 `expect(...)` 失败，而不是静默返回零值。另一个易混淆边界是 Unicode 字符 U+FFFD 与哨兵恰好同值：`cet::insertWeights` 对 0900 的 U+FFFD 显式补入 `[0xFFFD, 0]` 长表项，以维持“主表值等于哨兵就必须能查长表”的协议。

## 并发与资源生命周期

两个常量均不可变、无所有权资源、无锁、无通道、无任务、无事务和无初始化顺序要求，可被任意线程无同步读取。表生成发生在离线命令中；运行时只读取编译进二进制的静态生成表。若更改哨兵，必须在生成器、既有生成数据与读取端之间原子地保持协议一致，否则并发安全虽然不受影响，所有查询线程仍会一致地读取错误协议数据或触发缺项 panic。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/util/collate/ucadata/data.go` 定义 `LongRune8 = 0xFFFD`，Rust 的 `LongRune8` 在名称和值上直接对应；`LONG_RUNE_8` 是 Rust 额外提供的惯用命名。Go 文件的两条 `//go:generate` 指令生成 4.0.0 与 9.0.0 数据文件，Rust 文件保留相同关系的说明，并另外列出 Rust generator 的显式 `cargo run` 命令。

Go 生成器 `generator/main.go::insertWeights` 与 Rust 的 `generator/main.rs::cet::insertWeights` 都在五至八个非零权重时写 `LongRune8` 并填充两段长权重；两者也都在隐式权重产生第二段时使用相同协议。Go 测试 `unicode_ci_data_test.go` 对照完整 0400 主表与长表并检查长权重唯一性，`unicode_0900_ai_ci_data_test.go` 检查 0900 Jamo 布局与长权重首段非零；对应 Rust 测试保留了这些语义。

## 扩展指南

- 若增加新的 UCA 版本或表格式，优先复用 `LONG_RUNE_8`/`LongRune8`，并在新生成器写入路径与新读取路径同时实现“哨兵命中后查长表”。不要在消费者或生成器中再引入第三个未经校验的数值副本。
- 若必须改变哨兵值，需要同步 `data.rs`、`generator/magic.rs`、Go 的 `data.go`/`generator/magic.go`、全部生成表以及 0400 本地 `longRune`；这是数据格式兼容变更，旧生成表与新读取器不能混用。
- 新生成格式若继续采用二分查找，必须保持长表键严格递增且唯一；若改变容器，应同步修改读取算法和生成表测试。还需保留“哨兵一定有长表项”“最多八个权重”“U+FFFD 特例”这些不变量。
- 测试应放在独立文件。常量协议直接扩展 `data_1_aster_unit_test.rs`；生成器编码路径扩展 `generator/migration_aster_unit_test.rs`；生成表布局和查找分别扩展 `unicode_*_data_generated_*_test.rs` 与 Go 对照测试，不应把测试嵌入 `data.rs`。
- 性能风险主要在热路径额外长表查找和生成表尺寸；兼容风险主要在 Rust/Go 常量或生成/读取协议漂移。修改后应重新生成数据并核对 diff，不能只验证常量单测。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 `pkg/util/collate/ucadata`；`files --filter pkg/util/collate/ucadata` 确认源、生成器及独立测试；`node --file pkg/util/collate/ucadata/data.rs` 核对完整源码；`query LONG_RUNE_8` 定位直接协议测试；`explore "LONG_RUNE_8 LongRune8 MapTable4 LongRuneMap ucadata"` 核对生成器、消费者和测试上下文。常量引用没有可靠的普通 caller/callee 边，故未把模糊的图结果当作调用证据。
- crate 与入口：`pkg/util/collate/ucadata/Cargo.toml`、`pkg/util/collate/ucadata/lib.rs`。
- Rust 生产证据：`pkg/util/collate/ucadata/data.rs`、`generator/main.rs`、`generator/magic.rs`、`unicode_ci_data_generated.rs`、`unicode_0900_ai_ci_data_generated.rs`、`pkg/util/collate/unicode_0400_ci_impl.rs`、`pkg/util/collate/unicode_0900_ai_ci_impl.rs`。
- Rust 测试证据：`data_1_aster_unit_test.rs`、`generator/migration_aster_unit_test.rs`、`unicode_ci_data_generated_3_aster_unit_test.rs`、`unicode_ci_data_test.rs`、`unicode_0900_ai_ci_data_test.rs`。
- Go 对照证据：`data.go`、`generator/main.go`、`generator/magic.go`、`unicode_ci_data_test.go`、`unicode_0900_ai_ci_data_test.go`，以及运行时消费者 `pkg/util/collate/unicode_0400_ci_impl.go`、`pkg/util/collate/unicode_0900_ai_ci_impl.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文档存在且恰有十一个固定二级标题。
