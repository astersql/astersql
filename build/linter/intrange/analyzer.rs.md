# `build/linter/intrange/analyzer.rs`

## 文件定位

本文件是 Go 适配层 [`analyzer.go`](./analyzer.go) 的 Rust 迁移稿，位于构建期静态检查器目录 `build/linter/intrange`。它不实现 intrange 的 AST 检查算法，只把上游 `intrange::Analyzer` 暴露为包级 `Analyzer`，并用 `init` 表达接入仓库两层跳过策略的意图（`analyzer.rs:17-33`）。

当前真正进入构建链的是 Go/Bazel 版本：[`BUILD.bazel`](./BUILD.bazel) 的 `go_library` 只列出 `analyzer.go`，[`../../BUILD.bazel`](../../BUILD.bazel) 将 `//build/linter/intrange` 聚合进 `tidb_nogo`。Rust 生产文件没有对应 Cargo crate 或模块声明；根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace/依赖也没有 intrange Rust crate。根包 [`../../../pkg/lib.rs`](../../../pkg/lib.rs) 仅在 `#[cfg(test)]` 下引入 `analyzer_test.rs`，没有引入本生产文件。因此，本文件当前是未接线的迁移接口草稿，不能描述为已在 Rust 运行时执行。

## 核心职责

1. `Analyzer` 保留上游 analyzer 的对象身份，而非在仓库内复制规则实现或配置（`analyzer.rs:24-27`）。
2. `init` 保留 Go 初始化顺序：先调用 `SkipAnalyzerByConfig` 做仓库配置级文件过滤，再调用 `SkipAnalyzer` 做源码指令级跳过（`analyzer.rs:29-33`；`analyzer.go:25-28`）。
3. 文件将规则算法和 TiDB/AsterSQL 接线分开：循环模式识别、诊断与 suggested fix 属于固定在 Go 模块 v0.3.1 的上游 `github.com/ckaznocha/intrange`；本地适配层只负责复用与包装。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = intrange::Analyzer`：公开的共享引用，指向上游 analyzer。上游 v0.3.1 的 Go 定义名为 `intrange`，依赖 `inspect.Analyzer`，其 `Run` 才负责识别可改写为 Go 1.22 整数 range 的循环并报告诊断；这些算法不在本文件内。
- `pub fn init()`：公开、无参数且无返回值的适配函数。它依次调用 `util::SkipAnalyzerByConfig(Analyzer)` 和 `util::SkipAnalyzer(Analyzer)`，不创建新 analyzer，也不直接扫描源码。

两者保留了 Go 风格的大写名称和 `init` 名称以便逐项对照。Rust 普通函数不会因为名为 `init` 而自动运行；必须由未来的模块装配代码显式调用或用等价的一次性注册机制替代。

## 执行流程

按代码表达的预期流程：

1. 模块取得上游 `intrange::Analyzer` 的同一共享对象，并通过本地 `Analyzer` 名称公开它。
2. 装配方显式调用 `init`。
3. `SkipAnalyzerByConfig` 应包装原始 `Run`：复制 analysis pass，依据 analyzer 名称 `intrange` 和 [`../../nogo_config.json`](../../nogo_config.json) 的 `exclude_files` 过滤 `Files`，再转发给旧 `Run`。
4. `SkipAnalyzer` 应在外层追加 `Directives` 前置分析器，并过滤整文件跳过指令以及同行、同 linter 的 `lint:ignore`/`nolint` 诊断。
5. 通过两层过滤的文件才交给上游 intrange 规则；上游 v0.3.1 借助 `inspect.Analyzer` 遍历 `ForStmt`/`RangeStmt`，报告可使用整数 range 的位置及适用的 suggested fix。

步骤 2—5 是 Go 生产接线和 Rust 代码意图的组合说明，不代表当前 Rust 文件已经可执行。现有 Rust `util.rs` 的两个包装函数都接收 `&mut analysis::Analyzer`，而本文件持有 `&analysis::Analyzer` 并按共享引用传参；加上缺少模块/Cargo 接线，当前形态不能完成上述包装。

## 数据与状态

本文件自身没有自定义结构体、集合、缓存、锁或配置副本。唯一模块级状态是共享引用 `Analyzer`；规则名称、文档、前置 analyzer、运行回调等都由上游对象持有。

配置状态位于 [`../../nogo_config.json`](../../nogo_config.json) 的 `intrange.exclude_files`，当前排除 parser 生成入口、`external/`、生成文件、测试文件、mock 文件和 cgo 路径。本地包装器还会改变 analyzer 的 `Run` 与 `Requires`：`SkipAnalyzerByConfig` 替换 `Run`，`SkipAnalyzer` 再追加 `Directives` 并替换一次 `Run`。因此包装顺序和“只初始化一次”是重要不变量；重复初始化可能重复叠加依赖和闭包。

## 依赖与调用关系

- 上游规则：`intrange::Analyzer`。Go 生产依赖由 [`../../../go.mod`](../../../go.mod) 和 [`../../../DEPS.bzl`](../../../DEPS.bzl) 固定为 `github.com/ckaznocha/intrange v0.3.1`；该依赖提供实际 AST 检查算法。
- 分析框架：`analysis::Analyzer` 表示 Go analysis analyzer 的 Rust 迁移类型，但目标文件没有导入声明，且没有可解析它的生产模块上下文。
- 本地适配：[`../util/util.rs`](../util/util.rs) 中 `SkipAnalyzerByConfig` 和 `SkipAnalyzer` 分别表达配置过滤与指令过滤；对应的 Go 生产实现位于 [`../util/util.go`](../util/util.go)。
- Go 上游调用链：`build/BUILD.bazel:tidb_nogo -> //build/linter/intrange:Analyzer -> intrange.Analyzer.Run`。Go 包加载会自动调用 `init`，所以两层 wrapper 在 analyzer 执行前安装。
- Rust 调用链：RustCodeGraph 将本文件标为 `used by 0 files`，对本文件 `init` 未找到有效调用边；仓库搜索也只发现 [`../../../pkg/lib.rs`](../../../pkg/lib.rs) 的测试模块引用。故当前没有已验证的 Rust 生产上游调用者。

