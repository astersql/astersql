# `build/linter/exptostd/analyzer.rs`

## 文件定位

本文件位于构建期静态检查器目录 `build/linter/exptostd`，是同目录 [`analyzer.go`](./analyzer.go) 的机械迁移草稿。它试图为第三方 `github.com/ldez/exptostd` analyzer 提供一层仓库本地注册适配：暴露 analyzer，并在初始化时套用 AsterSQL/TiDB 的文件过滤配置。

当前 Rust 文件不属于任何 Cargo crate：根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员中没有 `build/linter`，该目录下也没有 `Cargo.toml`；仓库 Rust 模块搜索没有找到 `mod`、`include!` 或路径属性引用本文件。RustCodeGraph 同样报告该文件 `used by 0 files`。因此这里描述的是 Go 生产链的迁移意图，不是当前会被 Rust 构建或运行的实现。实际构建接线仍在 [`BUILD.bazel`](./BUILD.bazel)，且其 `srcs` 只有 `analyzer.go`。

## 核心职责

本文件只有两项职责：

1. `Analyzer` 延迟调用 `exptostd::NewAnalyzer()`，表达“复用第三方检查器，不在本仓库重复实现表达式转标准库函数的规则”。
2. `init()` 把该 analyzer 交给 `util::SkipAnalyzerByConfig`，表达“运行第三方检查器前，按照仓库配置筛选待分析文件”。

它不遍历 AST、不产生诊断、不读取配置文件，也不实现 exptostd 的规则。第三方规则在 Go 依赖中实现；仓库侧的配置过滤语义由 [`../util/util.go`](../util/util.go) 承担，Rust 对照草稿位于 [`../util/util.rs`](../util/util.rs)。

## 主要符号

- `pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>`：公开的惰性全局值。第一次解引用时执行闭包并调用 `exptostd::NewAnalyzer()`；后续访问复用同一实例。源文件没有条件编译项、常量、trait、结构体或 `impl`。
- `pub fn init()`：公开、无参数、无返回值的初始化适配函数，仅执行 `util::SkipAnalyzerByConfig(&Analyzer)`。Rust 不会像 Go 那样自动执行名为 `init` 的普通函数，而当前索引与模块搜索均未发现调用者。

两个名字刻意保留 Go 风格的大写/驼峰命名，以对应 [`analyzer.go`](./analyzer.go) 的 `Analyzer`、`exptostd.NewAnalyzer` 和 `util.SkipAnalyzerByConfig`，而不是采用惯常 Rust 命名。

## 执行流程

按源码表达的预期流程：

1. 某个尚不存在的 Rust 装配入口首次访问 `Analyzer`。
2. `once_cell::sync::Lazy` 执行一次初始化闭包，调用 `exptostd::NewAnalyzer()` 并保存返回的 `analysis::Analyzer`。
3. 装配入口还需显式调用 `init()`；仅加载模块不会触发它。
4. `init()` 调用 `util::SkipAnalyzerByConfig`，意图把 analyzer 原有 `Run` 回调替换为过滤包装器。
5. 按 [`../util/util.rs`](../util/util.rs) 的实现，该包装器在每次分析时复制 pass，依据 analyzer 名称及文件路径调用 `shouldRun`，用保留结果重建 `Files`，然后调用原 `Run`。

当前实际生产流程是 Go/Bazel 链：[`../../BUILD.bazel`](../../BUILD.bazel) 在 `with_nogo` 分支依赖 `//build/linter/exptostd`，该目标编译 [`analyzer.go`](./analyzer.go)；Go 包加载时自动执行其 `init()`。Rust 上述 1—5 步尚未接线，不能视为已运行行为。

## 数据与状态

本文件唯一持久状态是全局 `Analyzer`。`Lazy` 负责一次性构造并在进程生命周期内保存 analyzer；文件内没有计数器、缓存、通道、锁或事务。

配置本身不保存在此处。[`../../nogo_config.json`](../../nogo_config.json) 的 `exptostd` 项定义了三类排除路径（parser 生成文件、`external/`、`*_generated.go`）以及 `pkg/bindinfo/`、`pkg/meta/`、`pkg/owner/`、`pkg/planner/`、`pkg/statistics/`、`pkg/ttl/`、`pkg/util/` 等允许目录。过滤器应以 analyzer 的名称查找这项配置；[`../../config_test.rs`](../../config_test.rs) 仅验证该配置能加载并包含 `pkg/util/`，不直接执行本 analyzer。

## 依赖与调用关系

源码表达的直接依赖和边如下：

```text
首次访问 Analyzer -> once_cell::sync::Lazy -> exptostd::NewAnalyzer
显式调用 init -> util::SkipAnalyzerByConfig -> analyzer.Run 包装/文件过滤
```

- `once_cell::sync::Lazy`：为不能在普通 Rust `static` 初始化器中直接执行构造逻辑的场景提供延迟初始化。
- `analysis::Analyzer`：迁移草稿引用的 analyzer 类型，但本目录没有 Cargo 声明或 Rust 模块导入来解析它。
- `exptostd::NewAnalyzer`：对应 Go 第三方依赖构造器；Go 版本锁定在 `go.mod` 的 `github.com/ldez/exptostd v0.4.5`，Bazel 外部仓库声明在 [`../../../DEPS.bzl`](../../../DEPS.bzl)。没有等价 Rust Cargo 依赖。
- `util::SkipAnalyzerByConfig`：Rust 对照函数签名是 `(&mut analysis::Analyzer)`，会取得旧 `Run`、安装过滤闭包并把调用转发给旧回调。

