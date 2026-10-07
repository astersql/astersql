# `build/linter/ineffassign/analyzer.rs`

## 文件定位

[`analyzer.rs`](analyzer.rs) 是 `build/linter/ineffassign` 下对 Go 版 ineffassign 适配器的 Rust 迁移文件。它位于开发期静态检查基础设施，而不在 SQL 请求、执行或存储运行时主链上。文件自身只有一个分析器别名 `Analyzer` 和一个初始化函数 `init`；真正检查“赋值结果从未被使用”的算法属于上游 `github.com/gordonklaus/ineffassign/pkg/ineffassign`，本文件不实现该算法。

当前生产接线仍是 Go/Bazel：[`BUILD.bazel`](BUILD.bazel) 的 `go_library(name = "ineffassign")` 只把 `analyzer.go` 列入 `srcs`，并由 `build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 依赖 `//build/linter/ineffassign`。根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员没有 `build/linter/ineffassign`，目录中也没有 `Cargo.toml`、`lib.rs` 或 `mod.rs`。因此该 Rust 文件目前是未接入 Cargo crate 的迁移源码，不能把 Go/Bazel 的生产接线误写成 Rust 已运行。

## 核心职责

该文件表达两项很窄的适配职责：

1. `Analyzer` 以共享引用形式暴露上游 `ineffassign::Analyzer`，意图复用同一个分析器对象，而不是在本地复制分析逻辑或配置状态（`analyzer.rs:24-25`）。
2. `init` 按 Go 版顺序先调用 `util::SkipAnalyzerByConfig`，再调用 `util::SkipAnalyzer`，意图为上游分析器叠加仓库配置过滤和 lint 指令过滤（`analyzer.rs:27-31`）。

这些职责描述的是源码所表达的适配意图。由于该文件没有 Cargo 模块入口，且它的不可变 `&analysis::Analyzer` 与当前 [`util.rs`](../util/util.rs) 中两个包装函数所需的 `&mut analysis::Analyzer` 不匹配，现有证据不能证明 Rust 路径已编译或执行。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = ineffassign::Analyzer`：包级公开静态引用，指向上游分析器。引用语义对于对象身份很重要：两个初始化调用都接收同一 `Analyzer` 表达式。RustCodeGraph 能看到该声明所在文件，但没有把 `Analyzer` 建成独立可导航节点。
- `pub fn init()`：唯一函数节点，无参数、无返回值。函数体只按固定顺序调用两个包装器。它虽名为 `init`，Rust 并不会像 Go 一样自动执行普通的 `init` 函数；必须由模块接线显式调用，但当前图索引没有找到调用者。
- 本文件没有结构体、枚举、trait、impl、局部辅助函数、条件编译项或本地错误类型。

公开性仅体现在源码的两个 `pub` 声明；缺少 crate/module 接线意味着它们当前没有可确认的 Rust 公共 API 边界。

## 执行流程

若未来由有效的 Rust 模块显式调用 `init`，源码表达的顺序如下：

1. 取得共享静态 `Analyzer`，其目标是上游 ineffassign 分析器。
2. 调用 `util::SkipAnalyzerByConfig(Analyzer)`，意图把上游 `Run` 包装为先按 `exclude_files`/`shouldRun` 过滤 `analysis::Pass.Files`，再执行旧 `Run`。
3. 调用 `util::SkipAnalyzer(Analyzer)`，意图再添加 `Directives` 前置分析器，并在运行和报告阶段处理文件级跳过及同行 `lint:ignore`/`nolint` 过滤。
4. 最终分析算法仍由被层层保存的上游 `Run` 执行；本文件本身不遍历 AST，也不生成诊断。

顺序不能随意交换。按当前 [`util.rs`](../util/util.rs) 的闭包包装方式，后调用的 `SkipAnalyzer` 会成为外层包装，并在其内部调用已经被配置过滤包装过的旧 `Run`。[`analyzer_test.rs`](analyzer_test.rs) 只以源文本位置断言“配置过滤调用早于全局跳过调用”，没有执行此调用链。

## 数据与状态

本文件不创建业务数据结构，唯一长期状态是 `Analyzer` 指向的分析器对象。初始化的预期副作用发生在该对象的字段上：

- `SkipAnalyzerByConfig` 取走旧 `Run`，安装按文件名过滤 `Pass.Files` 的闭包。
- `SkipAnalyzer` 向 `Requires` 增加 `Directives`，再次取走旧 `Run` 并安装过滤文件和诊断的闭包。
- 包装器都复制/克隆传入的 `analysis::Pass` 后调整视图，不应修改调用方原始 `Pass.Files`；诊断最终转发到原 `Report`。

以上状态变化由 `build/linter/util/util.rs` 的实现及 `build/linter/util/util_test.rs` 的行为测试支撑，而不是由本文件自己的运行测试支撑。尤其要注意：目标文件声明不可变共享引用，而包装器签名要求可变引用；在真正接线前必须解决所有权、可变性与一次初始化约束。

## 依赖与调用关系

上游依赖分三层：

- `analysis::Analyzer` 提供分析器类型；`ineffassign::Analyzer` 提供实际 ineffassign 实例；`util::{SkipAnalyzerByConfig, SkipAnalyzer}` 提供仓库适配。目标 Rust 文件没有 `use` 声明，假定这些模块名由尚不存在的 crate/module 上下文提供。
- Go 对应包通过 `go.mod` 固定 `github.com/gordonklaus/ineffassign v0.2.0`，Bazel 目标依赖 `@com_github_gordonklaus_ineffassign//pkg/ineffassign` 和 `//build/linter/util`。这只能证明 Go 生产目标的依赖，不能替代 Rust Cargo 依赖声明。
- `build/BUILD.bazel` 将 Go 目标 `//build/linter/ineffassign` 放进 `tidb_nogo` 的分析器依赖列表，所以 Go 版在 Bazel/nogo 构建检查链中使用。

