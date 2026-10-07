# `build/linter/durationcheck/analyzer.rs`

## 文件定位

本文件位于构建期静态检查目录 `build/linter/durationcheck/`，是同目录 Go 实现 `analyzer.go` 的机械迁移草稿。它不实现 duration 数值检查算法，而是把上游 `github.com/charithe/durationcheck` 提供的 analyzer 暴露为仓库内的 `Analyzer`，并保存 TiDB 对 analyzer 追加两层跳过包装的初始化意图。

当前真实接线必须区分 Go 与 Rust：Go 侧由 `build/linter/durationcheck/BUILD.bazel` 构造成公开 `go_library`，再由 `build/BUILD.bazel` 的 `nogo(name = "tidb_nogo")` 依赖；Rust 侧没有本目录的 `Cargo.toml` 或生产模块声明，RustCodeGraph 对本文件报告 `used by 0 files`。根 crate 只在 `pkg/lib.rs` 的 `#[cfg(test)]` 下引入独立的 `analyzer_test.rs`，测试再以 `include_str!("analyzer.rs")` 将本文件作为文本读取。因此本文件不是 SQL 运行时链路的一部分，也不是当前可执行的 Rust lint 入口。

## 核心职责

源码保存两项极小的适配职责：`Analyzer` 指向上游 durationcheck analyzer；`init` 依次调用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`，意图先按仓库的 `exclude_files` 配置过滤输入文件，再支持源码中的 `nolint`/`lint:ignore` 指令。具体的 duration 使用规则不在本文件内，而在 Go 外部依赖 `github.com/charithe/durationcheck` 中；本次本地证据没有展开该外部依赖源码，因此不对规则细节作额外推断。

这两个包装调用不是把规则“禁用”：由 `build/linter/util/util.go` 可见，前者替换 analyzer 的 `Run`，在调用旧 `Run` 前过滤 `pass.Files`；后者再次包装 `Run`，添加 `Directives` 前置依赖、过滤整文件，并拦截应忽略位置的诊断。源码注释中“全局跳过动作”容易被误解，实际 Go 行为是增强 analyzer 对仓库排除配置和内联抑制指令的支持。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = durationcheck::Analyzer`：模块级共享引用，意图保持与 Go `var Analyzer = durationcheck.Analyzer` 相同的上游 analyzer 对象身份，而非复制 analyzer 状态。文件没有可见的 Rust `use`、模块声明或 Cargo 依赖来解析 `analysis` 与 `durationcheck`。
- `pub fn init()`：普通公开函数，按固定顺序调用 `util::SkipAnalyzerByConfig(Analyzer)` 和 `util::SkipAnalyzer(Analyzer)`。Rust 的同名函数不会像 Go 的包级 `init()` 那样自动运行，仓库也没有找到显式 Rust 调用者。

文件没有自定义常量、结构体、枚举、trait、impl、条件编译项或 lint 算法函数。两个符号虽声明为 `pub`，但本文件没有进入 Rust 生产模块树，因而目前不形成可被 crate 使用的公开 API。

## 执行流程

按 Go 生产实现的实际生命周期，流程是：加载 `durationcheck.Analyzer` 指针；包初始化自动调用 `SkipAnalyzerByConfig`，将原始 `Run` 包装为“按 analyzer 名称和文件路径执行 `shouldRun` 后再运行”；随后调用 `SkipAnalyzer`，在前一层包装外再加指令解析、整文件过滤和诊断抑制；Bazel 的 `tidb_nogo` 将该 analyzer 与其他检查器一起用于 Go 构建分析。因为后调用的包装保存前一次包装后的 `Run`，运行时先执行 `SkipAnalyzer` 的外层逻辑，再进入 `SkipAnalyzerByConfig`，最终才调用上游 durationcheck 的原始规则。

