# `pkg/executor/internal/testutil/sort.rs`

## 文件定位

本文件属于 `astersql-executor-internal-testutil` crate，是执行器内部测试工具中的 Sort 用例参数模块。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod sort` 装配本模块，并用 `pub use sort::*` 将其公开符号再导出。它不是 SQL 请求执行链中的 Sort 算子实现；真正的 Go Sort 执行器位于 `pkg/executor/sortexec`，Rust Sort 的行为测试也位于该目录。本文件只提供测试场景的 schema、数据分布、排序列和内存限制配置。

[`Cargo.toml`](Cargo.toml) 将该目录定义为独立 library crate，crate 根为 `lib.rs`，移植元数据指向 Go 包 `pkg/executor/internal/testutil`。目标文件自身只引用同 crate 的 `testutil` 模块和标准库格式化接口，不直接调用 `astersql-executor-internal-exec`；后者是该 crate 声明的常规依赖，由通用测试工具使用。

## 核心职责

文件围绕 `SortCase` 完成三件事：

1. 用一个可修改的值对象汇总测试会话 `Ctx`、临时文件前缀 `FileNamePrefixForTest`、排序列 `OrderByIdx`、各列 NDV `Ndvs` 和输入行数 `Rows`。
2. 用 `SortCase::Columns` 固定提供两列 `LongLong` schema，并用 `Display::fmt` 生成与 Go `String()` 一致的用例名称，便于测试或 benchmark 标识场景。
3. 用 `DefaultSortTestCase` 与 `SortTestCaseWithMemoryLimit` 构造正常内存和受限内存两类默认场景。两者都重置 Chunk 大小；后者额外设置会话级与语句级内存限制，用于让消费者选择 spill 路径。

这里没有排序、行生成、内存计量或临时文件读写逻辑。`Ndvs`、`OrderByIdx` 和文件名前缀只有被下游数据源或 Sort/TopN 测试消费后才会产生行为。

## 主要符号

- `pub struct SortCase`：公开测试参数集合。`Ctx: SessionContext` 按值持有简化会话；`FileNamePrefixForTest: String` 用于下游隔离或检查 spill 文件；`OrderByIdx: Vec<usize>` 保存 schema 下标；`Ndvs: Vec<i32>` 保存每列不同值数量约束；`Rows: usize` 保存输入规模。结构体字段均公开，调用者可派生场景。
- `SortCase::Columns(&self) -> Vec<ColumnDef>`：每次调用新建两项 `ColumnDef`，下标依次为 0、1，类型均为 `FieldKind::LongLong`。`ColumnDef::new` 同时令列默认可空、非 unsigned（见 `testutil.rs` 的 `ColumnDef` 实现）。该方法不读取 `self` 的字段，因此当前所有用例 schema 固定为两列。
- `impl Display for SortCase::fmt`：输出 `(rows:<Rows>, orderBy:[...], ndvs: [...])`。两个向量先逐项转成十进制文本，再以单个空格连接；这刻意复现 Go 切片的展示形式而不是 Rust 的逗号分隔 Debug 形式。
- `DefaultSortTestCase(mut ctx: SessionContext) -> SortCase`：把 Chunk 初始/最大容量恢复为默认值，把语句级 tracker 设为 `limit = -1, attached = false`，随后返回 300,000 行、按第 0 和第 1 列排序、NDV `[0, 0]`、空文件前缀的用例。
- `SortTestCaseWithMemoryLimit(mut ctx: SessionContext, bytes_limit: i64) -> SortCase`：同样重置 Chunk 大小，把会话级 tracker 设为给定限制且未挂接，把语句级 tracker 设为同一限制且标记 `attached = true`，然后返回与默认构造器相同的数据规模和排序参数。

文件没有模块级常量、trait、枚举、条件编译项或私有辅助函数。命名保留 Go 风格；crate 根通过 `#![allow(..., non_snake_case, ...)]` 接受这些公开名称。

## 执行流程

默认场景的流程是：调用者准备 `SessionContext` → `DefaultSortTestCase` 调用 `resetChunkSizes` → 替换语句级 `MemoryTracker` 为无限制状态 → 填入默认行数、排序列和 NDV → 返回拥有该上下文的 `SortCase`。之后消费者可调用 `Columns` 构造 schema，使用 `Rows`/`Ndvs` 构造 Mock 输入，按 `OrderByIdx` 生成排序表达式，并以 `Display` 文本命名派生场景。

受限内存场景的流程相同，但 `SortTestCaseWithMemoryLimit` 会先把会话级和语句级 tracker 的 `limit` 都设为 `bytes_limit`，并仅将语句级 tracker 标为已挂接。这个函数本身不会消耗内存、判断超限或执行 spill；这些步骤必须由实际执行器及测试夹具完成。

