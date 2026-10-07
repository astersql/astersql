# `build/debug-linter/main.rs`

## 文件定位

`build/debug-linter/main.rs` 是从同目录 `main.go` 机械迁移得到的 Rust 草稿，用来保留 debug linter 的“单 analyzer 启动入口”形状。它不属于 SQL 请求、规划或执行主链，而是构建/开发辅助目录中的调试工具入口。

当前 Rust 文件尚未接入可构建目标：目录下没有 `Cargo.toml`，根 `Cargo.toml` 的 workspace `members` 中也没有 `build/debug-linter`；同目录 `BUILD.bazel` 的 `debug-linter_lib` 只把 `main.go` 列入 `srcs`，`debug-linter` 二进制也只嵌入这个 Go library。因此，现阶段真正可由仓库构建描述启动的是 Go 版本，Rust 文件只记录迁移意图，不能据此声称 Rust debug linter 已可运行。

## 核心职责

文件只承担一项职责：由 `main` 选择一个 analyzer，并把它交给单 analyzer 驱动器。当前选择写死为 `bootstrap::Analyzer`，对应检查 TiDB bootstrap 元数据一致性的规则；开发者若要单独调试另一个 linter，需要手工替换这个参数。

该文件本身不解析 Go AST、不遍历源码、不生成诊断，也不修改任何构建产物。真正的 Go 行为由 `singlechecker.Main` 驱动并调用 `build/linter/bootstrap/analyzer.go` 中声明的 `Analyzer`；Rust 源码注释明确说明 `singlechecker`、`bootstrap` 都只是占位依赖，当前不会启动真实 Go analysis。

## 主要符号

- `pub fn main()`：文件唯一的 Rust 符号和概念入口。它无参数、无返回值，函数体只有 `singlechecker::Main(bootstrap::Analyzer)` 一条调用。虽然声明为 `pub`，但当前不存在包含它的 Rust crate 或调用它的 Rust binary target，所以这里的可见性尚没有可用的 crate 级 API 含义。
- `singlechecker::Main`：对 Go `golang.org/x/tools/go/analysis/singlechecker.Main` 的形状保留。仓库 RustCodeGraph 没有为本文件解析出这个调用的下游定义，根 `Cargo.toml` 也没有对应依赖，故它不是已验证的 Rust 实现。
- `bootstrap::Analyzer`：对 Go 包级变量 `bootstrap.Analyzer` 的形状保留。相邻 Rust 草稿 `build/linter/bootstrap/analyzer.rs` 定义了同名静态值，但当前没有模块声明、crate 依赖或构建目标把两者连接起来；不能仅凭同名认定调用已经接线。

本文件没有模块级常量、类型、trait、`impl`、条件编译项或其他内部辅助函数。

## 执行流程

按当前源码表达的设计意图，流程只有三步：

1. 调试二进制进入 `main`。
2. `main` 取得当前手工选定的 `bootstrap::Analyzer`。
3. `main` 把 analyzer 交给 `singlechecker::Main`，后续参数处理、待分析包加载、规则执行和诊断输出都应由 singlechecker/analyzer 层负责。

第三步是 Go 版本的真实流程，却不是当前 Rust 仓库中可执行的流程。对 Rust 文件而言，执行在构建之前就缺少必要条件：它既未归属 Cargo target，也没有可解析的 `singlechecker` 和 `bootstrap` 模块接线。因此没有可观测的 Rust 运行路径或退出状态可供验证。

## 数据与状态

入口不创建或持有本地数据结构，也没有全局可变状态。唯一传递的数据是静态选择的 analyzer 值 `bootstrap::Analyzer`。

Go 对照中的 `Analyzer` 是 `*analysis.Analyzer`，其名称为 `bootstrap`、说明为 “Check developers don't forget something in TiDB bootstrap logic”，运行回调是 `run`。相邻 Rust 草稿把它写成 `pub static Analyzer: analysis::Analyzer`，但该静态值及其内部状态属于 analyzer 实现文件，不由本入口管理。入口也没有缓存、配置文件、数据库连接或构建产物生命周期。

## 依赖与调用关系

上游方面，Go 构建图在 `build/debug-linter/BUILD.bazel` 中以 `go_binary(name = "debug-linter")` 嵌入 `debug-linter_lib`，后者包含 `main.go`。RustCodeGraph 对 Rust `main` 没有找到可信的调用者；它报告的若干“used by”测试文件并非源码中的显式引用，文本检索也没有发现这些测试引用本路径或符号，因而不作为调用证据。

下游方面，Go 的 `main` 明确调用 `singlechecker.Main(bootstrap.Analyzer)`，Bazel 依赖也分别指向 `//build/linter/bootstrap` 和 `@org_golang_x_tools//go/analysis/singlechecker`。`bootstrap.Analyzer` 再通过其 `Run: run` 回调检查 `bootstrap.go` 与 `upgrade_def.go`。RustCodeGraph 对本文件 `main` 返回 “No callees found”，这与占位模块没有可解析定义的现状一致。

