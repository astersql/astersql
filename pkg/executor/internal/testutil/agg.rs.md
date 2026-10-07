# `pkg/executor/internal/testutil/agg.rs`

## 文件定位

本文件属于 `astersql-executor-internal-testutil` crate，由同目录的 `lib.rs` 通过 `pub mod agg` 声明并用 `pub use agg::*` 再导出。它是执行器内部测试工具的一部分，负责描述聚合算子测试/基准所需的参数和固定输入列布局，不是 SQL 聚合的生产执行实现。

`pkg/executor/internal/testutil/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/executor/internal/testutil`；crate 的直接依赖是 `astersql-executor-internal-exec`，而本文件自身只使用同 crate `testutil.rs` 中的轻量测试类型。当前 Rust 侧直接验证入口是 `pkg/executor/internal/testutil/testutil_test.rs::operator_cases_match_go_defaults`；完整聚合基准消费流程仍可从 Go 对照 `pkg/executor/benchmark_test.go` 观察。

## 核心职责

本文件有四项职责：

1. 用 `AggTestCase` 汇总执行器种类、聚合函数、分组 NDV、行数、并发度、输入有序性、DISTINCT 开关和测试会话上下文。
2. 用 `AggTestCase::Columns` 固定聚合测试的两列 schema：索引 0 为 `Double` 聚合输入列，索引 1 为 `LongLong` 分组键。
3. 用 `Display::fmt` 生成稳定的 Go 风格用例标签，供断言以及基准子项命名。
4. 用 `DefaultAggTestCase` 产生与 Go 默认构造器一致的起始参数，并先恢复会话的默认 Chunk 容量。

它不解析 SQL、不构造聚合描述符、不生成输入数据，也不选择或运行 hash/stream 聚合器；这些行为由消费该参数对象的执行器测试或基准负责。

## 主要符号

- `pub struct AggTestCase`：公开的测试参数载体。`Ctx: SessionContext` 保存测试会话变量；`ExecType` 和 `AggFunc` 是由消费方解释的字符串；`GroupByNDV`、`Rows`、`Concurrency` 是规模参数；`DataSourceSorted` 与 `HasDistinct` 是执行分支参数。所有字段均公开，调用方可从默认用例出发逐项改写。
- `pub fn AggTestCase::Columns(&self) -> Vec<ColumnDef>`：每次调用都新建两个 `ColumnDef`。第 0 列是 `FieldKind::Double`，第 1 列是 `FieldKind::LongLong`；`ColumnDef::new` 还会令列默认可空且非 unsigned（定义见 `testutil.rs`）。该方法不读取 `self` 的字段，因此 schema 不随用例参数变化。
- `impl Display for AggTestCase::fmt`：按固定顺序输出 `execType`、`aggFunc`、`ndv`、`hasDistinct`、`rows`、`concurrency`、`sorted`。它省略 `Ctx`，避免把会话内部状态写进基准名称。
- `pub fn DefaultAggTestCase(mut ctx: SessionContext, exec: String) -> AggTestCase`：取得会话上下文和执行器标签的所有权，调用 `resetChunkSizes`，再返回默认 `sum` 用例。返回值是结构体本身，而非共享指针。

本文件没有模块级常量、trait、条件编译项或私有辅助函数。

## 执行流程

典型流程如下：

1. 调用方准备 `SessionContext` 和执行器类型字符串（例如 `"hash"` 或 `"stream"`），调用 `DefaultAggTestCase`。
2. 构造器先通过 `resetChunkSizes` 将 `ctx.vars.init_chunk_size` 和 `ctx.vars.max_chunk_size` 恢复为 `DEF_INIT_CHUNK_SIZE` 与 `DEF_MAX_CHUNK_SIZE`。
3. 构造器填入 `AggFunc = "sum"`、`GroupByNDV = 1000`、`Rows = 10_000_000`、`Concurrency = 4`、`DataSourceSorted = true`、`HasDistinct = false`，并保存传入的 `ExecType` 和已重置的 `Ctx`。
4. 调用方按测试维度改写公开字段；Go 基准 `BenchmarkHashAggRows`、`BenchmarkAggGroupByNDV`、`BenchmarkAggConcurrency` 和 `BenchmarkAggDistinct` 分别展示了行数、NDV、并发度和 DISTINCT 的参数扫描。
5. 消费方调用 `Columns` 得到聚合输入列与分组列，并把其余字段传给数据源和聚合执行器构造逻辑。Go 的 `buildAggExecutor` 以第 0 列构造聚合参数、以第 1 列构造 group-by 表达式，并依据 `ExecType` 选择 hash 或 stream 聚合；`benchmarkAggExecWithCase` 用 NDV、有序性和行数配置 mock 数据源。
6. `Display` 生成用例名称；Rust 回归测试锁定默认值及完整格式。