Rust 文件只保存了初始化调用的书写顺序，没有上述可运行流程：`init` 不会自动触发，且没有调用者；更直接地，`build/linter/util/util.rs` 中两个函数的参数均为 `&mut analysis::Analyzer`，而这里的 `Analyzer` 类型是不可变共享引用 `&analysis::Analyzer`。因此当前调用在类型层面不匹配，不能把这段草稿描述为已经完成注册或已经能够运行 lint。

## 数据与状态

本文件自身没有业务数据结构或局部状态。`Analyzer` 理论上指向一个静态 analyzer 对象，其重要可变内容由下游包装函数操作：`Requires` 会追加 `Directives`，`Run` 会被逐层取出并替换为闭包。调用顺序是不变量，因为第二次包装必须保存第一次包装后的 `Run` 才能叠加两种过滤能力。

Go 版本通过可变指针直接修改上游 analyzer 的全局对象，修改发生在包初始化期且影响后续所有使用者。Rust 草稿却声明共享不可变引用，无法满足 `util.rs` 的可变借用要求；同时静态共享可变对象在 Rust 中还需要明确的独占初始化与同步设计，当前文件没有提供这类生命周期或所有权方案。

## 依赖与调用关系

Go 上游链路为 `build/BUILD.bazel::tidb_nogo -> //build/linter/durationcheck -> analyzer.go::Analyzer`。`build/linter/durationcheck/BUILD.bazel` 声明直接依赖 `//build/linter/util` 和 `@com_github_charithe_durationcheck//:durationcheck`；`go.mod` 将后者固定为 `v0.0.11`，`DEPS.bzl` 也声明对应 Bazel 外部仓库。仓库内没有针对本规则的 Go 测试文件。

Go 下游包装关系为 `init -> util.SkipAnalyzerByConfig` 和 `init -> util.SkipAnalyzer`，两者最终转调先前保存的 analyzer `Run`。RustCodeGraph 能定位 Rust `init`，但 `callers`、`callees` 和 `impact` 查询均未返回目标文件的调用边；仓库搜索也没有发现 `mod durationcheck`、Rust 生产引用或显式 `init()` 调用。根 `Cargo.toml` 的 workspace members 不含本目录，依赖区也没有 Rust `durationcheck`/Go-analysis 等价依赖；Rust 唯一接线是 `pkg/lib.rs` 在测试配置下加载 `analyzer_test.rs`。

## 错误处理与边界

本文件没有 `Result`、错误类型、诊断生成或显式失败分支；所有实际检查错误和诊断行为属于上游 durationcheck analyzer。两层工具包装的 Go 实现也直接调用保存的旧 `Run` 并传播其 `(any, error)` 返回值，本适配层不转换错误。配置过滤和内联抑制可能减少参与分析的文件或丢弃诊断，但不修改源文件。

当前 Rust 边界更严格：未解析的 Go 风格命名空间、普通 `init` 无自动初始化语义，以及 `&analysis::Analyzer` 传给要求 `&mut analysis::Analyzer` 的函数，都会阻止其成为可执行实现。独立测试仅断言静态文本中使用共享引用、两个调用存在且顺序正确，并明确拒绝额外写成 `&Analyzer`；它没有编译本文件，也没有验证 analyzer 行为、配置过滤、指令抑制或上游诊断。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、事务、文件句柄或网络资源。Go 版本的状态变更集中在单线程包初始化阶段：对 analyzer 的 `Requires` 和 `Run` 完成包装后，nogo 分析阶段复用该对象。包装闭包只在每次 analysis pass 内复制 pass 视图、过滤文件和代理报告函数，资源由分析框架管理。

若未来把此适配真正移植到 Rust，不能直接用不可变 `static` 共享引用承载初始化期变更；需要先确定 analyzer 的所有权归属，并保证包装只执行一次，避免重复追加 `Directives` 或递归套娃 `Run`。若对象会跨线程共享，还需由实际 analyzer 类型及框架约束证明其同步安全，当前源码没有这方面证据。

## 与 Go 版本的对应关系