Go 的 `pkg/executor/sortexec/benchmark_test.go` 展示了完整消费路径：用 `Columns`、`Rows`、`Ctx`、`Ndvs` 构造 Mock 数据源，用 `OrderByIdx` 构造 `ByItems`，随后执行 `Open`、反复 `Next` 到空 Chunk、最后 `Close`。`sort_spill_test.go` 还会把 `FileNamePrefixForTest` 传入 `SortExec`。这是 Go 对照行为的直接证据，不代表 Rust 本文件已接入这些 Go 调用点。

## 数据与状态

`SortCase` 是普通拥有型结构体，不保存引用或共享句柄。构造器会取得 `SessionContext` 的所有权并修改其中的 `SessionVars`，原调用者不能在不克隆的情况下继续使用传入值。`Columns` 每次返回新的 `Vec<ColumnDef>`，修改返回值不会回写用例。

默认不变量为：两列 schema、`Rows = 300_000`、`OrderByIdx = [0, 1]`、`Ndvs = [0, 0]`、空文件前缀。通用测试工具在 `MockDataSourceParameters` 上定义 `Ndvs` 的含义：0 表示逐行随机/回调生成，-1/-2 表示使用预置数据，正数表示从有限不同值池抽样；本文件仅设置默认的两个 0，不校验向量长度或取值。

`MemoryTracker` 是测试替身，只有 `limit: i64` 和 `attached: bool` 两个字段。`-1` 表示不限制；`attached` 记录挂接状态，而不是实际维护父子 tracker 图。`resetChunkSizes` 把 `init_chunk_size` 和 `max_chunk_size` 恢复为 `DEF_INIT_CHUNK_SIZE`、`DEF_MAX_CHUNK_SIZE`。

## 依赖与调用关系

直接下游依赖均来自同 crate 的 [`testutil.rs`](testutil.rs)：`ColumnDef::new` 与 `FieldKind::LongLong` 形成 schema，`resetChunkSizes` 修改会话变量，`MemoryTracker` 表示内存策略，`SessionContext` 承载这些状态。标准库的 `Display`、`Formatter` 和 `write!` 负责文本表示。

模块上游由 [`lib.rs`](lib.rs) 装配和再导出。RustCodeGraph 将 [`testutil_test.rs`](testutil_test.rs) 标为本文件的直接使用者；仓库文本搜索进一步确认 Rust 侧只有 `sort_case_string_matches_go_format` 调用 `DefaultSortTestCase`，以及 `operator_cases_match_go_defaults` 调用 `SortTestCaseWithMemoryLimit`。RustCodeGraph 对两个构造器未识别出可解析的 callees，细粒度依赖由目标源码和 `testutil.rs` 的定义补充核验。

Go 对照调用者包括 `pkg/executor/sortexec/benchmark_test.go` 和多项 `sort_spill_test.go`、`parallel_sort_spill_test.go`、`topn_spill_test.go` 测试。需要特别区分：`pkg/executor/sortexec/benchmark_test.rs` 声明了自己的私有 `SortCase`，未引用本 crate 的 `SortCase` 或两个构造器，因此当前 Rust 文件主要是移植契约和独立回归测试工具，尚不是 Rust Sort benchmark 的公共参数入口。

## 错误处理与边界

所有公开函数都返回普通值，不返回 `Result`，也没有显式 panic 分支。`Display::fmt` 只传播 `write!` 返回的 `std::fmt::Result`。内存限制参数不在本层校验：负数、0 或极小正数都会原样写入 tracker；是否有效由消费者解释。

`Columns` 永远只生成下标 0、1。`OrderByIdx` 是公开字段且没有边界检查，消费者若写入大于等于 2 的下标，再以该值索引 `Columns()`，可能在消费者处越界 panic。`Ndvs` 也不强制与列数相等。`Rows` 可以被改为 0，文件前缀可以为空；这些都是允许构造的状态，本文件不赋予额外保证。

本文件注释提及 spill，但 `SortTestCaseWithMemoryLimit` 仅布置触发条件。不能据此推断临时文件一定创建、一定清理，或具体限制一定触发 spill；相关断言应放在实际 Sort/TopN 执行器的独立测试中。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、事务或网络资源。`SortCase` 及其中的向量和字符串遵循 Rust 所有权自动释放；构造器返回后没有后台生命周期。`SessionContext` 和 `MemoryTracker` 也是可克隆的普通数据结构，不包含原子计数或同步机制，因此 `attached` 只是状态快照。

