# `build/linter/rowserrcheck/analyzer.rs`

## 文件定位

本文件是 Go 适配层 [`analyzer.go`](./analyzer.go) 的 Rust 迁移镜像，位于构建期静态检查器目录 `build/linter/rowserrcheck`。它描述如何构造第三方 `rowserrcheck` analyzer，并按 TiDB/AsterSQL 的规则包装其跳过逻辑；它不属于 SQL 请求、执行或存储运行时。

当前接线必须区分两条路径：Go 包由 [`BUILD.bazel`](./BUILD.bazel) 定义为 `//build/linter/rowserrcheck`，再被 `build/BUILD.bazel` 的 `tidb_nogo` 依赖；Rust 文件则没有独立 `Cargo.toml`，根 [`Cargo.toml`](../../../Cargo.toml) 也没有把此目录声明为 workspace 成员或生产模块。RustCodeGraph 对本文件报告 “used by 0 files”。当前 Rust 侧唯一可确认的仓库入口是 `pkg/lib.rs` 在 `#[cfg(test)]` 下挂载 [`analyzer_test.rs`](./analyzer_test.rs)，测试通过 `include_str!("analyzer.rs")` 检查源码文本，而不是编译或执行本文件。因此本文件目前是迁移语义载体，不应被描述为已经接入 Rust 构建期 linter。

## 核心职责

文件只有两项职责：

1. `Analyzer` 延迟调用 `rowserr::NewAnalyzer()`，保存一个共享的 `analysis::Analyzer`。
2. `init()` 依次应用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`，表达 Go 版本的配置排除与统一 `nolint`/指令排除顺序。

真正的行错误检查规则来自外部 Go 依赖 `github.com/jingyugao/rowserrcheck/passes/rowserr`；本文件不实现规则遍历、诊断生成或 SQL 行处理。Go 依赖版本由 `go.mod` 固定为 `github.com/jingyugao/rowserrcheck v1.1.1`，Bazel 目标也依赖对应外部仓库。

## 主要符号

- `pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>`：模块级延迟单例。首次解引用时执行闭包 `|| rowserr::NewAnalyzer()`，避免在 Rust `static` 初始化表达式中直接调用运行时构造函数。名称保留了 Go 导出变量 `Analyzer` 的大小写。
- `pub fn init()`：公开初始化适配函数。它先调用 `util::SkipAnalyzerByConfig(&Analyzer)`，再调用 `util::SkipAnalyzer(&Analyzer)`；函数没有参数、返回值或显式错误类型。
- 本文件没有类型、trait、`impl`、模块级常量或条件编译项，也没有自行声明 `use`/`mod`。`analysis`、`rowserr`、`util` 和 `once_cell` 都依赖外围模块或未来 crate 接线提供。

需要注意，邻近 Rust 工具实现 `build/linter/util/util.rs` 中两个包装函数的当前签名均接收 `&mut analysis::Analyzer`，而这里传入的是共享 `Lazy` 的 `&Analyzer` 形式。由于本文件目前未被生产模块编译，现有源码文本测试不会暴露该类型/可变性接线问题；不能仅凭这些测试推断该文件可直接编译。

## 执行流程

按文件表达的预期流程：

1. 某个未来调用方访问 `Analyzer`，`once_cell::sync::Lazy` 在首次访问时执行 `rowserr::NewAnalyzer()`；后续访问复用同一实例。
2. 初始化阶段调用 `init()`。
3. `SkipAnalyzerByConfig` 先包装原 analyzer 的 `Run`：运行时按 analyzer 名称和文件路径查询 `build/nogo_config.json` 的 `exclude_files`，过滤不应检查的文件，再调用旧的 `Run`。
4. `SkipAnalyzer` 再包装上一步得到的 `Run`：加入 `Directives` 前置分析，过滤整文件跳过指令，并包装报告函数以抑制匹配 analyzer 名称和位置的 `nolint` 诊断。
5. 最外层统一跳过包装调用配置包装，配置包装最终调用第三方 rowserrcheck 的原始检查函数。这个嵌套顺序来自两次顺序改写 `Run`，也是 `both_skip_hooks_share_the_lazy_analyzer_in_go_order` 明确保护的顺序。

上述是由目标文件与 `build/linter/util/util.rs` 可推导的预期包装流程；当前仓库没有已验证的 Rust 生产调用方实际触发它。实际投入构建的对应流程仍由 Go `analyzer.go` 的包初始化自动完成。

## 数据与状态

`Analyzer` 是唯一的模块级状态。`Lazy` 提供一次初始化和跨调用共享，不为每次检查重新构造 analyzer。其负载 `analysis::Analyzer` 持有名称、依赖 analyzer 列表、`Run` 回调等分析配置；`init()` 的两个工具函数意图原地改写这些字段，尤其是逐层替换 `Run`。

配置状态不存放在本文件内。Go/Bazel 生产链由 `build/nogo_config.json` 中键名 `rowserrcheck` 的 `exclude_files` 控制文件排除；`.golangci.yml` 也启用了 `rowserrcheck`，并对 `_test.go` 配置排除。两套配置属于不同 linter 驱动，不能把其中一套的排除规则自动视为另一套已生效。

文件不缓存每次分析的 AST、诊断或数据库状态。包装运行期间创建的过滤文件集合、指令结果和报告闭包属于 `util` 实现的单次分析状态，不是本文件定义的持久状态。

## 依赖与调用关系

上游关系：

- Go 生产侧：`build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 依赖 `//build/linter/rowserrcheck`；该目标在本目录 `BUILD.bazel` 中只以 `analyzer.go` 为源文件。
- Rust 测试侧：根 crate 的 `pkg/lib.rs` 以 `#[cfg(test)]` 和 `#[path = "../build/linter/rowserrcheck/analyzer_test.rs"]` 引入测试模块；测试再以 `include_str!` 读取本文件。
- RustCodeGraph：本文件索引出文件节点和 `init` 函数，但报告 0 个使用文件；对精确 `init` 的 callers 没有返回可靠调用方。

