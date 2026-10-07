# `pkg/executor/internal/testutil/window.rs`

## 文件定位

本文件属于 `astersql-executor-internal-testutil` crate，是执行器内部测试工具中的窗口算子用例模型。crate 入口 `pkg/executor/internal/testutil/lib.rs` 通过 `pub mod window` 声明模块，并用 `pub use window::*` 再导出本文件的公开项；crate 边界和 Go 包映射由 `pkg/executor/internal/testutil/Cargo.toml` 定义，其中 `package.metadata.porting.go-package` 指向 `pkg/executor/internal/testutil`。

它不是窗口执行器实现。实际 Rust 窗口执行逻辑位于 `pkg/executor/windows/`，而本文件只保存测试/基准所需的参数、默认值和展示文本。当前 Rust 侧可确认的直接消费者是同 crate 的独立测试 `pkg/executor/internal/testutil/testutil_test.rs`；RustCodeGraph 虽把文件关联到 `pkg/executor/benchmark_test.rs`，但精确符号搜索没有发现后者使用本文件的 `WindowTestCase`、`DefaultWindowTestCase` 或简化 `WindowFrame`。该基准文件使用的是 `astersql_executor_windows::window::WindowFrame`，两者不可混同。

## 核心职责

- `WindowFrame` 保存一对有符号边界偏移，作为测试用的轻量窗口帧描述。它没有帧类型、无界标志或 `CURRENT ROW` 等完整执行语义，解释工作留给未来的适配/执行层。
- `WindowTestCase` 聚合一组窗口测试参数：会话上下文、函数及帧、固定输入 schema、数据规模、分区 NDV、并发度、流水线开关和输入有序性。
- `WindowTestCase` 的 `Display` 实现产生与 Go 基准命名兼容的稳定摘要，便于测试或基准子用例标识。
- `DefaultWindowTestCase` 建立与 Go 默认用例一致的起点，并先把传入会话的 Chunk 初始容量和最大容量恢复成测试工具默认值。

本文件不生成输入行、不构建物理计划、不创建窗口执行器，也不执行分区、排序或帧计算。`Rows`、`Ndv`、`Concurrency`、`Pipelined` 等字段只是数据，当前文件不会验证或消费它们。

## 主要符号

- `pub struct WindowFrame { pub start: i64, pub end: i64 }`：轻量帧边界值对象，派生 `Clone`、`Debug`、`Default`、`Eq` 和 `PartialEq`。默认值为两个偏移均为 `0`；源码没有进一步规定该组合对应哪一种 SQL 帧。
- `pub struct WindowTestCase`：公开、可变字段组成的用例参数容器。`Ctx` 是 crate 内测试工具定义的 `SessionContext`；`Frame` 是可选的上述轻量帧；`Columns` 是 `Vec<ColumnDef>`；其余标量描述窗口函数和数据/执行矩阵。
- `impl Display for WindowTestCase::fmt`：按 `func`、首列类型、函数数、NDV、行数、有序性、并发度、流水线标志的固定顺序格式化。类型文本来自 `self.Columns[0].kind` 的 `Display`。
- `pub fn DefaultWindowTestCase(mut ctx: SessionContext) -> WindowTestCase`：取得会话所有权、重置 Chunk 大小，然后按 Go 默认值构造并按值返回用例。
- `crate::testutil::{ColumnDef, FieldKind, SessionContext, resetChunkSizes}`：本文件唯一的业务依赖集合；其中 `resetChunkSizes` 写入 `DEF_INIT_CHUNK_SIZE` 和 `DEF_MAX_CHUNK_SIZE`。

命名保留了 Go 导出成员风格（例如 `Ctx`、`NumFunc`、`DefaultWindowTestCase`）。`lib.rs` 在 crate 级允许 `non_snake_case`，说明这是有意的移植兼容接口，而非新的 Rust 命名约定。

## 执行流程

默认构造流程如下：

1. 调用者把一个 `SessionContext` 传给 `DefaultWindowTestCase`；函数取得该值的所有权。
2. `resetChunkSizes(&mut ctx)` 将 `ctx.vars.init_chunk_size` 和 `ctx.vars.max_chunk_size` 分别恢复为 `DEF_INIT_CHUNK_SIZE`、`DEF_MAX_CHUNK_SIZE`，其他会话字段保持原值。
3. 函数创建默认用例：函数为 `row_number`，函数数为 `1`，无显式帧，NDV 为 `1000`，行数为 `10_000_000`，并发度为 `1`，流水线标志为 `0`，输入标记为已排序。
4. `RawDataSmall` 被设为 16 个 `x`；`Columns` 被设为索引与类型依次为 `(0, Double)`、`(1, LongLong)`、`(2, VarString)`、`(3, LongLong)` 的四列 schema。
5. 调用者可修改公开字段形成矩阵用例。Go 端 `pkg/executor/benchmark_test.go` 展示了预期用法：调整行数、NDV、并发度、排序状态、函数、函数数、帧和流水线标志，再构建数据源与窗口执行器。
6. 当用例被格式化时，`fmt` 读取首列类型并生成摘要；格式化本身不触发执行。