RustCodeGraph 对 `build/linter/ineffassign/analyzer.rs::init` 的 `callers` 与 `callees` 查询都返回空数组。这与“没有 Cargo 模块入口”的文件证据一致：当前没有可验证的 Rust 上游调用边；图也没有解析两个包装调用为下游边。若要证明未来接线，必须补充模块声明、显式 `init` 调用和可编译依赖后重新同步索引。

## 错误处理与边界

`init` 没有返回 `Result`，也没有显式错误分支。潜在失败边界来自包装器及接线条件：

- `SkipAnalyzer` 运行时若 `Pass.ResultOf` 缺少按 `Directives` 对象身份登记的结果、结果类型不是 `Vec<Directive>`，或原分析器没有 `Run`，当前 Rust 实现会 `expect` 并 panic；`util_test.rs` 覆盖了缺失 directives 的 panic。
- `SkipAnalyzerByConfig` 同样假定原分析器存在 `Run`，否则执行包装后的分析器时会 panic。
- 文件过滤只改变送入旧 `Run` 的文件集合；它不在这里吞掉或转换上游 ineffassign 返回的错误，旧 `Run` 的结果原样向外传播。
- 源码当前存在可变性边界：`Analyzer` 是 `&analysis::Analyzer`，但两个 util 函数需要 `&mut analysis::Analyzer`。目录未加入 Cargo，现有文本测试也没有类型检查，因此不能声称这一调用能够通过 Rust 编译。
- Rust 中普通 `init` 不会自动运行。即使类型问题修复，遗漏显式调用也会让两个过滤包装完全不生效。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、I/O、事务或显式资源清理。值得关注的是分析器的全局共享生命周期：`Analyzer` 是 `static` 引用，理论上贯穿进程；`init` 的目的则是改写其 `Requires` 和 `Run`。这种“共享静态对象 + 可变初始化”需要在分析并发开始前完成，并且通常只能完成一次，否则可能重复追加 `Directives`、重复套入过滤闭包。