`build/linter/durationcheck/analyzer.go` 是当前可构建的权威实现。Rust 文件逐行保留其最外层结构：Go 的 `Analyzer = durationcheck.Analyzer` 对应 Rust 的同名静态共享引用；Go `init` 中先 `SkipAnalyzerByConfig`、后 `SkipAnalyzer` 的顺序也被原样保存。Rust 独立测试 `upstream_analyzer_is_reexported_by_reference` 与 `configuration_and_global_skip_keep_go_order_without_double_reference` 专门锁定这两项文本契约。

两者尚不具备运行等价性。Go 的 `Analyzer` 是 `*analysis.Analyzer`，可被两个包装函数原地修改，且 Go 运行时自动执行 `init`；Rust 使用不可变 `&analysis::Analyzer`，与 `util.rs` 要求的 `&mut` 不兼容，普通 `init` 也不会自动运行。Go 侧有真实 imports、Bazel 外部依赖和 nogo 接线；Rust 侧没有对应 crate 或依赖。故目前只能确认迁移草稿保存了名称、对象引用意图和调用顺序，不能确认 Rust 能编译或执行 durationcheck。

## 扩展指南

若只调整仓库级排除或内联抑制机制，应优先修改 `build/linter/util` 的对应实现并评估所有 analyzer，而不是在本文件复制过滤逻辑。若升级 durationcheck，应同步核对 `go.mod`、`go.sum`、`DEPS.bzl` 和 `BUILD.bazel`，确认上游 `Analyzer` API 与诊断行为未变。任何调用顺序变化都要验证两层 `Run` 包装的实际嵌套顺序，避免丢失配置过滤或指令抑制。

若目标是使 Rust 版本真正可用，需要先建立明确的 crate/模块边界和真实 analysis/durationcheck 依赖，再重新设计 analyzer 的可变所有权与一次性初始化入口；不能仅把共享引用改成值或强行获取可变静态引用。相关测试应继续放在独立的 `build/linter/durationcheck/analyzer_test.rs`，并从当前文本契约扩展为可执行测试，至少覆盖：初始化只发生一次、两层包装均生效、配置排除、整文件指令、单行 linter 指令、非本 analyzer 指令、上游错误传播和诊断保留。兼容性风险主要是误抑制或漏抑制；重复包装还可能增加每次 pass 的遍历与闭包层级。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件和 7,032 个 Rust 文件；`files --filter build/linter/durationcheck` 列出 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/durationcheck/analyzer.rs`：读取目标文件全部 36 行，确认仅有 `Analyzer` 与 `init` 两个主要符号，无条件编译项，并显示 `used by 0 files`。
- RustCodeGraph 对 `build/linter/durationcheck/analyzer.rs::init` 的 `callers`、`callees`、`impact --depth 3`：均未给出已索引调用边；`query durationcheck --json` 确认目标符号及两个独立 Rust 测试。
- `build/linter/durationcheck/analyzer.go`：核对 Go 的上游 analyzer 转接和两个初始化调用；`build/linter/util/util.go`：核对两层 `Run` 包装、文件过滤、指令过滤与错误透传语义。
- `build/linter/util/util.rs`：核对 Rust `SkipAnalyzer`、`SkipAnalyzerByConfig` 均要求 `&mut analysis::Analyzer`，证明当前共享引用调用存在类型边界。
- `build/linter/durationcheck/analyzer_test.rs` 与 `pkg/lib.rs`：核对两个文本契约测试、`include_str!` 读取方式及仅在 `cfg(test)` 下的接线；仓库未发现对应 Go 测试。
- `build/linter/durationcheck/BUILD.bazel`、`build/BUILD.bazel`、`go.mod`、`go.sum`、`DEPS.bzl`：核对 Go library、nogo 接线和 `github.com/charithe/durationcheck v0.0.11` 依赖；根 `Cargo.toml` 核对本目录不是 Rust workspace crate且没有对应 Rust 依赖。
- 按任务约束未运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核没有将迁移草稿描述为已运行能力。
