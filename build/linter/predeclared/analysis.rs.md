# `build/linter/predeclared/analysis.rs`

## 文件定位

本文件位于构建期静态检查器目录 `build/linter/predeclared`，是同目录 [`analysis.go`](./analysis.go) 的 Rust 对照文件。它不实现“预声明标识符遮蔽”算法，而是用 `Analyzer` 转发上游 analyzer，并用 `init` 表达接入仓库两层跳过策略的意图（`analysis.rs:24-32`）。它属于开发/构建辅助面，不参与 SQL 请求、规划、执行或存储运行时。

当前真正进入检查链的是 Go/Bazel 版本：[`BUILD.bazel`](./BUILD.bazel) 的 `go_library` 只把 `analysis.go` 列为源码，[`../../BUILD.bazel`](../../BUILD.bazel) 将 `//build/linter/predeclared` 无条件加入 `tidb_nogo`。`build` 目录没有 `Cargo.toml`，根 [`Cargo.toml`](../../../Cargo.toml) 也没有本目录或 Rust `predeclared`/`analysis` 依赖；根 [`pkg/lib.rs`](../../../pkg/lib.rs) 仅在 `#[cfg(test)]` 下引入 [`analysis_test.rs`](./analysis_test.rs)，没有声明本生产模块。RustCodeGraph 对目标文件报告 `used by 0 files`。因此，本文件当前是未接线的迁移接口，不能描述为已在 Rust 构建链中运行。

## 核心职责

- `Analyzer` 以共享引用形式暴露 `predeclared::Analyzer`，意图保留上游 analyzer 的对象身份，而不是复制规则、配置或运行回调（`analysis.rs:24-26`）。
- `init` 保留 Go 初始化顺序：先调用 `util::SkipAnalyzerByConfig` 按 `exclude_files` 过滤文件，再调用 `util::SkipAnalyzer` 处理 `//lint:`/`//nolint:` 指令（`analysis.rs:28-32`；`analysis.go:25-28`）。
- 本文件只负责转发与包装。实际识别包名、导入名、变量、类型、函数、参数、命名返回值等是否遮蔽 Go 预声明标识符的逻辑，位于固定为 v0.2.2 的上游 `github.com/nishanths/predeclared/passes/predeclared`。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = predeclared::Analyzer`：公开的进程级共享引用，对应 Go 的 `var Analyzer = predeclared.Analyzer`。源码意图是让仓库适配层和上游规则操作同一个 analyzer，而不是创建副本（`analysis.rs:24-26`；`analysis_test.rs::upstream_analyzer_is_reexported_by_shared_reference`）。目标文件没有定义或导入 `analysis`、`predeclared` 模块，根 Cargo 也没有相应依赖。
- `pub fn init()`：公开、无参数、无返回值的适配函数，仅按固定顺序调用两个 wrapper（`analysis.rs:28-33`）。Rust 普通函数不会因为名为 `init` 就自动执行；若未来接线，必须由装配代码显式调用或改用等价的一次性注册机制。

文件中没有常量、自定义结构体、枚举、trait、`impl`、条件编译项或私有辅助函数，也没有本地实现 analyzer 的 `Run` 回调。

## 执行流程

按源码意图与 Go 生产实现，完整流程为：

1. 模块取得上游 `predeclared::Analyzer` 的同一对象，并以本地 `Analyzer` 名称公开。
2. 装配阶段调用 `init`；Go 中由包初始化自动触发，Rust 中则需要未来调用者显式触发。
3. `SkipAnalyzerByConfig` 先保存旧 `Run`，为每个 analysis pass 复制 pass 视图，并依据 analyzer 名称 `predeclared` 与 [`../../nogo_config.json`](../../nogo_config.json) 的 `exclude_files` 过滤 `Files`，之后调用旧回调（`../util/util.go::SkipAnalyzerByConfig`；Rust 对照 `../util/util.rs:175-193`）。
4. `SkipAnalyzer` 再追加 `Directives` 前置 analyzer，并在外层过滤整文件跳过指令或抑制同一行、同一 linter 的诊断，最后进入前一步包装过的回调（`../util/util.go::SkipAnalyzer`；Rust 对照 `../util/util.rs:103-171`）。
5. 上游 v0.2.2 的 `run` 为每次 pass 构造配置，遍历 `pass.Files`；`processFile` 检查声明和短变量声明等 AST 节点，对遮蔽 `go/doc.IsPredeclared` 所识别名称的位置报告诊断。默认不检查字段和方法，只有上游 `-q` 标志开启时才检查 qualified names；`-ignore` 可排除逗号分隔的标识符。

步骤 3—5 是 Go 生产行为及 Rust 代码表达的目标形状，不是当前 Rust 文件已运行的证明。现有 Rust `util.rs` 的两个 wrapper 都要求 `&mut analysis::Analyzer`，而本文件传入的是 `&analysis::Analyzer`；结合缺少模块和依赖接线，当前形态不能完成该流程。

## 数据与状态

本文件唯一模块级状态是共享引用 `Analyzer`；它不自行持有 AST、文件列表、诊断集合或规则配置。上游 Go analyzer 则包含名称 `predeclared`、说明文本、flags 和 `Run` 回调，并通过包级 `fIgnore`、`fQualified` 接收 `-ignore` 与 `-q` 配置。

两层仓库 wrapper 会原地改变 analyzer：`SkipAnalyzerByConfig` 替换一次 `Run`，随后 `SkipAnalyzer` 追加 `Directives` 到 `Requires` 并再次替换 `Run`。因此“两个 wrapper 操作同一对象”“配置过滤在内、指令过滤在外”“只初始化一次”都是重要不变量。重复调用会重复嵌套回调并可能重复追加 `Directives`；目标文件没有幂等保护。

上游 `run` 每次执行重新构造 `config`，其中 `ignoredIdents` 是由 flag 文本解析得到的集合；`processFile` 另建 `seenValueSpecs`，避免同一个 `ValueSpec` 在 AST 遍历中重复报告。上述 pass 内数据由上游实现拥有，不由本适配文件缓存。

## 依赖与调用关系

Rust 源码写出的下游关系为 `Analyzer -> predeclared::Analyzer` 和 `init -> SkipAnalyzerByConfig -> SkipAnalyzer`。RustCodeGraph 能定位目标 `init` 以及两个 `util.rs` wrapper，但目标文件的精确 callers/callees 查询没有产出有效调用边；文件级结果为 `used by 0 files`。仓库搜索只发现 [`pkg/lib.rs`](../../../pkg/lib.rs) 对独立测试文件的 `#[cfg(test)]` 引用，没有发现生产模块声明或 `init` 调用者。