当前源码没有 `Once`、互斥锁或其他幂等/并发保护，也没有测试重复或并发调用 `init`。由于不可变引用与可变包装器签名尚不相容，实际 Rust 生命周期模型仍未落地；扩展时不能假定 Go 的包初始化串行保证会自动存在于 Rust。

## 与 Go 版本的对应关系

[`analyzer.go`](analyzer.go) 是直接语义来源：Go 的 `var Analyzer = ineffassign.Analyzer` 保存上游 `*analysis.Analyzer`，包 `init()` 自动执行，并依次调用 `util.SkipAnalyzerByConfig(Analyzer)`、`util.SkipAnalyzer(Analyzer)`。Go 版由 [`BUILD.bazel`](BUILD.bazel) 编译并接入 `tidb_nogo`，上游版本由 `go.mod` 固定为 v0.2.0。

Rust 文件保留了相同的公开名、同一上游对象的引用意图以及两个调用的顺序；[`analyzer_test.rs`](analyzer_test.rs) 的三个独立测试分别检查引用声明、调用顺序/同一表达式以及版权与非占位描述。然而差异也必须保留在结论中：Go `init` 自动运行，Rust `init` 不自动运行；Go 指针允许包装器原地修改，Rust 当前静态共享引用不能满足 util 的 `&mut` 参数；Go 有 Bazel 生产接线，Rust 没有 Cargo crate/module 接线。目录中没有对应 Go `_test.go`，因此 Go 语义依据主要来自实现和 util 层测试。

## 扩展指南

若只调整适配顺序或过滤策略，最可能修改 `init`，并同步更新独立的 [`analyzer_test.rs`](analyzer_test.rs)；不要把测试内嵌到生产 `.rs`。若更换上游分析器或其版本，应同时核对 `Analyzer` 的真实类型/所有权、Go 的 `go.mod` 与 Bazel 依赖，并为 Rust 增加可复现的、带发布 tag 的 Git 依赖，不能使用本地 `[patch]` 或复制 vendor。

要把该文件从迁移源码变为可运行 Rust，至少需要：建立明确的 Cargo crate/module 入口；声明并统一 `analysis`、`ineffassign`、`util` 依赖；设计可安全取得可变分析器的所有权模型；在分析启动前显式且只调用一次初始化；增加执行级测试，验证配置排除、文件级指令、同行 lint 指令、上游错误传播和重复初始化行为。完成这些工作后应重新运行 RustCodeGraph，确认出现真实 callers/callees，而不能只继续增加字符串包含断言。

兼容风险集中在过滤调用顺序、上游分析器对象身份和诊断位置语义；并发风险集中在全局可变初始化；性能风险较低但重复包装会增加每文件和每诊断的过滤遍历。任何扩展都应与 `build/linter/util/util.rs` 及其独立测试保持一致。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件，其中 Rust 7,032 个；`files --filter build/linter/ineffassign` 找到 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/ineffassign/analyzer.rs --offset 1 --limit 300`：读取完整 32 行源码；精确 `node build/linter/ineffassign/analyzer.rs::init` 确认唯一函数体。
- RustCodeGraph `callers`/`callees build/linter/ineffassign/analyzer.rs::init`：均为空数组；`Analyzer` 没有独立符号节点。
- 直接读取：`build/linter/ineffassign/analyzer.go`、`build/linter/ineffassign/analyzer_test.rs`、`build/linter/ineffassign/BUILD.bazel`、`build/BUILD.bazel`、`build/linter/util/util.go`、`build/linter/util/util.rs`、`build/linter/util/util_test.rs`、根 `Cargo.toml`、`go.mod`、`go.sum`、`DEPS.bzl`。
- 独立测试证据：`analyzer_test.rs` 只做源码契约检查；`util_test.rs` 执行并验证文件/诊断过滤、缺失 directives panic 和配置过滤。没有发现 ineffassign 专属 Go `_test.go`。
- 人工复核结论：本文件存在是为了镜像 Go 的第三方 analyzer 适配层；预期流程是按固定顺序叠加两层过滤；安全扩展首先要解决 Cargo 接线、显式初始化和可变所有权，而不是在本文件重写 ineffassign 算法。
