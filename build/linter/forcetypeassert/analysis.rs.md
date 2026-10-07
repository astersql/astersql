# `build/linter/forcetypeassert/analysis.rs`

## 文件定位

`build/linter/forcetypeassert/analysis.rs` 是 Go 文件 [`analysis.go`](analysis.go) 的机械迁移草稿，位于构建期静态检查工具目录，而不在数据库 SQL 请求、事务或存储运行链路中。文件本身第 15～17 行明确声明“当前不保证可编译”，并且根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace members 中没有 `build/linter/forcetypeassert`，该目录也没有自己的 `Cargo.toml` 或 Rust 模块入口。因此，当前 Rust 文件不是可构建 crate 的组成部分；它保存的是 Go analyzer 导出与初始化包装顺序的迁移意图。

实际投入构建的是同目录 Go 包：[`BUILD.bazel`](BUILD.bazel) 只把 `analysis.go` 列入 `go_library.srcs`，并依赖 `//build/linter/util` 与 `@com_github_gostaticanalysis_forcetypeassert//:forcetypeassert`；顶层 [`build/BUILD.bazel`](../../BUILD.bazel) 又把 `//build/linter/forcetypeassert` 放入 nogo analyzer 依赖集合。故阅读本文件时必须区分“Go/Bazel 当前生效链路”与“Rust 尚未接线的结构草稿”。

## 核心职责

本文件只表达两项职责，没有实现类型断言分析算法本身：

1. 包级静态项 `Analyzer` 直接引用外部 `forcetypeassert::Analyzer`，意图保持上游 analyzer 的对象身份，而不是克隆或在本地重新构造 analyzer。
2. `init()` 依次调用 `util::SkipAnalyzerByConfig(Analyzer)` 和 `util::SkipAnalyzer(Analyzer)`，意图在原 analyzer 的执行入口外叠加“配置排除文件”和“linter 指令跳过”两层包装。