临时文件前缀的生命周期由下游执行器决定；本文件既不创建也不清理文件。新增并发测试时，应为每个测试实例设置唯一的 `FileNamePrefixForTest`，并在执行器测试中验证 Close/清理行为，而不能把资源管理责任放入 `SortCase`。

## 与 Go 版本的对应关系

直接对照文件是 [`sort.go`](sort.go)。字段、两列 LongLong schema、300,000 默认行、排序下标 `[0, 1]`、NDV `[0, 0]`、Chunk 大小重置和字符串格式均保持 Go 意图。Rust 使用 `Vec<usize>`/`usize` 表示索引和行数、`Vec<i32>` 表示 NDV，并按值返回 `SortCase`；Go 使用 `[]int`/`int` 且返回 `*SortCase`。

Go 默认构造器通过 `memory.NewTracker(-1, -1)` 新建语句 tracker；Rust 用 `MemoryTracker { limit: -1, attached: false }` 表达同一测试所需状态。Go 受限构造器分别新建会话与语句 tracker，并执行 `StmtCtx.MemTracker.AttachTo(SessionVars.MemTracker)`；Rust 将两者 limit 设为相同值，并用 `statement_memory_tracker.attached = true` 模拟挂接结果，没有实现真实 tracker 层级、消费传播或 OOM action。

Go 的 `Columns` 返回 `[]*expression.Column`，字段类型来自 MySQL `TypeLonglong`；Rust 返回本地 `Vec<ColumnDef>`，`FieldKind::LongLong` 是测试替身。因而这里验证的是参数与类型语义对齐，不是生产 expression/sessionctx/memory 类型的完整替代。

## 扩展指南

增加 Sort 默认参数时，优先修改 `SortCase` 及两个构造器，并同步检查 `Display` 是否属于稳定用例标识；若改变 schema，应同时调整 `Columns`、`OrderByIdx` 默认值以及所有按下标取列的消费者。改变 NDV 语义时还必须核对 `testutil.rs` 中 `MockDataSourceParameters` 的生成规则，避免只改参数而未改数据分布。

任何行为修改都应同步 Go `sort.go` 的对应语义，或在文档和测试中明确记录有意差异。Rust 回归应继续放在独立的 [`testutil_test.rs`](testutil_test.rs)，不要嵌入 `sort.rs`；至少覆盖默认 Chunk 大小、默认 tracker、受限 tracker 两级 limit/挂接状态、schema，以及 Display 格式。若要让 Rust Sort benchmark/spill 测试复用此类型，还需显式接线目标 crate 依赖和类型转换，不能仅假定同名局部 `SortCase` 已经兼容。

兼容性风险主要是 Display 文本影响 benchmark 名称、字段类型/排序下标影响比较语义、tracker 模型差异掩盖真实 spill 生命周期。性能风险不在构造器本身，而在默认 `Rows = 300_000` 及 NDV/排序列组合造成的测试工作量；派生用例应避免无意扩大矩阵。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标目录已收录；`files --filter pkg/executor/internal/testutil` 列出 `sort.rs`、Go 对照、crate 入口和独立测试；`node --file pkg/executor/internal/testutil/sort.rs` 核对 99 行源码及 6 个符号；`query` 核对 `SortCase`、`DefaultSortTestCase`、`SortTestCaseWithMemoryLimit` 与 `resetChunkSizes` 定义；`callers/callees` 查询未给构造器解析出 callees，文件级关系指向 `testutil_test.rs`。
- 已读 Rust 文件：`pkg/executor/internal/testutil/sort.rs`、`lib.rs`、`testutil.rs` 的 `ColumnDef`/`MemoryTracker`/`SessionContext`/`resetChunkSizes` 区段、`testutil_test.rs`，以及 `pkg/executor/sortexec/benchmark_test.rs` 的局部用例定义和执行流程。
- 已读配置与 Go 对照：`pkg/executor/internal/testutil/Cargo.toml`、`sort.go`、`pkg/executor/sortexec/benchmark_test.go`、`sort_spill_test.go` 的构造辅助流程。
- 仓库搜索：`rg` 确认 Rust 构造器只由 `testutil_test.rs` 直接调用；Go 构造器由 Sort benchmark 调用，Go `SortCase` 字段还被 Sort/Parallel Sort/TopN spill 测试消费。
- 测试证据：`sort_case_string_matches_go_format` 锁定 Display 输出；`operator_cases_match_go_defaults` 锁定受限场景中两个 limit 和语句 tracker 的挂接标记。按任务要求这是纯文档分析，未运行 Cargo。
