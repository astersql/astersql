# `build/linter/makezero/analyzer.rs`

## 文件定位

`build/linter/makezero/analyzer.rs` 是 Go 文件 [`build/linter/makezero/analyzer.go`](analyzer.go) 的 Rust 迁移镜像，位于构建期 linter 的 `makezero` 包中。它只声明 analyzer 的共享实例和初始化包装，不实现 makezero 的具体检查算法。

当前必须区分两条接线状态：Go 包由 [`build/linter/makezero/BUILD.bazel`](BUILD.bazel) 构建，并作为 `build/BUILD.bazel` 中 `nogo` 目标的依赖参与实际 Go lint；Rust 文件没有相邻 `Cargo.toml`，根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员和根库模块也没有纳入该生产文件。根 [`pkg/lib.rs`](../../../pkg/lib.rs) 只在 `cfg(test)` 下纳入 [`analyzer_test.rs`](analyzer_test.rs)，测试再以 `include_str!` 读取本文件。因此，本文件目前是迁移中的 Rust 源码镜像，不是已接入 AsterSQL Rust 运行链的 crate 模块。

## 核心职责

本文件有两项职责：

1. `Analyzer` 用 `once_cell::sync::Lazy<analysis::Analyzer>` 延迟调用 `analyzer::NewAnalyzer()`，表达“全局只构造一次并共享同一个 analyzer”的意图（`analyzer.rs:27-28`）。真正的 makezero 规则应来自外部 `analyzer` 模块，而非本文件。
2. `init()` 按 Go 版本的顺序先调用 `util::SkipAnalyzerByConfig`，再调用 `util::SkipAnalyzer`（`analyzer.rs:31-35`），意图在原始规则外依次叠加仓库配置排除和源码指令排除。

它不负责读取配置、解析 lint 指令、遍历 AST 或产生诊断；这些工作分别属于 `build/linter/util/util.rs` 和上游 makezero analyzer。也不能仅凭当前源码断言 Rust 路径已经可运行：目标文件没有生产模块接线，并且其调用形式与现有 Rust `util` API 的可变借用签名并不匹配。

## 主要符号

- `pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>`：公开的进程级延迟值。初始化闭包 `|| analyzer::NewAnalyzer()` 只在首次解引用时执行；声明意图对应 Go 的包级 `var Analyzer = analyzer.NewAnalyzer()`，但把 Go 包初始化时的立即构造改成了首次访问时构造。
- `pub fn init()`：公开、无参数、无返回值的包装入口。函数体只按固定顺序调用两个跳过配置函数；本文件内没有任何地方自动调用它。
- `analyzer::NewAnalyzer()`：未在本文件或当前 Cargo 依赖中定义的下游工厂名称，对应 Go 依赖 `github.com/ashanbrown/makezero/pkg/analyzer`。Go/Bazel 锁定的是 `DEPS.bzl` 中的 `com_github_ashanbrown_makezero` v1.2.0。
- `util::SkipAnalyzerByConfig` 与 `util::SkipAnalyzer`：仓库 linter 适配层。现有 Rust 定义分别位于 `build/linter/util/util.rs:175` 和 `:103`，两者都接受 `&mut analysis::Analyzer`；而这里传入的是 `&Analyzer`（即共享的 `Lazy` 引用），这是当前迁移尚未闭合的接口差异。

文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项。`Analyzer` 使用 Go 风格的大写名称，是为了保持跨语言符号对应，而不是惯用 Rust 静态量命名。

## 执行流程

按源码表达的预期流程：

1. 首次访问 `Analyzer` 时，`once_cell::sync::Lazy` 执行 `analyzer::NewAnalyzer()` 并缓存返回的 `analysis::Analyzer`（`analyzer.rs:27-28`）。
2. 外部接线显式调用 `init()`；Rust 不会像 Go 一样按包语义自动执行名为 `init` 的函数。
3. `util::SkipAnalyzerByConfig` 应先保存旧 `Run`，安装一层闭包，在每次分析时依据 `exclude_files`/`shouldRun` 过滤 `Pass.Files`，然后调用旧 `Run`（`build/linter/util/util.rs:175-193`）。
4. `util::SkipAnalyzer` 应再包一层闭包并把 `Directives` 加入 `Requires`：它过滤带跳过文件指令的文件，并包装 `Report` 以丢弃同文件同位置、且 linter 名匹配的诊断，最后调用上一层 `Run`（`build/linter/util/util.rs:103-171`）。
5. 因包装顺序是“配置过滤在内、指令过滤在外”，实际分析调用会先经过 `SkipAnalyzer` 的外层闭包，再进入 `SkipAnalyzerByConfig` 的内层闭包，最后到上游 makezero 的原始 `Run`。

步骤 1 至 5 是代码想表达的组合关系；当前仓库没有把本 Rust 文件纳入生产模块，也没有解决可变访问，因此上述 Rust 流程尚不能作为已接线运行行为。实际 Go 路径则由包初始化自动执行两个包装调用，并由 Bazel `nogo` 依赖加载。

