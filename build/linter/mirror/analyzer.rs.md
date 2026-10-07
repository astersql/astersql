# `build/linter/mirror/analyzer.rs`

源文件：[`analyzer.rs`](./analyzer.rs)；Go 对照：[`analyzer.go`](./analyzer.go)；独立 Rust 测试：[`analyzer_test.rs`](./analyzer_test.rs)。

## 文件定位

该文件是 Go 包 `build/linter/mirror` 的单文件 Rust 迁移稿，意图为 `github.com/butuzov/mirror` 提供的 Go 静态分析器建立全局句柄，并套用 TiDB/AsterSQL 的两层跳过规则。对应 Go 库通过 [`build/linter/mirror/BUILD.bazel`](./BUILD.bazel) 进入 `build/BUILD.bazel` 的 nogo analyzer 依赖集合，因而 Go 版本属于构建期代码检查链，而不是数据库 SQL 请求运行链。

Rust 侧当前没有相同的生产接线。根 `Cargo.toml` 的 workspace 成员中没有 `build/linter`，目录内也没有独立 `Cargo.toml`；RustCodeGraph 将本文件标为“used by 0 files”。根 crate 的 `pkg/lib.rs` 只在 `#[cfg(test)]` 下通过 `#[path = "../build/linter/mirror/analyzer_test.rs"]` 引入独立测试，测试再用 `include_str!("analyzer.rs")` 把本文件当文本检查，并没有把它编译成 Rust 模块。因此，下文涉及 `Analyzer` 和 `init()` 的流程是源码表达的迁移意图，不等同于当前可执行的 Rust 生产路径。

## 核心职责

文件只承担两项接线职责，不实现 mirror 检查算法本身：

1. `Analyzer` 延迟调用 `mirror::NewAnalyzer()`，保存一个 `analysis::Analyzer`。
2. `init()` 按 Go 原顺序先调用 `util::SkipAnalyzerByConfig`，再调用 `util::SkipAnalyzer`，意图先包裹配置排除逻辑，再包裹源码指令排除逻辑。

真正的诊断规则应来自外部 Go 依赖 `github.com/butuzov/mirror`；本 Rust 文件没有遍历 AST、产生诊断或读取配置的代码。仓库中也未发现一个已接入 Cargo 的同名 Rust `mirror` analyzer 实现，故不能据此声称 Rust 端已经具备真实 mirror 分析能力。

## 主要符号

- `pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>`：公开全局惰性值。首次解引用时执行闭包 `|| mirror::NewAnalyzer()`，此后复用同一实例。名称和大写风格刻意对应 Go 的包级变量 `Analyzer`，没有 `const`、trait、结构体或条件编译项。
- `pub fn init()`：公开的显式初始化函数，依次把 `&Analyzer` 传给两项 util 包装函数。它模仿 Go 的 `func init()` 内容，但 Rust 不会因函数名为 `init` 自动调用它；必须有模块入口显式调用才会运行，而当前仓库没有这样的 Rust 调用者。
- `mirror::NewAnalyzer`、`analysis::Analyzer`、`util::SkipAnalyzerByConfig`、`util::SkipAnalyzer`：均是文件假定由外围模块提供的路径，本文件没有 `use` 声明、模块声明或本地定义。

## 执行流程

按源码意图，若未来有可编译模块显式调用 `init()`，流程应为：

1. 求值 `&Analyzer` 时触发 `once_cell::sync::Lazy`；第一次调用外部工厂 `mirror::NewAnalyzer()`，后续访问复用结果。
2. `SkipAnalyzerByConfig` 包裹 analyzer 原有的 `Run`：对每个待分析文件调用配置判定，过滤 `exclude_files` 不应运行的文件。
3. `SkipAnalyzer` 再次包裹上一步的 `Run`：加入 `Directives` 前置分析器，排除文件级跳过项，并在报告阶段抑制命中 `lint:ignore`/`nolint` 的诊断。
4. 真正执行 analyzer 时，外层源码指令包装调用内层配置包装，内层再调用 mirror 原始 `Run`。这一嵌套次序来自 `build/linter/util/util.rs` 中两函数“取走旧 Run、安装闭包、再调用旧 Run”的实现。