RustCodeGraph 对 `init` 未识别到两个函数调用边，这是索引对这些未解析 Go 风格命名空间的覆盖限制；调用事实直接来自 `init` 函数体，并由 Go 对照和源码契约测试交叉验证。

## 错误处理与边界

`Analyzer` 的暴露和 `init` 都没有本地错误返回。实际分析失败、类型断言失败及诊断构造由上游 v0.3.1 的 `Run` 负责；上游在缺少或无法转换 `inspect.Analyzer` 结果时返回带 `failed analysis` 上下文的错误。

本地配置包装的边界由 `util` 决定：配置不存在时应继续分析，路径不匹配时移除对应文件；非法正则会在配置匹配实现中触发 panic。指令包装假定 analyzer 存在原始 `Run`、pass 中存在 `Directives` 结果且类型正确，否则 Rust 对照实现会 `expect` 失败。若所有文件都被过滤，包装器仍会把空文件列表交给原始 `Run`。

更直接的当前边界是可编译性：共享 `&analysis::Analyzer` 不能传给要求 `&mut analysis::Analyzer` 的 Rust wrapper。本文件也没有 Cargo 依赖或模块接线。独立测试只对 `include_str!` 得到的源码文本做断言，并未编译 `analyzer.rs`，所以这些测试通过不能证明生产实现可用。

## 并发与资源生命周期

适配层不创建线程、任务、通道、文件句柄或堆资源。Go 生产对象是包级共享 analyzer，包初始化阶段串行安装包装器，之后由 nogo 驱动在 analysis pass 生命周期内使用。每次 wrapper 调用都复制 pass 视图、构造过滤后的文件列表；它不修改被检查的源文件。

若将 Rust 版本真正接线，必须在 analyzer 被并发读取前完成一次性可变装配，然后只读共享。不能用不安全的全局可变引用绕过 `&mut` 冲突，也不能在分析已开始后重复调用 `init`；否则可能造成 wrapper 重复嵌套、`Requires` 重复或并发数据竞争语义偏差。

## 与 Go 版本的对应关系

[`analyzer.go`](./analyzer.go) 是直接语义基准：`var Analyzer = intrange.Analyzer` 与 Rust 的共享引用声明都意在保留同一个上游 analyzer；两边的初始化顺序均为 `SkipAnalyzerByConfig` 后 `SkipAnalyzer`。Go 注释把它误称为 ineffassign analyzer，Rust 注释明确以右侧 `intrange.Analyzer` 为准，没有复制这处语义错误。