## 数据与状态

本文件自身唯一持久状态是 `Analyzer` 的惰性单例。`Lazy` 负责一次初始化和之后共享；文件没有显式锁、缓存集合、通道或任务。

如果完成接线，两个适配函数会原地改写 `analysis::Analyzer`：`SkipAnalyzerByConfig` 取走并替换 `Run`；`SkipAnalyzer` 追加 `Directives` 到 `Requires`，再次取走并替换 `Run`。这使初始化具有顺序敏感性和一次性要求：重复调用 `init()` 会重复叠加依赖与包装层，不能视为幂等操作。

每次规则执行时，适配层克隆 `analysis::Pass`，只替换克隆值中的 `Files` 或 `Report`，再调用被包装的旧 `Run`；原始传入 `Pass` 的文件列表没有被就地改写。名称匹配使用 analyzer 的 `Name`，因此上游工厂返回值的名称也是跳过指令能否命中的关键状态。

## 依赖与调用关系

上游关系分为三类：

- RustCodeGraph 对 `build/linter/makezero/analyzer.rs` 报告 2 个符号且“used by 0 files”；`init` 没有已索引调用者。这与 Cargo 未接线的事实一致。
- [`analyzer_test.rs`](analyzer_test.rs) 通过 `include_str!("analyzer.rs")` 把文件当作字符串检查；它不编译或调用 `Analyzer`、`init()`。该测试由根 [`pkg/lib.rs`](../../../pkg/lib.rs) 的 `#[cfg(test)] #[path = ...]` 模块声明纳入。
- Go 侧 [`analyzer.go`](analyzer.go) 的包级变量和 `init()` 由 Go 包加载机制执行；[`build/linter/makezero/BUILD.bazel`](BUILD.bazel) 把包暴露给 `build/BUILD.bazel` 的 `nogo` 目标。

下游关系是 `analyzer::NewAnalyzer()`、`util::SkipAnalyzerByConfig`、`util::SkipAnalyzer` 和类型 `analysis::Analyzer`。RustCodeGraph 能定位后两个 `util` 定义及其实现，但不能定位目标文件内静态量为可调用生产链的一部分；`analyzer`/`analysis` 在目标文件中也没有 `use`、`mod` 或 Cargo 依赖声明。Go 侧的真实外部依赖由 `BUILD.bazel` 指向 `@com_github_ashanbrown_makezero//pkg/analyzer`，并由 `DEPS.bzl` 固定版本。

## 错误处理与边界

`Analyzer` 初始化闭包与 `init()` 都没有 `Result` 返回值或显式恢复路径。目标文件本身不处理工厂失败、配置读取失败或分析诊断；其 API 形态假定工厂直接返回 analyzer，包装函数也直接修改它。

现有 `util` 实现包含会 panic 的结构性前提：`SkipAnalyzerByConfig` 要求被包装 analyzer 的 `Run` 为 `Some`；`SkipAnalyzer` 还要求 `Directives` 的结果存在且能下转成 `Vec<Directive>`（`build/linter/util/util.rs:109-114,167-170,189-192`）。因此安全接线前必须确认外部工厂提供有效 `Run`，分析驱动器按 `Requires` 先执行 `Directives`，且 `ResultOf` 使用相同 analyzer 身份。

当前最直接的编译边界是可变性：两个 `util` 函数要求 `&mut analysis::Analyzer`，目标文件却把共享 `Lazy` 以 `&Analyzer` 传入。即便模块与依赖补齐，仍需设计一次初始化期间的独占可变访问；不能用不安全强转绕过，也不能在文档中把它描述为已解决。

配置过滤和指令过滤的边界由适配层决定：配置过滤按文件名调用 `shouldRun`；指令过滤只忽略整个文件或同一文件同一行、名称匹配的 linter 诊断。该包装不改变 makezero 自身“哪些值应为零值”等规则语义。

## 并发与资源生命周期

`once_cell::sync::Lazy` 提供线程安全的一次构造；初始化完成后多个线程可以取得共享引用。不过 `analysis::Analyzer` 的适配需要可变访问，当前接口没有展示在首次发布共享引用前如何完成这两次修改。这是生命周期设计缺口，而不是 `Lazy` 自动解决的问题。

合理的生命周期不变量应是“构造一次、包装一次、随后只读共享并执行多次”。若未来接线，可在 `Lazy` 初始化闭包内先把工厂返回值绑定为局部 `mut`，依次调用两个包装函数，再返回完整 analyzer；这样可变阶段发生在值发布之前，也避免重复 `init()`。是否采用该方案仍需与调用侧 API 对齐，本文件目前尚未实现。

运行时适配闭包会捕获旧 `Run`，并在每次分析中临时构造克隆的 `Pass`、文件集合和指令集合；这些值随单次调用释放。本文件没有后台线程、异步任务、通道、事务、文件句柄或需要显式清理的外部资源。