第 5 步描述的是 Go 对照消费流程，不代表当前 Rust 文件内已经实现聚合执行器接线。

## 数据与状态

`AggTestCase` 是可变的值对象，没有内部封装或派生缓存。默认状态及语义为：

| 字段 | 默认值 | 消费语义 |
| --- | ---: | --- |
| `ExecType` | 调用方传入 | 预期由消费方识别为 hash、stream 等类型；本文件不校验 |
| `AggFunc` | `sum` | 聚合函数名称；本文件不解析 |
| `GroupByNDV` | `1000` | 分组键不同值数量 |
| `Rows` | `10_000_000` | mock 输入行数 |
| `Concurrency` | `4` | 消费方使用的 worker/执行并发度 |
| `DataSourceSorted` | `true` | 输入是否按分组键有序 |
| `HasDistinct` | `false` | 聚合参数是否去重 |

`Ctx` 按值移入用例。`SessionContext` 当前是可克隆的测试结构，内部保存 `SessionVars`；默认构造器只重置两个 Chunk 大小字段，不触碰内存跟踪器。`Columns` 返回新分配的 `Vec<ColumnDef>`，调用方修改返回值不会回写 `AggTestCase`。

## 依赖与调用关系

上游关系：

- `lib.rs` 声明并再导出本模块，使 crate 用户能直接访问 `AggTestCase` 和 `DefaultAggTestCase`。
- RustCodeGraph 将 `pkg/executor/internal/testutil/testutil_test.rs::operator_cases_match_go_defaults` 识别为 `DefaultAggTestCase` 的 Rust 调用者；该测试检查 Chunk 初始容量、`sum`、千万行默认值及格式化输出。
- Go 对照的 `pkg/executor/benchmark_test.go` 多处调用 Go 版 `DefaultAggTestCase`，是这些参数在完整聚合基准链中的直接证据。仓库检索未发现除同 crate 回归测试之外的 Rust 消费方。

下游关系：

- `AggTestCase::Columns` 调用 `ColumnDef::new`，并依赖 `FieldKind::{Double, LongLong}`。
- `DefaultAggTestCase` 调用 `testutil.rs::resetChunkSizes`，后者写入 `DEF_INIT_CHUNK_SIZE`（32）和 `DEF_MAX_CHUNK_SIZE`（1024）。
- `Display::fmt` 只依赖标准库格式化接口，错误直接沿 `std::fmt::Result` 返回。

RustCodeGraph 对目标文件识别出 5 个符号（文件、`AggTestCase`、`Columns`、`fmt`、`DefaultAggTestCase`）；对简单字段构造和宏格式化没有给出完整静态 callees，因此局部依赖同时由索引源码和直接源码核验。

## 错误处理与边界

`Columns` 和 `DefaultAggTestCase` 都是不返回 `Result` 的纯构造路径，不会主动报告错误。`Display::fmt` 唯一可能返回的错误来自底层 `Formatter` 写入，使用 `write!` 的返回值原样传播。

本文件刻意不校验公开字段：未知 `ExecType`、空 `AggFunc`、`Concurrency = 0`、`Rows = 0`、NDV 大于行数等状态都能被构造。真正的合法性和失败行为属于消费方；Go `buildAggExecutor` 对未知执行器类型调用 `b.Fatal("not implement")`，但 Rust 文件没有对应分支。因此新增 Rust 消费者时不能把“能构造 `AggTestCase`”误认为参数已通过执行器校验。

固定 schema 是重要边界：第 0 列必须继续作为 Double 聚合输入，第 1 列作为 LongLong 分组键，否则 Go 基准中 `childCols[0]`/`childCols[1]` 的约定及相关移植语义会改变。`usize` 还意味着规模参数不能表达负数，其最大值随目标平台位宽变化。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、锁、事务或执行器，也不持有外部资源。`Concurrency` 只是数据字段；是否以及如何创建 worker 由下游聚合执行器决定。`DataSourceSorted` 同样只是对消费方的提示，不会在此处排序数据或验证顺序。