关键差异是接线和所有权。Go 的 `init()` 自动运行，`Analyzer` 是可变指针，符合两个 wrapper 的 `*analysis.Analyzer` 参数，并通过 Bazel 进入 `tidb_nogo`。Rust 的 `init` 不自动运行，`Analyzer` 是不可变共享引用，而 Rust wrapper 需要可变引用；根 Cargo 也没有 intrange Rust 依赖或生产模块声明。因此 Rust 文件目前只保留 API/顺序契约，不是上游 Go 检查器算法的完整 Rust 移植。

相关独立 Rust 测试 [`analyzer_test.rs`](./analyzer_test.rs) 验证三项源码契约：按引用再导出、两个 wrapper 的调用顺序与参数文本、AsterSQL/PingCAP 版权和无占位声明。仓库没有本目录 Go 测试；实际规则行为由上游 v0.3.1 的 `intrange_test.go::TestAnalyzer` 使用 `analysistest.RunWithSuggestedFixes` 覆盖。

## 扩展指南

- 若只调整仓库排除路径，修改 [`../../nogo_config.json`](../../nogo_config.json) 的 `intrange` 条目，不要在本适配文件复制路径判断；同步验证 `util` 的配置过滤测试。
- 若调整 wrapper 种类或顺序，修改 `init` 时必须与 [`analyzer.go`](./analyzer.go) 保持一致，并更新独立 [`analyzer_test.rs`](./analyzer_test.rs)；特别检查重复初始化、`Requires` 去重和闭包嵌套顺序。
- 若升级规则算法，先在独立上游依赖中完成并发布版本，再更新 Go 模块/Bazel 锁定；本文件只应更换稳定依赖引用，不应复制上游 AST 算法。
- 若要让 Rust 版本真正运行，应建立明确的 crate/module 与带 tag 的上游 Rust 依赖，设计“构造局部可变 analyzer -> 安装 wrapper -> 冻结并共享”的一次性初始化流程，并添加独立的编译及行为测试。不能把当前 `include_str!` 测试当作行为验证。
- 新测试继续放在独立 `analyzer_test.rs`，不要内嵌到生产文件。至少覆盖显式装配入口、配置过滤、指令过滤、空文件集合、重复初始化防护，以及上游诊断/suggested fix 的等价性。兼容风险集中在 Go 版本/上游版本差异和 wrapper 顺序；性能风险主要是每个 pass 的文件列表复制及重复包装。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter build/linter/intrange` 找到 Go/Rust/测试三个文件；`node --file build/linter/intrange/analyzer.rs` 读取完整 34 行并报告 `used by 0 files`；`query ...::init` 定位 `init`，callers 无有效调用者，callees 未解析本文件内的 wrapper 调用。
- 目标与直接对照：[`analyzer.rs`](./analyzer.rs)、[`analyzer.go`](./analyzer.go)、[`analyzer_test.rs`](./analyzer_test.rs)、[`BUILD.bazel`](./BUILD.bazel)。
- 装配与配置：根 [`Cargo.toml`](../../../Cargo.toml)、[`../../../pkg/lib.rs`](../../../pkg/lib.rs)、[`../../BUILD.bazel`](../../BUILD.bazel)、[`../../nogo_config.json`](../../nogo_config.json)、[`../../../go.mod`](../../../go.mod)、[`../../../DEPS.bzl`](../../../DEPS.bzl)。
- wrapper 语义：[`../util/util.go`](../util/util.go) 与 [`../util/util.rs`](../util/util.rs) 的 `SkipAnalyzer`、`SkipAnalyzerByConfig`。
- 上游 v0.3.1 本地模块缓存：`intrange.go` 的 `Analyzer`、`run`、`checkForStmt`、`checkRangeStmt`，以及 `intrange_test.go::TestAnalyzer`，用于核对规则职责、错误和 suggested fix；本任务未运行上游测试或 Cargo。
- 结构验收使用任务指定命令，确认文档存在且固定二级标题恰好为 11 个；人工复核重点为“Go 生产链已接线”和“Rust 迁移稿未接线、可变性不匹配”不能混淆。