下游关系：

- `rowserr::NewAnalyzer()`：提供第三方行错误检查 analyzer；本文件只负责取得并包装它。
- `once_cell::sync::Lazy`：表达线程安全的一次性延迟构造。
- `util::SkipAnalyzerByConfig`：按 `nogo_config.json` 的 analyzer 名称与路径规则过滤文件。
- `util::SkipAnalyzer`：加入指令分析并处理整文件或逐诊断的跳过语义。
- `analysis::Analyzer`：承载 analyzer 元数据与执行回调。

根 Cargo manifest 没有为 `build/linter/rowserrcheck` 声明 crate 边界，也未列出 `once_cell`、`analysis` 或 `rowserr` 作为该文件可用的直接依赖；若要把它变成可编译模块，必须先明确这些依赖的 Rust 实现来源和所有权，而不能依赖 Go/Bazel 声明。

## 错误处理与边界

目标文件自身没有 `Result`、错误枚举、恢复分支或日志。构造闭包和 `init()` 都假定下游调用成功；显式错误策略由第三方 analyzer 和 `util` 包装器决定。

直接证据显示 `util.rs` 的包装器在缺少 `Directives` 结果、结果类型错误或原 `Run` 缺失时使用 `expect`，即 Rust 接线完成后这些不变量被破坏会 panic，而不是返回可恢复错误。文件路径过滤依赖 analyzer 的 `Name` 与配置键一致；名称漂移会使配置排除静默失效。跳过包装的调用顺序也不可随意交换，因为两者都取出并替换 `Run`，顺序决定嵌套关系。

当前最重要的边界是“源码形态通过测试”不等于“Rust 可编译可运行”：独立测试只断言字符串、调用顺序、版权和占位语句缺失，没有类型检查、构造失败测试、真实诊断测试或并发执行测试。

## 并发与资源生命周期

`once_cell::sync::Lazy` 的选择表达线程安全的一次初始化：并发首次访问应只构造一个 analyzer，成功初始化后状态存活到进程结束，不提供显式销毁或重置入口。`runtime_factory_is_lazily_initialized_once` 只验证源码使用了这一类型和构造表达式，并未通过并发执行证明运行时行为。

`init()` 意图修改共享 analyzer 的 `Run` 与 `Requires`，所以安全接线必须保证包装只执行一次，并且发生在任何并发分析开始之前。当前 API 使用共享静态值，而 `util.rs` 需要可变引用，这一所有权冲突尚未由编译验证解决。未来不能用无同步的可变全局或重复执行 `init()` 来绕开它，否则可能重复添加 `Directives`、多层包装 `Run`，或引入初始化与分析并发竞争。