真正识别未经检查的 Go 类型断言并产生诊断的逻辑属于外部 `github.com/gostaticanalysis/forcetypeassert` v0.2.0；版本证据分别位于 [`go.mod`](../../../go.mod)、[`go.sum`](../../../go.sum) 和 [`DEPS.bzl`](../../../DEPS.bzl)。本 Rust 草稿既不解析源码，也不创建或执行 analysis pass。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = forcetypeassert::Analyzer;`：公开的共享 analyzer 引用。其声明没有本地构造、`clone()` 或惰性容器；[`analysis_test.rs`](analysis_test.rs) 的 `upstream_analyzer_is_reexported_by_reference_without_clone` 以文本断言固定了这一形态。注意：当前目录没有声明 `analysis` 或 `forcetypeassert` Rust 模块/依赖，所以该签名是迁移目标的表达，不是已验证可编译 API。
- `pub fn init()`：公开普通函数，按顺序把同一个 `Analyzer` 传给两个 util 包装函数。它并非 Rust 的自动模块初始化机制；Rust 中只有在未来模块入口显式调用它时才会执行，而当前 RustCodeGraph 报告该文件 `used by 0 files`，也没有找到真实 Rust 调用边。
- 文件没有自定义类型、trait、impl、常量、条件编译项或 analyzer 的运行函数；实际检查器状态与回调均来自外部 analyzer。

## 执行流程

按照当前源码所表达的预期流程：

1. 模块加载后，`Analyzer` 指向外部 forcetypeassert analyzer。
2. 某个未来入口显式调用 `init()`。
3. `SkipAnalyzerByConfig` 先包装 analyzer 的原始 `Run`：每次执行时复制 pass，只保留 `shouldRun(analyzer.Name, filename)` 为真的文件，再调用此前的 `Run`。
4. `SkipAnalyzer` 后包装上一步形成的 `Run`：它增加 `Directives` 前置依赖，执行时剔除带跳过文件指令的文件，并包装报告函数以过滤匹配 analyzer 名称和源码位置的跳过诊断，最后调用上一步保存的 `Run`。
5. 因第二次包装位于最外层，运行时先处理指令过滤，再进入配置过滤，最后才进入外部 forcetypeassert 的原始检查逻辑；不过源码中的注册调用顺序仍必须保持“配置包装在前、通用跳过包装在后”。

步骤 3～4 的行为来自 [`build/linter/util/util.go`](../util/util.go) 的当前 Go 实现及其 Rust 对照草稿 [`build/linter/util/util.rs`](../util/util.rs)。对本文件而言，当前实际可执行流程止于 Go 包的自动 `init()`；Rust 侧尚无调用 `init()` 的入口，不能声称上述 Rust 流程已经运行。

## 数据与状态

本文件没有实例字段或局部持久状态，核心状态是 `Analyzer` 所指向的共享 analyzer 对象及其可变运行配置。Go 版本的两个 util 函数都会修改同一 `*analysis.Analyzer`：保存旧 `Run`、安装新闭包，且 `SkipAnalyzer` 还向 `Requires` 追加 `Directives`。因此对象身份和包装顺序都是行为不变量，不能替换为相互独立的副本。

每次 analyzer 执行时，util 层复制 analysis pass 后替换其文件集合或报告回调，避免直接改写调用方传入的 pass；配置、解析后的 directives、文件位置和 analyzer 名称共同决定哪些文件或诊断被过滤。`analysis.rs` 自身不拥有这些执行期数据，也没有缓存、计数器或序列化格式。

Rust 草稿目前还存在所有权/可变性不闭合：这里把 `&analysis::Analyzer` 传给 util，而当前 [`util.rs`](../util/util.rs) 的两个函数签名要求 `&mut analysis::Analyzer`。独立测试特意检查文本保持无 `&Analyzer` 的 Go 迁移形态，并不证明类型可用；因此不能把当前静态引用描述为已能被安全修改。

## 依赖与调用关系

上游关系分为两套：

- Go/Bazel 生效链：[`build/BUILD.bazel`](../../BUILD.bazel) 的 nogo 目标依赖 `//build/linter/forcetypeassert`，Bazel 加载 Go 包时自动执行 `analysis.go:init`，随后构建系统使用该包导出的 analyzer。目录内 [`BUILD.bazel`](BUILD.bazel) 给出了 util 和 forcetypeassert 的直接依赖。
- Rust 草稿链：RustCodeGraph 为 `analysis.rs` 建立了 `Analyzer` 和 `init` 节点，但文件级结果为 `used by 0 files`；精确查看 `init` 时没有解析出被调用函数，仓库也没有 Cargo manifest 或 `mod` 声明把该文件纳入 crate。因此当前没有可确认的 Rust 上游调用者。

下游语义依赖是外部 `forcetypeassert::Analyzer`、`analysis::Analyzer` 类型，以及 `util::SkipAnalyzerByConfig`、`util::SkipAnalyzer`。这些名称在目标文件中没有 `use` 声明或模块定义，进一步印证它是结构草稿。仓库业务源码中的 `//nolint:forcetypeassert` 注释属于 Go linter 的消费面；它们说明规则名称被实际使用，但不是本 Rust 文件的直接调用者。

## 错误处理与边界

`Analyzer` 声明和 `init()` 自身没有 `Result`、错误返回或显式分支。初始化阶段的失败边界全部隐含在依赖是否存在、类型是否匹配及 util 是否能取得 analyzer 的 `Run` 回调中。

Go 版本的包装闭包将外部 analyzer 的 `(result, error)` 原样返回，不吞掉检查错误。配置过滤可以把 `pass.Files` 缩减为空，此时仍会调用下层 analyzer；指令过滤仅匹配相同文件/行和 analyzer 名称，其他诊断继续传给旧 report。Rust `util.rs` 草稿在缺失 `Directives` 结果、结果类型不符或 analyzer 没有 `Run` 时使用 `expect`，会 panic；这些是下游草稿的边界，不是本文件新增的处理策略。

最重要的当前边界是不可编译、未接线：没有 Cargo 依赖、导入和可变引用方案时，不应把 `init()` 当作可运行实现。文档也不推断外部 forcetypeassert v0.2.0 的内部算法细节；这里只能确认本地透传和包装行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或网络资源。初始化意图是一次性修改全局 analyzer 的运行回调；Go 的包初始化天然发生在 analyzer 被并发执行之前，随后每个 pass 使用包装闭包中的共享配置与每次调用创建的局部 pass 副本。

