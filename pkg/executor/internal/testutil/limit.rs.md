# `pkg/executor/internal/testutil/limit.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-internal-testutil`，由同目录的 [`lib.rs`](lib.rs) 以 `pub mod limit` 声明，并通过 `pub use limit::*` 重新导出。它不是 Limit 执行器本身，而是执行器测试基础设施中的参数模型：用 `LimitCase` 描述一次 Limit 测试或基准所需的输入规模、截取区间、列裁剪开关和会话上下文，并由 `DefaultLimitTestCase` 提供与 Go 版本一致的默认场景。

[`Cargo.toml`](Cargo.toml) 将该 crate 映射到 Go 包 `pkg/executor/internal/testutil`，常规依赖只有 `astersql-executor-internal-exec`；本文件实际使用的 `SessionContext`、`MemoryTracker`、`ColumnDef` 和 `FieldKind` 都来自同 crate 的 [`testutil.rs`](testutil.rs)。仓库根 `Cargo.toml` 将该 crate 纳入 workspace。当前 Rust 代码搜索只发现同 crate 回归测试直接调用 `DefaultLimitTestCase`，没有发现 Rust 版 Limit 基准消费 `LimitCase`；完整消费路径仍可在 Go 的 `pkg/executor/benchmark_test.go` 中看到。

## 核心职责

- `LimitCase` 集中保存 Limit 场景的测试数据，避免调用方分别维护行数、`OFFSET`、`COUNT`、schema 使用位图和投影策略。
- `LimitCase::Columns` 构造固定的两列测试 schema，列下标依次为 `0`、`1`，类型均为 `FieldKind::LongLong`。
- `Display for LimitCase` 生成稳定的场景标签，供测试断言或基准名称使用；字符串只包含行数、偏移、数量和内联投影状态，不包含上下文与列位图。
- `DefaultLimitTestCase` 规范化会话的 Chunk 参数和语句内存跟踪器，再返回一组与 Go 默认值相同的 Limit 参数。

该文件只描述和构造测试用例，不读取数据、不执行 `OFFSET`/`COUNT`、不实现列裁剪，也不创建执行器。

## 主要符号

### `pub struct LimitCase`

- `Ctx: SessionContext`：用例拥有的测试会话上下文；构造函数按值接收并返回该上下文。
- `ChildUsedSchema: Vec<bool>`：按子节点 schema 顺序标记上层需要的列。默认 `[false, true]` 表示只使用第二列。位图的长度与 schema 的匹配由消费者负责，本类型不校验。
- `Rows: usize`：子节点输入行数，默认 `30_000`。
- `Offset: usize`：需跳过的行数，默认 `10_000`。
- `Count: usize`：最多输出的行数，默认 `10_000`。
- `UsingInlineProjection: bool`：是否让 Limit 内部完成列裁剪；默认关闭。

字段与方法均沿用 Go 风格命名；crate 根的 `#![allow(non_snake_case)]` 使这些名字可直接用于 Rust 迁移代码。

### `pub fn LimitCase::Columns(&self) -> Vec<ColumnDef>`

每次调用都新建一个两元素 `Vec`，两个 `ColumnDef` 通过 `ColumnDef::new` 创建，因而除下标与 `LongLong` 类型外，还继承 `ColumnDef::new` 的默认属性：可空且非无符号。方法不依赖实例字段，保留 `&self` 是为了对应 Go 的实例方法接口。

### `impl Display for LimitCase`

输出格式固定为 `(rows:{}, offset:{}, count:{}, inline_projection:{})`。Rust 的布尔格式为小写 `true`/`false`，与 Go `%v` 的结果一致。`ChildUsedSchema` 和 `Ctx` 不进入标签，因此仅靠该字符串不能区分不同列位图或会话设置。

### `pub fn DefaultLimitTestCase(mut ctx: SessionContext) -> LimitCase`

该函数取得 `SessionContext` 所有权，先调用 `resetChunkSizes`，再用 `MemoryTracker { limit: -1, attached: false }` 替换语句级跟踪器，最后组装默认 `LimitCase`。返回值是实体而非指针；调用方可以直接修改其公开字段构造变体。

## 执行流程