本文件不持有文件句柄、网络连接、事务、任务或通道；分析期临时集合与回调捕获值由包装闭包及 analyzer 生命周期管理。

## 与 Go 版本的对应关系

Go [`analyzer.go`](./analyzer.go) 是语义基准：

- Go `var Analyzer = rowserr.NewAnalyzer()` 对应 Rust `Lazy<analysis::Analyzer>`。差异是 Go 在包初始化期间立即构造，Rust 表达首次访问时构造；这是为静态初始化约束引入的生命周期差异。
- Go `init()` 由运行时自动调用；Rust 的普通 `pub fn init()` 不会自动执行，必须由明确的模块/启动接线调用，而当前没有这样的生产调用边。
- 两边都先应用 `SkipAnalyzerByConfig`，再应用 `SkipAnalyzer`，并针对同一个 analyzer 实例。
- Go 的 analyzer 是可变指针，两个工具函数能原地改写；Rust 当前静态声明和传参形式尚未满足邻近 Rust `util.rs` 的 `&mut` 签名。
- Go 生产依赖由 `go.mod`、`DEPS.bzl` 和 Bazel `go_library` 完整声明；Rust 版本没有对应 crate 清单或外部 Rust 依赖，因此迁移状态应标为“结构镜像与文本回归已存在，生产接线和编译语义未完成验证”。

本目录没有 Go 单元测试；相关独立 Rust 测试是 `analyzer_test.rs`，它保护延迟构造文本、两个 hook 的顺序以及版权/非草稿标记。

## 扩展指南

若只调整适配语义，应优先修改 `Analyzer` 构造闭包或 `init()` 的包装顺序，并同步更新独立测试 [`analyzer_test.rs`](./analyzer_test.rs)；不要把测试嵌入生产源文件。若第三方 analyzer 名称或配置格式变化，还要同步检查 `build/nogo_config.json`、`.golangci.yml`、`go.mod`/`DEPS.bzl` 与本目录 `BUILD.bazel`。

若目标是让 Rust 文件真正可用，需要先完成最小且可验证的接线设计：为 `analysis`、`rowserr`、`util` 和 `once_cell` 提供真实 crate/module 依赖；在模块树中声明本文件；为共享 analyzer 选择与 `&mut analysis::Analyzer` 相容且只初始化一次的所有权模型；由显式入口恰好调用一次 `init()`；再增加独立的编译与行为测试，验证排除文件、`nolint` 抑制、无 `Run` 时的失败方式以及并发初始化。外部 Rust 依赖必须按仓库政策在独立上游仓库移植并以已发布 tag 引用，不能复制到 `vendor`/`third_party` 或用本地 `[patch]`。

兼容风险集中在 analyzer 名称与配置键、包装顺序和 Go/Rust 初始化时机；性能风险主要是重复包装导致每次分析重复过滤，以及误用每次构造代替单例。扩展时应保持第三方检查算法本身在 `rowserr` 实现中，不在本适配层复制规则。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/rowserrcheck` 返回 Go 源、Rust 源和 Rust 测试三个已索引文件。
- RustCodeGraph `node --file build/linter/rowserrcheck/analyzer.rs`：确认文件共 34 行、报告 used by 0 files，并核对 `Analyzer` 与 `init()` 源码；`query rowserrcheck --json` 确认 `init` 位于第 30 行。
- RustCodeGraph `node/callers/callees build/linter/rowserrcheck/analyzer.rs::init`：确认函数定义；精确 callers 无可靠结果，callees 因同名 `init` 产生跨仓库噪声，目标定义本身显示 “No callees found”，故调用关系改由清单和源码直接核验，不据此声称已接线。
- 已读直接证据：`build/linter/rowserrcheck/analyzer.rs`、`analyzer.go`、`analyzer_test.rs`、`BUILD.bazel`、`build/BUILD.bazel`、`build/linter/util/util.rs`、`build/nogo_config.json`、`.golangci.yml`、根 `Cargo.toml`、`pkg/lib.rs`、`go.mod`、`DEPS.bzl`。
- `analyzer_test.rs` 的三个测试分别检查延迟工厂形态、两个跳过 hook 共享同一 analyzer 且保持 Go 顺序、版权与无草稿占位；这些是源码结构回归，不是目标 Rust 文件的编译或运行行为证明。
- 本任务按计划为纯文档分析，未运行 Cargo。交付结构检查要求本文恰好包含规定的 11 个二级标题，并由任务指定命令验证。