RustCodeGraph 对目标 `init` 没有发现调用者，也没有解析出它到过滤器的调用边；源码读取给出了该直接调用，但这不构成生产接线证据。

## 错误处理与边界

本文件没有 `Result`、错误枚举、日志或恢复分支。`NewAnalyzer` 的构造结果直接保存，`init()` 也不返回状态。

当前最重要的边界是“草稿不可编译且未接线”：`Analyzer` 是不可变 `Lazy`，调用处传入 `&Analyzer`；而现有 Rust `util::SkipAnalyzerByConfig` 要求 `&mut analysis::Analyzer`。此外，`analysis`、`exptostd`、`util` 与 `once_cell` 都没有由本目录 Cargo manifest 建立解析关系。即便未来解决名称解析，也必须先设计安全的可变初始化方式，不能把当前文本视作可工作的包装器。

过滤器的运行期边界来自下游实现：它预期 analyzer 已有 `Run`，否则 [`../util/util.rs`](../util/util.rs) 会在调用时以 `expect("analysis analyzer Run should exist")` 失败；文件是否保留由 `shouldRun` 及配置正则决定。这些是下游契约，并非本文件当前已执行的行为。

## 并发与资源生命周期

`once_cell::sync::Lazy` 的意图是让构造在并发首次访问时只发生一次，并让值存活到进程结束。本文件不创建线程、异步任务、文件句柄、网络连接或显式释放流程。

不过，初始化后的过滤包装需要改变 analyzer 的 `Run` 字段，和不可变全局共享存在冲突。若未来接线，应把“构造 analyzer”与“应用一次包装”合并到同一个受同步保护的初始化阶段，或让工厂返回完成配置的拥有值；不应通过不安全全局可变状态绕过 `&mut` 约束。还应防止重复调用初始化导致多层包装同一 `Run`。

## 与 Go 版本的对应关系

[`analyzer.go`](./analyzer.go) 是当前行为基准，Rust 文件逐项保留其形状：

- Go `var Analyzer = exptostd.NewAnalyzer()` 对应 Rust `Lazy<analysis::Analyzer>`；差异是 Go 在包初始化阶段立即构造，Rust 草稿推迟到首次访问。
- Go `init()` 会由运行时自动调用，Rust `pub fn init()` 只是普通函数，必须显式接线。
- 两边都只调用一次 `SkipAnalyzerByConfig`，没有调用更宽的 `SkipAnalyzer`，因此本地适配目的仅是配置级文件筛选。
- Go 的 `*analysis.Analyzer` 可原地修改；当前 Rust 调用传共享引用，而下游要求可变引用，语义尚未移植完成。
- Go 构建已由 Bazel `go_library` 和根 `nogo` 依赖接入，第三方版本为 v0.4.5；Rust 没有 Cargo crate、依赖或模块入口。

因此，本文件准确保留了注册关系的设计提示，但没有实现与 Go 相同的可执行生命周期。

## 扩展指南

若只调整哪些 Go 文件运行 exptostd，应优先修改 [`../../nogo_config.json`](../../nogo_config.json)，并同步扩展 [`../../config_test.rs`](../../config_test.rs) 的配置断言；无需在本文件复制路径规则。

若要真正完成 Rust 接线，最可能需要同时处理 `Analyzer` 的构造/所有权、`init()` 的显式装配入口、`analysis`/`exptostd`/`util` 的 crate 边界和 Cargo 依赖，并保证第三方规则、analyzer 名称及过滤顺序与 Go v0.4.5 保持一致。应在同目录新增独立 `analyzer_test.rs`，不要把测试内嵌进生产文件；测试至少覆盖一次性构造、初始化只包装一次、配置允许/排除路径、空文件集、缺失 `Run` 的约束，以及并发首次访问。

本任务是文档分析，不应借机补建整个 Rust linter 框架。任何接线改动还需确认 Bazel 的 Go 生产链是否保留、替换或并行存在，以免同一规则重复执行或诊断发生兼容性变化。性能风险主要在每个 pass 复制并过滤文件列表；当前薄封装本身没有扫描开销。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/exptostd` 找到 Go/Rust 两个源文件。
- RustCodeGraph `node --file build/linter/exptostd/analyzer.rs`：确认全文件 35 行、两个生产符号及 `used by 0 files`；`node ...::init` 确认唯一函数体；`callers ...::init` 无结果。
- 源码：[`analyzer.rs`](./analyzer.rs) 的 `Analyzer` 与 `init`；[`analyzer.go`](./analyzer.go) 的生产基准；[`../util/util.rs`](../util/util.rs) 与 [`../util/util.go`](../util/util.go) 的过滤包装器实现。
- 构建与依赖：根 [`Cargo.toml`](../../../Cargo.toml) 及 `build` 下无 Cargo manifest，证明未纳入 Rust workspace；[`BUILD.bazel`](./BUILD.bazel) 只编译 Go 源；[`../../BUILD.bazel`](../../BUILD.bazel) 仅在 `with_nogo` 选择分支接入该 Go target；`go.mod`、`go.sum` 和 [`../../../DEPS.bzl`](../../../DEPS.bzl) 确认 Go 第三方依赖版本。
- 配置与测试：[`../../nogo_config.json`](../../nogo_config.json) 给出 `exptostd` 文件范围；[`../../config_test.rs`](../../config_test.rs) 是唯一搜索到的相关 Rust 测试，只验证配置加载。没有找到直接引用本 Rust 模块或执行其两个符号的独立测试。
- 未运行 Cargo，符合纯文档任务要求；最终以任务指定命令校验固定的 11 个二级章节，并人工复核所有行为陈述均区分 Go 现状、Rust 草稿意图和未验证边界。