Go 的实际构建链是：

`build/BUILD.bazel:tidb_nogo -> //build/linter/predeclared -> analysis.go::Analyzer -> github.com/nishanths/predeclared/passes/predeclared.Analyzer.Run`。

[`BUILD.bazel`](./BUILD.bazel) 同时声明 `//build/linter/util` 与 Bazel 外部仓库 `@com_github_nishanths_predeclared//passes/predeclared`；[`go.mod`](../../../go.mod) 和 [`DEPS.bzl`](../../../DEPS.bzl) 均锁定 v0.2.2。`analysis.rs` 不在 Go target 的 `srcs` 中，也没有对应 Cargo target。配置侧，[`../../nogo_config.json`](../../nogo_config.json) 的 `predeclared.exclude_files` 排除 `external/`、`cmd/mirror`、生成文件、mock、parser 生成入口和 cgo 路径。

## 错误处理与边界

本适配文件没有 `Result`、错误分支或诊断构造；`init` 不返回错误。上游 v0.2.2 的 `run` 对正常 pass 总是返回 `(nil, nil)`，发现遮蔽时通过 `pass.Report` 发诊断，而不是返回错误。上游规则只检查语法声明位置：默认包含包名、显式导入别名、常量、变量、类型、函数、接收者、参数、命名返回值、标签和 `:=` 左侧标识符；字段、接口方法与具名方法只有 `-q` 开启才检查。空 ignore 项会被忽略，非空项会抑制同名标识符。

仓库 wrapper 还有自身前置条件：Rust `SkipAnalyzer` 假定 `Directives` 结果存在且类型正确，并假定旧 `Run` 存在，否则会 `expect` 失败；`SkipAnalyzerByConfig` 也假定旧 `Run` 存在。所有文件被配置过滤后，空文件列表仍会传给上游回调。

当前 Rust 边界更直接：目标命名空间没有生产模块上下文，静态值是不可变共享引用，却被传给要求可变引用的两个 Rust wrapper。独立测试通过 `include_str!` 检查源码文本，不编译 `analysis.rs`，所以它能证明接口文本与顺序契约，不能证明生产实现可编译或可运行。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、网络连接或文件句柄，也没有显式清理动作。`Analyzer` 意图在进程生命周期内共享；wrapper 装配应在分析器并发处理各 pass 之前完成，此后只读使用。

Go 包初始化阶段会顺序完成共享 analyzer 的原地改写。每个 pass 的 wrapper 复制 pass 视图并创建过滤后的文件集合，上游再创建 pass 局部配置和去重 map，因此这些临时数据随单次分析结束释放。若 Rust 版本未来接线，应采用“构造或取得可变 analyzer -> 一次性安装 wrapper -> 冻结并共享”的生命周期，不能在分析开始后改写共享对象，也不能用未证明安全的全局可变别名规避 `&mut` 冲突。

## 与 Go 版本的对应关系

[`analysis.go`](./analysis.go) 是直接语义基准。Rust 的 `Analyzer` 对应 Go 包级 `var Analyzer = predeclared.Analyzer`，两者都意在复用上游对象；Rust `init` 中两个调用的顺序与 Go 完全一致：先配置级过滤，再指令级过滤。独立 [`analysis_test.rs`](./analysis_test.rs) 对共享引用文本、调用顺序、参数身份、版权头及无占位声明做了契约断言。