Rust 侧不存在对应 Cargo 依赖或模块入口。若未来实现，不应把同目录文件位置当成 Rust 模块关系；必须由实际 crate 的 `mod`/依赖声明和 binary target 建立连接。

## 错误处理与边界

本文件没有 `Result` 返回值、显式错误分支、恢复逻辑或诊断格式化。Go 版本把生命周期和失败处理整体交给 `singlechecker.Main`；入口不会截获 analyzer 错误。

其功能边界也很窄：一次只选择一个 analyzer，选择发生在源码中而非命令行或配置中。它不负责 bootstrap 规则的 AST 假设、报告内容和 panic 风险；这些属于 `build/linter/bootstrap/analyzer.go`。当前 Rust 草稿最重要的边界是“不可据此运行”：未接线依赖、未建立 Cargo target，且源码头注释明确标注“不保证可编译”。

## 并发与资源生命周期

入口自身没有线程、异步任务、锁、channel、文件句柄或网络资源，也没有显式清理阶段。analyzer 值在调用点被直接传给驱动器，入口不保留所有权相关状态。

Go 工具实际分析多少包、是否并行以及资源如何释放，由 `singlechecker`/`go/analysis` 驱动层决定；本文件和现有 Rust 证据均未定义这些策略，因此不作并发保证。未来若为 Rust 增加驱动器，应在驱动层而不是这个薄入口中记录任务取消、诊断汇聚与资源回收契约。

## 与 Go 版本的对应关系

Rust `main` 与 `build/debug-linter/main.go` 基本逐句对应：两者都有 debug-only 注释、都要求通过修改入口手工切换 linter，并都把 `bootstrap.Analyzer` 交给名为 `singlechecker.Main` 的函数。Rust 版本把 Go 的包导入改写成路径形式，但保留了 Go 风格的大写符号名，没有提供 Rust 实现或适配层。

关键差异在于接线状态。Go 文件由 Bazel 的 `go_library`/`go_binary` 目标构建，依赖声明完整；Rust 文件没有所属 crate、Cargo target、模块声明或依赖声明。相邻 `analyzer.rs` 同样标注为机械迁移草稿，不能替代缺失的 analysis/singlechecker 基础设施。故当前一致性只限于源码结构与意图，不代表行为等价。

同目录没有独立 Rust 测试或 Go 测试；对 `debug-linter`、`bootstrap.Analyzer`、`singlechecker.Main` 的测试文件检索也未找到入口级用例。现阶段可核验的是 Go 源码与 Bazel 接线，以及 Rust 草稿的静态结构，而非 Rust 运行结果。

## 扩展指南

若只是调试另一个现有 Go analyzer，最小改动点是 Go `main` 传给 `singlechecker.Main` 的参数，并同步 `BUILD.bazel` 的依赖；Rust 草稿若继续保持对照，也应同步记录选择，但不能把这种文本同步当成移植完成。

若要让 Rust 版本真正可用，至少需要：确定独立 crate 或现有工具 crate 的归属；声明 binary target；提供或引用可工作的 analysis、singlechecker 与目标 analyzer 实现；用 Rust 命名和类型系统明确 analyzer 的所有权/错误契约；最后增加独立测试文件，覆盖“选中的 analyzer 被驱动”“参数/加载失败传播”“诊断与 Go 基准一致”等行为。测试不得内嵌在 `main.rs`，应放在同 crate 的独立测试文件中。

扩展时应避免把完整 analyzer 逻辑塞进入口；`main` 保持只负责选择与启动，规则逻辑留在对应 analyzer 模块。兼容风险主要是诊断文本、退出码、命令行参数和包加载语义与 Go `singlechecker` 不一致；性能风险主要在未来驱动层的包加载和并行策略，本入口当前没有可测性能行为。

## 验证依据

- `build/debug-linter/main.rs`：文件全貌；确认只有 `pub fn main`，且源码明确标注机械迁移、占位依赖和当前不保证编译。
- RustCodeGraph：`status` 显示索引覆盖本文件；`files --filter build/debug-linter` 找到 Go/Rust 两个源文件；文件限定 `query` 将目标解析为 `build/debug-linter/main.rs:27:function:main`；`callers` 未给出调用者，`callees` 对该目标显示 “No callees found”。
- `build/debug-linter/main.go`：确认 Go 原入口调用 `singlechecker.Main(bootstrap.Analyzer)`。
- `build/debug-linter/BUILD.bazel`：确认真实 `debug-linter` Bazel binary 只包含 Go 源，并声明 bootstrap 与 Go singlechecker 依赖。
- 根 `Cargo.toml`：确认 workspace members、根 package 与依赖均未接入 `build/debug-linter`，且目标目录没有独立 `Cargo.toml`。
- `build/linter/bootstrap/analyzer.go` 与 `build/linter/bootstrap/analyzer.rs`：确认 analyzer 的名称、说明和 `run` 入口，以及 Rust 侧仍为占位迁移草稿。
- `rg` 测试/引用检索：未发现独立 Rust 或 Go 测试引用 debug-linter 入口；仅命中这两个入口源码及其构建声明。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核所有“已接线/未接线”陈述均有上述路径或图查询支持。