当前 Rust 测试流程更窄：`testutil_test.rs` 只构造默认值，校验摘要、16 字符串和首列 `Double` 类型。尚未发现 Rust 生产/基准路径把该结构转换为 `PhysicalWindowPlan`。

## 数据与状态

`WindowTestCase` 是纯参数快照，不含内部可变性、缓存或运行进度。字段语义如下：

- `Ctx` 保存测试会话变量；构造时只强制重置两个 Chunk 大小字段。
- `Frame: Option<WindowFrame>` 用 `None` 表示不提供显式帧；默认即为 `None`。
- `WindowFunc` 和 `RawDataSmall` 拥有各自的 `String`；默认分别为 `row_number` 与 16 个 `x`。
- `Columns` 拥有四个 `ColumnDef`。首列是聚合/窗口值列，第二列与 Go 固定 schema 中的分区键对应，第三列承载小字符串，第四列是额外整数列。
- `NumFunc`、`Ndv`、`Rows`、`Concurrency` 使用 `usize`，因此不能表示负数，但允许 `0`；本文件没有拒绝零值。
- `Pipelined` 使用 `i32` 而不是 `bool`，以保持 Go 系统变量式开关的形状；注释只承诺非零值进入相关路径，具体合法范围由消费者决定。
- `DataSourceSorted` 是调用者对输入有序性的声明，不会由本文件检查。

`WindowFrame` 的 `start`/`end` 使用 `i64`，可以表达正负偏移，但它与 Go 的 `logicalop.WindowFrame` 不同：后者还能表达 `ROWS`/`RANGE` 类型、无界边界、当前行以及方向/数值。本文件没有编码这些状态。

## 依赖与调用关系

上游模块关系是 `lib.rs -> window.rs`：入口公开声明并再导出所有符号。RustCodeGraph 的文件查询显示本目录共 12 个已索引 Go/Rust 文件，并识别出本文件 5 个符号。

已验证的 Rust 调用边是：

- `testutil_test.rs::window_case_string_uses_go_field_type_name -> DefaultWindowTestCase -> resetChunkSizes`，随后调用 `WindowTestCase::fmt` 并断言完整输出。
- `testutil_test.rs::operator_cases_match_go_defaults -> DefaultWindowTestCase -> resetChunkSizes`，随后检查 `RawDataSmall` 和第一列类型。

`DefaultWindowTestCase` 的直接下游依赖是 `resetChunkSizes`、四次 `ColumnDef::new`、`String` 构造和 `Vec` 构造；没有 I/O 或执行器调用。RustCodeGraph 对精确限定符执行 `callers`/`callees` 时未返回静态边，因此上述边又用已索引源码和精确引用搜索复核。

`Cargo.toml` 说明该 crate 的常规直接依赖只有 `astersql-executor-internal-exec`；一组与 Go testutil 对应的表达式、规划、会话和类型依赖只在 `cfg(windows)` 下声明。本文件本身只引用同 crate 的 `testutil` 模块和标准库 `fmt`，不直接引用这些外部 crate。`BUILD.bazel` 则描述 Go `testutil` 包的完整依赖，不应被当成 Rust 本文件的依赖表。

## 错误处理与边界

构造函数不返回 `Result`，因为现有步骤只有确定性的内存值构造与字段赋值。它不会验证 `Rows`、`Ndv`、`NumFunc`、`Concurrency`、帧边界或列布局；无效组合只可能在未来消费者中暴露。

明确的本地边界是 `Display` 对 `self.Columns[0]` 的直接索引。如果调用者清空 `Columns` 后格式化用例，进程会 panic；默认构造器始终提供四列，因此默认路径安全。若调用者更换首列，摘要中的 `aggColType` 会随之变化。

`WindowFrame` 不检查 `start <= end`，也不区分 `ROWS` 和 `RANGE`。文档和扩展代码不得据此推断完整 SQL 窗口帧合法性。`Frame: None` 仅表示测试用例没有显式帧，实际默认帧语义应由真正的规划/执行层决定。

字符串格式属于已被测试锁定的兼容边界：字段名、顺序、标点、布尔值和 `FieldKind` 的展示形式发生变化时，会破坏 `window_case_string_uses_go_field_type_name`，也可能改变基准子用例名称。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件、网络连接或显式分配回收协议。`Concurrency` 仅是一个待消费者解释的数值；创建 `WindowTestCase` 不会启动工作线程。`SessionContext` 和 `WindowFrame` 可克隆，但 `WindowTestCase` 本身没有派生 `Clone`，调用者通常通过移动或逐字段修改来持有它。

资源生命周期完全是值所有权：传入的 `SessionContext` 被移动进构造器，原地重置 Chunk 配置后再移动进返回结构；字符串、列向量和可选帧随 `WindowTestCase` 一同释放。Go 基准中的执行器 `Open`/`Next`/`Close` 生命周期属于 `pkg/executor/benchmark_test.go` 及实际执行器，不属于本文件，当前 Rust 本文件也没有对应接线。