关键差异是语言语义和接线状态。Go 的 `init()` 自动执行，`Analyzer` 是可变指针，符合 `SkipAnalyzer(*analysis.Analyzer)`，并通过 Bazel 进入 `tidb_nogo`。Rust 的普通 `init` 不自动执行，`Analyzer` 是不可变共享引用，而 Rust wrapper 需要 `&mut analysis::Analyzer`；根 Cargo 既没有上游 Rust 依赖，也没有生产模块声明。因此 Rust 文件当前没有移植上游 AST 算法，也没有形成可运行的 analyzer。

仓库内本目录没有 Go 测试。规则行为证据来自本地 Go module cache 中上游 v0.2.2 的 `predeclared_test.go::TestAll`：它覆盖默认模式、`-ignore`、`-q`、全部预声明名称和无问题输入；这些上游测试不能替代本 Rust 适配层的编译与接线测试。

## 扩展指南

- 若只调整仓库排除路径，应修改 [`../../nogo_config.json`](../../nogo_config.json) 的 `predeclared` 条目，并同步验证 `util` 配置过滤行为，不要在此薄适配文件复制路径判断。
- 若修改 wrapper 类型或顺序，应同时核对 `analysis.go::init`、`analysis.rs::init` 和独立 `analysis_test.rs`；特别检查旧 `Run` 的闭包嵌套顺序、`Directives` 重复追加和重复初始化。
- 若升级或改变实际规则，应优先在独立上游依赖完成、提交并发布版本，再更新 Go module/Bazel 锁定；若引入 Rust 外部依赖，也必须使用已发布 tag 的 Git 依赖，不能复制到本仓库或用本地 `[patch]`。本文件仍应保持为装配层，不复制上游 AST 遍历算法。
- 若要让 Rust 版本真正可用，最可能修改的是 `Analyzer` 的所有权/初始化模型、`init` 的显式装配入口以及 Cargo/module 声明。必须先解决真实 `analysis`、`predeclared`、`util` 依赖和 `&mut` 契约，再增加编译与行为验证。
- 测试继续放在独立 [`analysis_test.rs`](./analysis_test.rs)，不要嵌入生产文件。现有文本契约应保留，并补充真实编译/装配、一次性初始化、配置过滤、指令过滤、`-ignore`/`-q`、默认声明类型及重复初始化防护。兼容风险集中在 Go 版本的预声明名称集合和上游 flag 语义；性能风险主要是每个 pass 的文件列表复制、AST 全遍历及误重复包装。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter build/linter/predeclared` 找到 `analysis.go`、`analysis.rs`、`analysis_test.rs`。
- RustCodeGraph `node --file build/linter/predeclared/analysis.rs --offset 1 --limit 260`：读取完整 33 行并报告 `used by 0 files`；目标文件只有 `Analyzer` 静态转发和 `init` 两项生产职责。`query init` 精确定位 `build/linter/predeclared/analysis.rs:29:function:init`。
- RustCodeGraph `node build/linter/predeclared/analysis.rs::init`、`node build/linter/util/util.rs::SkipAnalyzer`、`node build/linter/util/util.rs::SkipAnalyzerByConfig`：核对两个调用的源码顺序，以及 Rust wrapper 的 `&mut analysis::Analyzer` 参数、`Run` 替换和 `Directives` 依赖。目标 `init` 的精确 callers/callees 查询约 30 秒内没有输出，本文用文件级 `used by 0 files`、模块搜索和源码体交叉验证，未把无输出夸大为完整调用图证明。
- 已读仓库直接证据：[`analysis.rs`](./analysis.rs)、[`analysis.go`](./analysis.go)、[`analysis_test.rs`](./analysis_test.rs)、[`BUILD.bazel`](./BUILD.bazel)、[`../util/util.rs`](../util/util.rs)、[`../util/util.go`](../util/util.go)、[`../../BUILD.bazel`](../../BUILD.bazel)、[`../../nogo_config.json`](../../nogo_config.json)、根 [`Cargo.toml`](../../../Cargo.toml)、[`pkg/lib.rs`](../../../pkg/lib.rs)、[`go.mod`](../../../go.mod) 与 [`DEPS.bzl`](../../../DEPS.bzl)。
- 已读上游 v0.2.2 本地模块缓存：`passes/predeclared/predeclared.go`、`go18.go`、`pre_go18.go` 和 `predeclared_test.go`，用于核对规则范围、flags、诊断路径、Go 版本标识符判定及测试覆盖；本任务未运行上游测试或 Cargo。
- 人工复核结论：文件存在的理由是复刻 Go 的第三方 analyzer 转发与两层跳过包装；当前 Rust 文件未进入生产构建链；安全扩展必须维持共享对象与 wrapper 顺序，并先解决依赖、可变性、显式一次性初始化和独立测试接线。