## 与 Go 版本的对应关系

[`analyzer.go`](analyzer.go) 包含同名 `Analyzer` 和 `init()`：包变量立即调用 `analyzer.NewAnalyzer()`；Go 运行时在包初始化阶段自动调用 `init()`；两个 `util` 调用的顺序与 Rust 文本一致。Go 源注释把它误写成 “ineffassign”，Rust 在 `analyzer.rs:24-25` 保留并解释了该历史注释，实际依赖仍是 makezero。

关键差异如下：

- Go 是已构建的 Bazel `go_library`，Rust 生产文件未加入 Cargo/module 图。
- Go 的包变量在包初始化时构造，Rust `Lazy` 延迟到首次访问。
- Go 的 `init` 自动运行，Rust 同名普通函数必须由调用者显式调用。
- Go 传递可变指针语义的 `*analysis.Analyzer`；Rust 适配函数要求 `&mut analysis::Analyzer`，目标文件当前却传 `&Lazy<...>` 共享引用。
- Go/Bazel 明确依赖 makezero v1.2.0；根 Cargo manifest 没有对应 Rust crate、feature 或版本声明。外部 Go 包不是可直接从 Rust 调用的依赖。

因此语义意图与调用顺序已经镜像，但编译依赖、初始化触发和独占可变性尚未完成等价移植。

## 扩展指南

若只是调整 makezero 规则，应优先修改或升级真实规则提供方，而不是在此包装文件复制算法；同时核对 Go 的 `DEPS.bzl`、`BUILD.bazel` 版本和 Rust 侧将来采用的正式依赖来源。按仓库规则，新增外部 Rust 依赖必须在独立上游仓库移植、提交并打 tag，再以统一 tag 的 Git 依赖引用，不能放入本仓库本地 vendor 或 `[patch]`。

若完成 Rust 生产接线，最可能修改的符号是 `Analyzer` 的初始化闭包和 `init()`：需要先建立真实 `analysis`、`analyzer`、`util` 模块依赖，再把两次包装放到 analyzer 对外共享之前，并明确谁触发初始化。必须保持 `SkipAnalyzerByConfig` 在前、`SkipAnalyzer` 在后的构造顺序，因为这决定 `Run` 包装的嵌套次序；还要防止重复初始化。

测试必须继续放在独立的 [`analyzer_test.rs`](analyzer_test.rs)，不要嵌入生产源文件。现有三个测试只验证文本：Lazy 工厂形态、两个调用的源码顺序、许可证与旧占位声明。完成接线时应新增能够实际构造并运行 analyzer 的独立测试，覆盖配置排除、`lint:ignore`/`nolint`、普通诊断、重复初始化策略以及并发读取；不能把当前字符串断言当成编译或行为证据。兼容风险集中在 analyzer 名称与指令匹配、包装顺序和 Go/Rust 输出一致性；性能风险主要是每次运行克隆 pass/指令集合以及重复包装。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter build/linter/makezero` 返回 Go/Rust 源和 Rust 测试；`node --file build/linter/makezero/analyzer.rs` 确认文件只有 `Analyzer`、`init` 两项主体且报告 `used by 0 files`；`query makezero` 定位目标、Go 对照和三个测试；`node build/linter/makezero/analyzer.rs::init` 确认函数体；对 `Analyzer` 的精确 `node` 查询没有得到独立静态量节点。`node` 还核对了 `build/linter/util/util.rs::SkipAnalyzer` 和 `::SkipAnalyzerByConfig` 的可变签名及包装逻辑。
- Rust 源与测试：[`analyzer.rs`](analyzer.rs)、[`analyzer_test.rs`](analyzer_test.rs)、[`build/linter/util/util.rs`](../util/util.rs)、[`pkg/lib.rs`](../../../pkg/lib.rs)。现有测试验证的是源码字符串，不是目标生产文件的编译或运行。
- Go 对照与构建：[`analyzer.go`](analyzer.go)、[`build/linter/util/util.go`](../util/util.go)、[`build/linter/makezero/BUILD.bazel`](BUILD.bazel)、[`build/BUILD.bazel`](../../BUILD.bazel)、[`DEPS.bzl`](../../../DEPS.bzl)。这些文件证明 Go 包的真实依赖、初始化顺序和 `nogo` 接线。
- Cargo 边界：根 [`Cargo.toml`](../../../Cargo.toml) 不包含 `build/linter/makezero` workspace 成员、路径依赖或 makezero Rust 依赖；目录下也没有相邻 `Cargo.toml`。因此未声称目标 Rust 文件已进入生产 crate。
- 本任务只创建说明文档，依计划不运行 Cargo。结构验证要求本文恰好包含本计划规定的 11 个二级标题；内容人工复核重点是区分 Go 的已运行行为、Rust 的迁移意图和当前未接线/可变性限制。