生命周期完全由所有权决定：`DefaultAggTestCase` 消费传入的 `SessionContext` 与 `String`，返回独占的 `AggTestCase`；`Columns` 返回独立 vector；格式化只借用用例。若调用方需要多个变体，应分别构造用例或显式克隆 `SessionContext`，而不是假设这里存在共享、同步或自动清理机制。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/executor/internal/testutil/agg.go`：结构体字段集合与含义一致；`Columns` 保持 Double 聚合列和 LongLong 分组列；`Display` 对应 Go 的 `String()`，输出字段顺序及 `%v` 风格布尔/整数文本一致；`DefaultAggTestCase` 对应 Go 同名构造器，默认 `sum`、NDV 1000、非 DISTINCT、1000 万行、并发 4、已排序。

实现层面的差异包括：

- Go 保存 `sessionctx.Context` 接口并返回 `*AggTestCase`；Rust 保存具体 `SessionContext`，按值返回 `AggTestCase`。
- Go 的规模字段是 `int`；Rust 使用 `usize`。
- Go 使用 `ast.AggFuncSum` 常量；Rust 直接保存字符串字面量 `"sum"`。
- Go `Columns` 返回真实的 `*expression.Column` 与 `FieldType`；Rust 返回测试工具自己的 `ColumnDef`，是更轻量的 schema 描述。
- Go 直接设置会话变量；Rust 将相同动作集中在 `resetChunkSizes`。

这些差异说明 Rust 当前实现对“测试用例参数契约”完成了对齐，但不能据此声称 Go 聚合基准和执行器构建链已经在 Rust 侧完整接线。

## 扩展指南

- 新增聚合维度时，优先扩展 `AggTestCase` 和 `DefaultAggTestCase`，并同步 `Display`，确保基准名称包含会影响行为或性能的参数。同步修改 Go 对照只应基于明确的移植目标，不能自行制造两端契约差异。
- 改变列布局或类型时修改 `AggTestCase::Columns`，同时审查所有按位置取列的消费者；尤其要核对 Go `buildAggExecutor` 中索引 0/1 的聚合参数和 group-by 约定。若 Rust 侧新增执行器消费测试，应放在独立 `*_test.rs` 文件，不把测试嵌入 `agg.rs`。
- 改变默认值时同步更新 `pkg/executor/internal/testutil/testutil_test.rs::operator_cases_match_go_defaults`，并核对 Go `agg.go::DefaultAggTestCase` 与 `pkg/executor/benchmark_test.go` 的参数扫描，避免默认标签、数据规模和执行行为漂移。
- 若新增 `ExecType` 或聚合函数，必须在实际消费方增加构建与错误处理分支；仅允许字符串进入 `AggTestCase` 不等于功能可用。
- 性能风险主要来自默认 `Rows`、`GroupByNDV` 和 `Concurrency` 的变化，它们会显著改变基准成本和内存压力；兼容风险主要来自 `Display` 文本、字段公开 API 和固定列位置的变化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 `agg.rs` 已被索引为含 5 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/executor/internal/testutil/agg.rs`：核对 `AggTestCase`、`Columns`、`Display::fmt`、`DefaultAggTestCase` 的完整实现。
- RustCodeGraph `query DefaultAggTestCase`、`query AggTestCase`、`query resetChunkSizes`：核对 Rust/Go 同名符号和下游重置函数；`explore` 识别到 Rust 测试 `operator_cases_match_go_defaults` 对默认构造器的调用。
- 已读取 `pkg/executor/internal/testutil/Cargo.toml` 与 `lib.rs`，核对 crate 名称、Go 包映射、依赖边界、模块声明和公开再导出。
- 已读取 `pkg/executor/internal/testutil/agg.go`，逐字段核对 Go 数据结构、schema、字符串格式和默认值。
- 已读取 `pkg/executor/internal/testutil/testutil.rs` 中 `FieldKind`、`ColumnDef`、`SessionContext`、默认 Chunk 常量和 `resetChunkSizes` 定义。
- 已读取独立 Rust 测试 `pkg/executor/internal/testutil/testutil_test.rs`，确认默认构造器及格式化输出的现有回归覆盖；同目录没有独立同名 `agg_test.rs`。
- 已读取 Go 消费方 `pkg/executor/benchmark_test.go` 的 `buildAggExecutor`、`benchmarkAggExecWithCase` 及聚合基准参数扫描，确认字段的实际消费语义。
- 本任务为纯文档分析，按计划不运行 Cargo；结构检查用于确认目标文件存在且恰有 11 个固定二级标题。