若未来用例被多线程共享，必须由调用方选择 `Arc`/同步策略，并确认其中各字段及真实会话上下文的线程安全契约；本文件没有提供这一保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/internal/testutil/window.go`。两端共同保留了 `WindowTestCase` 的字段集合、固定四列 schema、展示字段顺序和默认值：`row_number`、函数数 `1`、空帧、NDV `1000`、一千万行、并发度 `1`、流水线 `0`、已排序、16 个 `x`。

主要差异如下：

- Go `Ctx` 是 `sessionctx.Context` 接口，Rust 是精简的测试 `SessionContext` 值。
- Go `Frame` 指向完整 `logicalop.WindowFrame`，Rust 在本文件内定义了只有 `start`/`end` 的轻量结构；两者不是等价的完整模型。
- Go `Columns` 是表达式列指针，列类型是完整 `FieldType`；Rust 使用 `ColumnDef` 和 `FieldKind` 的简化表示。
- Go 构造器返回指针，Rust 按值返回；Go 数值字段是 `int`，Rust 多数为 `usize`。
- Go 通过 `ast.WindowFuncRowNumber` 常量设置函数名，Rust 当前直接使用字面量 `"row_number"`。
- Go 基准 `benchmarkWindowExecWithCase` 实际消费全部字段：设置系统变量、构造 mock 数据源、创建窗口执行器并执行 `Open/Next/Close`；当前 Rust 侧该类型仅由独立测试验证，Rust 的真实窗口基准另行构造 `PhysicalWindowPlan`。

因此，本文件已经对齐默认用例的表面契约，但不能据此宣称 Go 窗口基准接线或完整帧表达能力已经移植完成。

## 扩展指南

扩展默认窗口用例时，应优先修改 `WindowTestCase`、`DefaultWindowTestCase` 和 `Display::fmt` 中与新字段相关的最小位置，并同步独立测试 `pkg/executor/internal/testutil/testutil_test.rs`。若变更意在对齐 Go，先核对 `window.go` 及 `pkg/executor/benchmark_test.go::benchmarkWindowExecWithCase` 的字段消费方式，不要只复制字段名。

扩展帧能力时，不宜继续把 `start`/`end` 当成完整 SQL 模型。应先决定它是仅供测试矩阵使用的转换输入，还是要替换为/转换到 `astersql_executor_windows::window::WindowFrame`，并为 `ROWS`/`RANGE`、无界、当前行、preceding/following 和非法边界建立明确映射及独立测试。

修改展示格式时要把格式看作兼容接口，同步更新 `window_case_string_uses_go_field_type_name`，并检查基准名称消费者。修改固定 schema 时，要检查 `Display` 的首列索引、Go 数据源的四项 `Ndvs`/`Orders` 假设和任何按列位置取分区键的代码。

若要接入 Rust 窗口基准，应新增独立测试文件中的转换/执行验证，而不是把测试嵌入 `window.rs`。至少覆盖默认值到真实 `PhysicalWindowPlan` 的映射、空列格式化边界、零并发/零 NDV 的处理、流水线开关、已排序与未排序输入、多函数以及显式帧。兼容风险集中在 Go/Rust 类型表达差异；性能风险来自默认一千万行和并发矩阵，验证时应把小规模正确性测试与显式的大规模基准分开。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/internal/testutil` 列出本 crate 的 12 个 Go/Rust 文件。
- RustCodeGraph 源码节点：`window.rs`（完整 94 行）、`lib.rs`、`testutil.rs` 中 `SessionVars`/`SessionContext`/`resetChunkSizes`、`testutil_test.rs` 中两个窗口默认值测试。
- RustCodeGraph 符号查询：`WindowFrame`、`WindowTestCase`、`DefaultWindowTestCase`、`resetChunkSizes`；精确限定的 `callers`/`callees` 没有返回边，故使用索引报告的文件关系和 `rg` 精确引用结果交叉核对。
- crate/构建边界：`pkg/executor/internal/testutil/Cargo.toml`、`pkg/executor/internal/testutil/BUILD.bazel`。
- Go 对照与真实消费路径：`pkg/executor/internal/testutil/window.go`、`pkg/executor/benchmark_test.go` 第 360 行起的 `benchmarkWindowExecWithCase` 及其窗口矩阵调用点。
- Rust 相关独立测试：`pkg/executor/internal/testutil/testutil_test.rs::window_case_string_uses_go_field_type_name` 和 `operator_cases_match_go_defaults`。没有发现同名独立 `window_test.rs`；测试通过 `lib.rs` 的 `#[cfg(test)]` 模块装配。
- 人工边界复核：确认本文件没有条件编译项、I/O、错误返回或并发原语；确认 Rust 真实基准的 `WindowFrame` 来自 `astersql_executor_windows`，而非本文件。

本任务是纯文档分析，按计划不运行 Cargo。交付结构以固定 11 个二级标题的检查命令为准。