1. 调用方向 `DefaultLimitTestCase` 传入测试 `SessionContext`。
2. `resetChunkSizes` 将 `ctx.vars.init_chunk_size` 和 `ctx.vars.max_chunk_size` 分别恢复为 `DEF_INIT_CHUNK_SIZE`（32）与 `DEF_MAX_CHUNK_SIZE`（1024）。
3. 函数覆盖 `ctx.vars.statement_memory_tracker`：`limit = -1` 表示不限制语句级内存，`attached = false` 表示未挂接到其他跟踪器；普通的 `ctx.vars.memory_tracker` 保持传入值不变。
4. 函数构造默认场景：30,000 行输入，跳过 10,000 行，最多取 10,000 行，只使用第二列，并关闭内联投影。
5. 当前 Rust 直接调用者 `operator_cases_match_go_defaults` 读取内存跟踪器并断言 `Display` 文本。Go 的 `BenchmarkLimitExec` 则展示了这些参数的完整预期消费方式：`Rows` 构造数据源，`Offset` 与 `Count` 形成 Limit 的 `[begin, end)` 范围，`ChildUsedSchema` 决定保留列，`UsingInlineProjection` 决定由 Limit 内部裁剪还是外接 Projection。

## 数据与状态

`LimitCase` 是纯拥有型配置对象：`Ctx`、布尔位图和数值均存放在结构体自身，不借用外部数据。`Columns` 返回新分配的列定义，调用方修改返回的 `Vec` 不会回写用例。

默认构造会有意改写传入上下文中的两类状态：Chunk 容量和语句内存跟踪器。它不会修改普通内存跟踪器，也不会验证 `Offset + Count` 是否溢出或是否超过 `Rows`。因此 `Rows`、`Offset`、`Count` 是场景描述而非已验证的不变量；真正的截断、空结果和溢出策略属于消费这些参数的执行器或测试装配代码。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 声明并公开重导出本模块，因此依赖 crate 可从 crate 根取得 `LimitCase` 与 `DefaultLimitTestCase`。
- RustCodeGraph 将 `pkg/executor/internal/testutil/testutil_test.rs::operator_cases_match_go_defaults` 标为 `DefaultLimitTestCase` 的调用者；全仓 Rust 文本搜索未发现其他直接调用者。
- Go 对照调用链为 `pkg/executor/benchmark_test.go::BenchmarkLimitExec` → `benchmarkLimitExec`，其中前者创建默认用例并分别测试内联投影关闭/开启，后者消费所有字段并驱动 `LimitExec`。

下游关系：

- `DefaultLimitTestCase` 调用 [`testutil.rs`](testutil.rs) 的 `resetChunkSizes`，并构造其中定义的 `MemoryTracker`。
- `LimitCase::Columns` 调用 `ColumnDef::new`，并使用 `FieldKind::LongLong`。
- `Display::fmt` 仅依赖标准库 `std::fmt::{Display, Formatter}` 和 `write!`。

RustCodeGraph 没有显示本文件调用实际 Limit 执行器；代码搜索也没有找到 Rust 端等价于 Go `benchmarkLimitExec` 的装配。因此本文件当前是已测试的数据模型，而不是已经接入 Rust Limit 执行链的证据。

## 错误处理与边界

本文件没有 `Result`、显式错误类型、`panic!` 或输入校验。`Columns` 和默认构造在正常内存分配条件下直接返回，`Display::fmt` 只透传格式化器可能产生的 `std::fmt::Result`。

需要由扩展者或消费者注意的边界包括：

- `Offset`、`Count` 与 `Rows` 允许任意 `usize` 组合，包括零值、偏移超过输入和 `Offset + Count` 溢出的组合；本文件不定义处理结果。
- `ChildUsedSchema` 可为空、过短或过长，也不保证至少有一列为 `true`。
- `Columns` 固定返回两列，而公开字段允许调用方替换为任意长度的位图；两者的一致性不是类型系统保证的。
- 默认构造覆盖调用方原有的语句内存跟踪器。如果测试需要保留已有 tracker，应手工构造 `LimitCase` 或在默认构造后明确恢复设置。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、文件句柄或网络资源。`DefaultLimitTestCase` 通过所有权转移接收并返回 `SessionContext`，不会与调用方共享该上下文；`LimitCase` 本身也没有内部共享或同步机制。是否可跨线程移动或共享取决于字段类型自动推导出的 `Send`/`Sync`，本文件没有显式承诺这些 trait。

内存资源仅包括 `ChildUsedSchema` 和 `Columns` 返回值的 `Vec` 分配，随拥有者离开作用域自动释放。`MemoryTracker` 在这里是测试用状态值，不会挂接到真实跟踪树，默认的 `attached = false` 正是回归测试锁定的生命周期状态。

## 与 Go 版本的对应关系

直接对照文件是 [`limit.go`](limit.go)：Rust 的 `LimitCase` 六个字段、`Columns`、字符串表示和默认构造均逐项对应 Go 实现。