当前实际可验证流程更窄：`cargo test` 构建根 crate 时可编译 `analyzer_test.rs`，三个测试只搜索本文件的字符串形状、调用顺序和版权/占位语句；生产 `Analyzer` 与 `init()` 本身不进入该测试 crate 的模块树。

## 数据与状态

`Analyzer` 是唯一的文件级状态。`Lazy` 负责一次性初始化及跨调用共享，避免在静态初始化表达式中直接调用普通工厂。文件自身没有集合、缓存、配置副本、通道或事务状态。

若按意图执行，两项 util 函数会原地修改同一个 `analysis::Analyzer`：向 `Requires` 添加 `Directives`，并连续替换其 `Run` 回调。顺序是可观察状态的一部分，因为第二次包装持有第一次包装后的回调。重复调用 `init()` 还会重复加入依赖并重复包裹回调；源码没有幂等保护，调用方必须保证只初始化一次，才能对应 Go 包初始化的单次语义。

同时必须注意当前类型矛盾：`build/linter/util/util.rs` 的两个函数签名都要求 `&mut analysis::Analyzer`，而本文件传入的是 `&Analyzer`，且 `once_cell::sync::Lazy` 没有在这里提供可变访问。由于生产文件未被 Cargo 编译，现有文本测试没有暴露这一问题；在解决可变所有权之前，不能把该全局状态描述为可工作的 Rust 注册实现。

## 依赖与调用关系

上游关系分为 Go 与 Rust 两条：

- Go：`build/linter/mirror/BUILD.bazel` 声明 `go_library`，依赖 `//build/linter/util` 和 `@com_github_butuzov_mirror//:mirror`；`build/BUILD.bazel` 将该 target 纳入 nogo analyzer 集合。Go 包加载时自动执行 `init()`。
- Rust：RustCodeGraph 对 `build/linter/mirror/analyzer.rs::init` 未找到调用者，对文件报告 0 个使用方；`pkg/lib.rs` 仅接入 `analyzer_test.rs`。根 Cargo workspace 没有本目录的 package，根 manifest 也未给本文件建立模块入口。

下游意图是 `mirror::NewAnalyzer()` 和 `build/linter/util/util.rs` 的两个包装器。RustCodeGraph 未能从 `init()` 解析出静态 callee 边，因此这些边由源码调用表达式及 util 定义交叉核验。外部 analyzer 工厂的具体规则、诊断内容和复杂度不在本文件中，也没有可用 Rust 实现可继续追踪。

## 错误处理与边界

本文件没有返回 `Result`、错误分支或显式 panic。工厂初始化若发生 panic，`Lazy` 访问会沿调用栈传播；源码没有恢复策略。两项 util 包装器的真实 Rust 实现会在缺失 `Directives` 结果、结果类型错误或缺失原始 `Run` 时 `expect` 失败，但这些是下游工具函数的边界，不是本文件自行处理的错误。

更直接的当前边界是不可编译接线：共享引用与 util 所需可变引用不匹配，并且 `analysis`、`mirror`、`util` 没有通过本目录 Cargo/module 边界导入。现有独立测试验证的是文本合同，无法证明名称解析、类型检查、真实诊断、配置过滤或 nolint 行为。文档因此不把“保留调用形状”提升为“功能已运行”。

## 并发与资源生命周期

`once_cell::sync::Lazy` 的设计目标是线程安全地只运行一次构造闭包，因此多个读取者在成功初始化后应观察同一 analyzer 实例。`analyzer_test.rs::runtime_factory_is_lazily_initialized_once` 只验证源码包含这种声明形状，并未启动线程或执行工厂。