若未来接线 Rust 实现，必须明确保证 `init()` 只执行一次，并在任何并发 analysis pass 开始前完成。重复调用会反复追加 `Directives`、重复嵌套 `Run` 包装，改变依赖和过滤成本；并发修改共享 analyzer 也会产生数据竞争或要求同步。当前源码没有 `Once`、锁或其他生命周期保护，因此调用方不能假定幂等或线程安全。

## 与 Go 版本的对应关系

Rust 的 `Analyzer` 对应 [`analysis.go`](analysis.go) 中 `var Analyzer = forcetypeassert.Analyzer`，`init()` 的两条调用与 Go 原实现顺序一致。[`analysis_test.rs`](analysis_test.rs) 固定了两项迁移语义：共享引用不得变成 clone/惰性副本；配置跳过必须先于无条件的通用跳过，而且两次都针对同一 `Analyzer` 文本。

关键差异是 Go 版本真实可用而 Rust 版本仅为草稿：Go 依赖在 `go.mod`/Bazel 中完整声明，Go `init` 自动执行，且 `*analysis.Analyzer` 可被 util 原地修改；Rust 文件缺少 crate 归属与 imports，普通 `init()` 不会自动运行，`&analysis::Analyzer` 也不满足当前 Rust util 的 `&mut analysis::Analyzer` 参数。Go 包没有同目录专门的 `*_test.go`；本任务的相关独立测试是 Rust 文本契约测试，而非 analyzer 行为测试。

## 扩展指南

若仅调整本文件表达的接入策略，最可能修改 `Analyzer` 声明或 `init()`，并同步更新 [`analysis_test.rs`](analysis_test.rs) 的文本契约。改变两层包装的顺序会改变执行时过滤层的嵌套关系；移除任一调用会分别失去配置排除或 directives/nolint 适配；复制 analyzer 则可能使构建入口和包装对象不再是同一个实例。

若目标是让 Rust 版本真正可用，不能只在此文件补几个 import：需要在独立任务中确定所属 crate/module、为 `analysis`、`forcetypeassert` 和 util 建立真实 Cargo 依赖、设计可安全初始化的可变全局所有权，并添加独立行为测试，验证配置排除、`skip_file`、按 linter 名称跳过、空文件集、诊断透传和重复初始化。测试逻辑应继续放在独立的 `analysis_test.rs`，不要嵌入生产源文件。

兼容风险主要是规则名称和诊断过滤语义；正确性风险集中于包装顺序、同一对象身份和重复初始化；性能风险来自重复包装以及每个 pass 对文件/directives 的遍历。任何可运行化修改还应与 Go v0.2.0 行为逐项对照，而不能用简化桩替代实际 analyzer。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/forcetypeassert` 返回 `analysis.go`、`analysis.rs`、`analysis_test.rs`。
- RustCodeGraph `node --file build/linter/forcetypeassert/analysis.rs`：读得完整 35 行源码并报告 `used by 0 files`；精确 `node build/linter/forcetypeassert/analysis.rs::init` 确认函数体只有两条包装调用；`callees` 输出中该文件的 `init` 为 `No callees found`。`callers` 查询在本地索引后端持续无输出，已终止，因此没有用其推导不存在的调用关系。
- 已核对源码与测试：[`analysis.rs`](analysis.rs)、[`analysis.go`](analysis.go)、[`analysis_test.rs`](analysis_test.rs)、[`util.go`](../util/util.go)、[`util.rs`](../util/util.rs)。
- 已核对构建和依赖：根 [`Cargo.toml`](../../../Cargo.toml)、[`go.mod`](../../../go.mod)、[`go.sum`](../../../go.sum)、[`DEPS.bzl`](../../../DEPS.bzl)、目录 [`BUILD.bazel`](BUILD.bazel) 和顶层 [`build/BUILD.bazel`](../../BUILD.bazel)。证据显示 Go 依赖版本为 v0.2.0，Bazel 只编译 Go 源，而目标 Rust 文件没有 Cargo crate 归属。
- 本文仅说明当前可验证事实；未运行 Cargo（任务明确禁止），也未把 Rust 文本测试当作编译或运行时正确性的证据。