- Go 的 `sessionctx.Context` 接口在 Rust 测试工具中收敛为拥有型 `SessionContext`；Go 返回 `*LimitCase`，Rust 返回 `LimitCase` 值。
- Go `Columns` 返回两个 `*expression.Column`，其 `RetType` 是 MySQL `TypeLonglong`；Rust 返回两个简化的 `ColumnDef`，类型为 `FieldKind::LongLong`。Rust 对象不是生产 `expression.Column` 的完整替代。
- Go 通过 `vardef.DefInitChunkSize`、`vardef.DefMaxChunkSize` 和 `memory.NewTracker(-1, -1)` 初始化真实会话状态；Rust 通过测试工具的 `resetChunkSizes` 与简化 `MemoryTracker` 表达同样的默认契约。
- 默认数值、列位图、投影开关和字符串格式保持一致。`testutil_test.rs::operator_cases_match_go_defaults` 已覆盖内存限制、挂接状态和显示文本，但没有逐项断言 `Rows`、`Offset`、`Count`、`ChildUsedSchema`、Chunk 大小或 `Columns` 的 schema。
- Go 的 `BenchmarkLimitExec` 证明该类型最初服务于真实 Limit/Projection 基准装配；截至本次检索，Rust 侧没有对应消费代码，因此不能宣称 Rust Limit 基准已经迁移完成。

## 扩展指南

- 新增用例参数时，先修改 `LimitCase`，再同步 `DefaultLimitTestCase`、`Display`（若该字段影响场景标识）和 [`limit.go`](limit.go) 的语义对照；同时检查所有结构体字面量是否需要补字段。
- 改变 schema 时，应修改 `LimitCase::Columns`，并保证 `ChildUsedSchema` 默认长度和下标语义同步。建议在独立的 [`testutil_test.rs`](testutil_test.rs) 中补充列数、下标、类型和位图一致性的测试，不要把测试内嵌到 `limit.rs`。
- 增加零行、零 `Count`、偏移越界或大数溢出场景时，应把执行行为测试放在真正的 Limit 执行器独立测试中；本文件最多提供构造数据，不应伪造执行语义。
- 若移植 Go `BenchmarkLimitExec`，应分别覆盖 `UsingInlineProjection = false/true`，验证位图 `[false, true]` 在外接 Projection 和 Limit 内联裁剪两条路径得到一致 schema 与行数，并明确处理 `Offset + Count` 的安全加法。
- 更改默认会话设置会影响共享测试夹具。需同时验证 Chunk 默认值、语句 tracker 与普通 tracker 的差异，避免把 `statement_memory_tracker` 和 `memory_tracker` 混为一谈。
- 性能风险主要来自将 `Columns` 或位图处理放入高频循环造成重复分配；兼容风险来自改变稳定的 `Display` 文本、默认规模或 Go/Rust 字段语义。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标目录和 `limit.rs` 均已索引。
- RustCodeGraph `explore "pkg/executor/internal/testutil/limit.rs LimitTestSuite LimitExecutor"`：读取目标文件全貌，并报告 `DefaultLimitTestCase` 的调用者为 `testutil_test.rs::operator_cases_match_go_defaults`。
- RustCodeGraph `query LimitCase`、`query DefaultLimitTestCase` 和 `node pkg/executor/internal/testutil/limit.rs::DefaultLimitTestCase`：核对 Rust/Go 同名符号、默认构造源码及调用轨迹。图的单独 `callers`/`callees` 命令未返回额外边，因此又以全仓 `rg` 核对直接引用。
- 源文件：[`limit.rs`](limit.rs)；crate 入口与边界：[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)；下游测试工具定义：[`testutil.rs`](testutil.rs)。
- Go 对照与真实消费：[`limit.go`](limit.go)、`pkg/executor/benchmark_test.go` 中的 `benchmarkLimitExec` 和 `BenchmarkLimitExec`。
- Rust 独立测试：[`testutil_test.rs`](testutil_test.rs) 中的 `operator_cases_match_go_defaults`。同目录没有 `limit_test.rs` 或 `limit_tests.rs`，测试由 `lib.rs` 的 `#[cfg(test)]` 模块装配。
- 人工复核结论：该文件存在是为了集中描述可复用 Limit 测试场景；当前 Rust 运行流程止于默认对象构造和对齐断言，Go 基准展示了完整预期消费；安全扩展需同步结构体、默认值、显示格式、独立测试及 Go 对照，并避免把未迁移的 Rust 执行链写成现状。