analyzer 的初始化又需要原地改写 `Requires` 与 `Run`，这与只读共享全局存在冲突。若未来接线，应该在发布共享引用之前完成构造和两层包装，例如让 `Lazy` 的闭包返回已经配置完成的值，而不是在公开的 `init()` 中事后修改；具体方案需与 `analysis::Analyzer` 的线程安全约束一起设计。文件不创建任务、锁、通道、文件句柄、网络连接或事务，也没有清理阶段。

## 与 Go 版本的对应关系

`build/linter/mirror/analyzer.go` 的 `var Analyzer = mirror.NewAnalyzer()` 在 Go 包初始化期间构造一次；紧随其后的语言级 `init()` 自动执行两个修改函数，参数是可变的 analyzer 指针。Rust 迁移稿用 `Lazy` 表达“一次构造”，并保留 `SkipAnalyzerByConfig` 在 `SkipAnalyzer` 之前的顺序，相关文本测试 `configured_and_global_skip_share_the_lazy_analyzer_in_go_order` 明确锁定了这一点。

差异有三项：Rust 的 `init()` 不会自动执行；Rust 的共享 `Lazy` 引用不能满足现有 util 的 `&mut` 参数；Go/Bazel 已链接真实 `butuzov/mirror`，Rust/Cargo 没有对应生产依赖或模块接线。因此 Go 版本目前是构建链中的真实实现，Rust 文件只是带“已处理”版权标记、受源码合同测试保护的迁移候选。目录中没有独立 Go 测试；行为参照来自同路径 Go 源码、Bazel target 和 Rust 文本测试。

## 扩展指南

若只调整 Go mirror analyzer 的注册策略，应同步修改 `analyzer.go`、本 Rust 对照文件，并保持两个跳过包装的顺序；同时扩展独立的 `analyzer_test.rs`，不要把测试嵌入生产源文件。

若要让 Rust 版本真正工作，最小接入点至少包括：为 `build/linter` 建立明确的 Cargo crate/module 边界；提供或引用可复现的 Rust `analysis`、`mirror` 与 `util` API；把 analyzer 的构造和可变包装放到独占初始化阶段；由真实入口注册或调用它；增加行为测试，覆盖配置排除、文件级跳过、单行 linter 跳过、工厂只执行一次和重复初始化策略。完成这些之前，不应仅放宽文本断言来宣称支持。

兼容风险主要是包装顺序改变导致过滤层级不同、重复初始化造成 `Requires` 重复或回调多重嵌套，以及外部 mirror 规则版本与 Go 依赖漂移。性能风险集中于每次分析对文件集合的两次过滤和指令扫描；本文件没有基准数据，优化时应先保留 Go 可观察语义并增加独立基准/行为测试。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/mirror` 找到 Go 源、Rust 源和 Rust 测试。
- RustCodeGraph `node --file build/linter/mirror/analyzer.rs`：确认文件共 35 行，生产符号为 `Analyzer` 与 `init()`；文件显示 0 个使用方。
- RustCodeGraph `node/callers/callees build/linter/mirror/analyzer.rs::init`：确认函数体的两次调用；未找到调用者或可解析的 callee 边。
- `build/linter/mirror/analyzer.go` 与 `build/linter/mirror/BUILD.bazel`：确认 Go 工厂、初始化顺序、外部 mirror/util 依赖及 Go library 边界；`build/BUILD.bazel` 确认其进入 nogo analyzer 集合。
- 根 `Cargo.toml`、`pkg/lib.rs:95-97`：确认 Cargo workspace 没有 `build/linter` 成员，根测试模块只引入 `analyzer_test.rs`。
- `build/linter/mirror/analyzer_test.rs`：确认现有三个测试都是 `include_str!` 文本合同，覆盖 Lazy 形状、调用顺序和版权/占位声明，不执行 analyzer。
- `build/linter/util/util.rs:101-193` 及 Go 对照 `util.go:102-183`：确认跳过包装的状态修改、嵌套顺序、失败边界，以及 Rust API 要求 `&mut analysis::Analyzer`。
- 本任务按计划不运行 Cargo；交付结构验证要求文档存在且恰含 11 个规定二级标题。
