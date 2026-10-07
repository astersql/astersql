# `build/linter/bodyclose/analysis.rs`

## 文件定位

本文件位于构建辅助目录 `build/linter/bodyclose`，是同目录 [`analysis.go`](./analysis.go) 的机械迁移草稿。它试图保留一个极薄的适配层：把第三方 `bodyclose` analyzer 以本包的 `Analyzer` 名称暴露，并在初始化时接入 TiDB/AsterSQL 的 lint 跳过指令处理（`Analyzer`、`init`）。它不属于 SQL 请求、规划、执行或存储运行时。

当前 Rust 文件没有进入可执行构建链。根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员和依赖中都没有 `build/linter/bodyclose` 或 Rust `bodyclose` crate；根 [`pkg/lib.rs`](../../../pkg/lib.rs) 也没有声明本模块或对应测试模块。RustCodeGraph 对本文件报告 `used by 0 files`。因此，本文对 Rust 侧的描述是迁移意图和源码现状，不能理解为 Rust linter 已经可编译或运行。

与此相对，Go 侧是实际生产接线：[`BUILD.bazel`](./BUILD.bazel) 定义 `//build/linter/bodyclose` Go library，[`build/BUILD.bazel`](../../BUILD.bazel) 将它无条件列入 `tidb_nogo` 的 analyzer 依赖；外部 analyzer 版本由根 [`go.mod`](../../../go.mod) 固定为 `github.com/timakin/bodyclose v0.0.0-20241222091800-1db5c5ca4d67`。

## 核心职责

- `Analyzer` 充当第三方 `bodyclose::Analyzer` 的包级转发名称，意图让统一 linter 装配层按本包获取 analyzer（`analysis.rs:23-25`；Go 对照 `analysis.go:22-23`）。
- `init` 调用 `util::SkipAnalyzer(Analyzer)`，意图在 analyzer 原有检查逻辑外包一层仓库自定义跳过协议（`analysis.rs:27-30`）。Go 的 `util.SkipAnalyzer` 会识别文件级和同一行 analyzer 级跳过指令、过滤输入文件或抑制诊断（[`../util/util.go`](../util/util.go) 的 `SkipAnalyzer`）。
- 本文件不解析 Go AST、不构建 SSA，也不判断 HTTP 响应体是否关闭。实际规则由外部 `github.com/timakin/bodyclose/passes/bodyclose` 提供；当前 Rust 文件顶部也明确说明“不会真正运行 HTTP body close 检查”（`analysis.rs:15-21`）。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = bodyclose::Analyzer`：公开静态引用，语义上对应 Go 的 `var Analyzer = bodyclose.Analyzer`。它没有复制 analyzer 状态，而是试图转发第三方共享实例（`analysis.rs:23-25`）。当前文件没有导入或定义 Rust `analysis`、`bodyclose` 模块，根 Cargo 也没有相应依赖，因此该声明只表达迁移形状。
- `pub fn init()`：公开、无参数、无返回值的初始化函数，仅调用一次 `util::SkipAnalyzer(Analyzer)`（`analysis.rs:27-31`）。Rust 不会像 Go 那样自动执行普通名为 `init` 的函数；即使将来能编译，也必须由模块装配代码显式调用或改用等价注册机制。
- 文件中没有常量、结构体、枚举、trait、`impl`、条件编译项或私有辅助函数；也没有在本地定义 analyzer 的 `Run` 回调。

## 执行流程

按源码意图，初始化路径只有两步：

1. 模块暴露 `Analyzer`，指向第三方 bodyclose analyzer（`Analyzer`）。第三方 Go analyzer 的名称是 `bodyclose`，依赖 `buildssa.Analyzer`，其 `Run` 会遍历 SSA 并报告 `response body must be closed`；这些算法属于上游依赖而非本文件。
2. `init` 把该 analyzer 交给 `util::SkipAnalyzer`。对应 Go 包加载时 `init()` 自动执行，包装 analyzer 的原始 `Run`，使 `//lint:ignore`/文件跳过类指令能够先过滤文件或诊断，再调用原 analyzer（`analysis.go:25-27`；`util.go::SkipAnalyzer`）。

当前 Rust 实际执行流程到此中断：没有模块入口或调用者触发 `init`，没有 Rust 第三方 analyzer 依赖，而且 [`../util/util.rs`](../util/util.rs) 的 `SkipAnalyzer` 需要 `&mut analysis::Analyzer`，本文件却传入静态不可变引用 `&analysis::Analyzer`。所以不能把上述意图描述为已运行行为。

## 数据与状态

本文件唯一状态是进程级静态 `Analyzer` 引用；它本身不拥有 HTTP 响应、文件集合、SSA 图或诊断缓冲区。若按 Go 版本接线，初始化会原地修改第三方 analyzer 的依赖和 `Run` 回调，因此 analyzer 是共享、可变的注册期状态，而不是每次分析新建的值（`analysis.go::Analyzer`、`init`；`util.go::SkipAnalyzer`）。

真正执行检查时的数据属于第三方 analyzer：每个 analysis pass 读取 `buildssa` 结果，定位 `net/http.Response`、`Body` 字段和 `Close` 方法，再沿 SSA 引用判断响应体是否可能保持打开。第三方 `runner.run` 使用值接收者，注释明确其目的是让不同 pass 并行运行时各自持有状态；该状态与当前适配文件没有所有权关系。

## 依赖与调用关系

Rust 源码写出的下游关系是 `init -> util::SkipAnalyzer`，以及 `Analyzer -> bodyclose::Analyzer`。RustCodeGraph 能定位 `init` 与文件，但精确 `callers`/`callees` 查询没有返回边；文件级结果为 `used by 0 files`。直接源码和 `rg` 搜索也未发现 Rust 调用者或模块声明。

Go 生产链为：

`WORKSPACE nogo 配置 -> //build:tidb_nogo -> //build/linter/bodyclose -> analysis.go::Analyzer -> 第三方 bodyclose.Analyzer -> buildssa.Analyzer`。

其中 [`build/linter/bodyclose/BUILD.bazel`](./BUILD.bazel) 只把 `analysis.go` 放入 Go library，并声明 `//build/linter/util` 与 `@com_github_timakin_bodyclose//passes/bodyclose`；`analysis.rs` 不在该 target 的 `srcs` 中。`build/BUILD.bazel` 对 bodyclose 的依赖位于 `tidb_nogo.deps` 的固定列表，而不是 `with_nogo` 条件分支，所以 Go analyzer 是 nogo 定义的基础组成部分。

## 错误处理与边界

本适配层没有 `Result`、错误分支或诊断构造；`init` 也不返回错误。Go 版本将第三方 analyzer 和包装器产生的行为直接透传。

上游第三方 analyzer 在目标包未导入 `net/http.Response` 时正常跳过；无法把 `Response` 解释为预期命名结构、找不到 `Body` 字段等结构不变量被破坏时返回错误。它对未关闭的响应体报告诊断，并为返回 `*http.Response` 的函数、全局保存、某些闭包路径以及 `httptest.ResponseRecorder.Result` 等情况设置例外。这些边界是被转发 analyzer 的能力，不是当前 Rust 草稿自行实现或验证的能力。

Rust 侧另有明确的编译边界：未解析的 `analysis`/`bodyclose`/`util` 命名空间、不可变 `Analyzer` 与 `SkipAnalyzer(&mut ...)` 的签名不匹配，以及普通 `init` 不会自动运行。扩展文档或调用者时不得掩盖这些事实，也不能以 Go target 可用推导 Rust 文件可用。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、网络连接、HTTP 响应或文件句柄，也没有显式清理动作。`Analyzer` 是静态引用，预期贯穿进程生命周期；包装动作应发生在 analyzer 被并行用于各 analysis pass 之前，避免注册期改写和分析期读取发生竞争。

第三方 analyzer 的每次 `Run` 使用独立的 `runner` 值处理一个 pass，以支持 pass 间并行；HTTP `Response.Body` 的运行时关闭并不发生在 linter 内，linter 只是静态检查业务代码是否具有关闭路径。Go `SkipAnalyzer` 会替换共享 analyzer 的 `Run` 并捕获旧回调，因此重复调用初始化可能形成多层包装；本文件没有幂等保护。Rust 草稿也没有证明共享静态 analyzer 可安全取得可变引用。

## 与 Go 版本的对应关系

Rust 两个符号逐项对应 [`analysis.go`](./analysis.go)：`Analyzer` 对应包级 `var Analyzer = bodyclose.Analyzer`，`init` 对应 Go 包初始化函数，且两者都只调用一次 `SkipAnalyzer`，没有调用 `SkipAnalyzerByConfig`。这保留了“第三方规则由依赖实现、仓库文件仅负责暴露和跳过适配”的结构。

差异在于可执行性和语言语义：Go import、Bazel 依赖及自动 `init` 完整接线，并被 `tidb_nogo` 使用；Rust 文件只有未解析的 Go 风格命名空间和手工命名的 `init`，既没有 Cargo 依赖也没有模块入口。Go 的 `Analyzer` 是可变指针，符合 `SkipAnalyzer(*analysis.Analyzer)`；Rust 声明为不可变静态引用，却调用要求可变引用的 Rust wrapper。因此 Rust 版本当前是接口草图，不是第三方 bodyclose 算法的 Rust 移植。

## 扩展指南

若只调整 Go linter 的装配策略，应同步核对 `analysis.go::init`、[`../util/util.go`](../util/util.go) 的 wrapper 协议、`build/linter/bodyclose/BUILD.bazel` 和 `build/BUILD.bazel`；不要把第三方 SSA 算法复制进这个薄适配文件。升级 bodyclose 时还需核对 `go.mod`、Bazel 外部依赖锁定及上游测试场景，尤其是 `net/http` 类型识别、闭包/字段传递和 `httptest.ResponseRecorder` 例外。

若要让 Rust 版本真正可用，最可能修改的是 `Analyzer` 的所有权/初始化方式和 `init` 的注册机制，同时需要提供真实 Rust `analysis`、`bodyclose`、`util` 模块或依赖，并把模块接入某个 Cargo crate。必须避免对共享静态值制造未证明安全的可变别名，且应设计一次性初始化以防重复包装。

测试必须放在独立文件中。当前同目录没有 `analysis_test.rs` 或 Go 包内测试；新增 Rust 接线时应新增 `build/linter/bodyclose/analysis_test.rs` 并在实际 crate 的测试入口声明它，至少覆盖 analyzer 元数据转发、单次 wrapper 注册、重复初始化策略和代表性的“已关闭/未关闭”响应体。若规则仍由外部实现，行为用例应以依赖升级回归为主，适配层测试则聚焦接线，不伪造已经移植的 SSA 分析能力。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、其中 7,032 个 Rust 文件；`files --filter build/linter/bodyclose` 只列出 `analysis.rs` 和 `analysis.go`。
- RustCodeGraph `node --file build/linter/bodyclose/analysis.rs --offset 1 --limit 240`：读取完整 31 行并报告 `used by 0 files`；文件仅有 `Analyzer` 和 `init` 两个符号。
- RustCodeGraph `node --file build/linter/bodyclose/analysis.go` 与 `node build/linter/bodyclose/analysis.go::init`：确认 Go 对照仅转发 analyzer 并调用 `util.SkipAnalyzer`。
- RustCodeGraph `node build/linter/util/util.rs::SkipAnalyzer` 和 Go 对照查询：确认 wrapper 会增加指令依赖、过滤文件/诊断并调用旧 `Run`，同时确认 Rust 参数为 `&mut analysis::Analyzer`。目标 `init` 的精确 callers/callees 查询在约 30 秒内没有输出，所以本文以文件级 `used by 0 files` 和直接搜索作为“未接线”证据，没有把超时解释成完整调用图证明。
- 已读构建与入口：根 [`Cargo.toml`](../../../Cargo.toml)、[`pkg/lib.rs`](../../../pkg/lib.rs)、[`build/linter/bodyclose/BUILD.bazel`](./BUILD.bazel)、[`build/BUILD.bazel`](../../BUILD.bazel)、根 [`go.mod`](../../../go.mod) 和 [`build/nogo_config.json`](../../nogo_config.json)。它们分别证明 Rust workspace/模块缺席、Go library 与 nogo 接线、外部版本及 analyzer 配置名称。
- 已读测试证据：仓库内没有引用本 Rust 文件的独立测试；本地 Go module cache 中的 `github.com/timakin/bodyclose/.../passes/bodyclose/bodyclose_test.go` 用 `analysistest.Run` 执行上游 analyzer。该测试证明外部依赖自身有行为测试，不构成本 Rust 适配文件的编译或接线测试。
- 人工复核结论：文件存在的理由是复刻 Go 的第三方 analyzer 转发与跳过包装；当前 Rust 文件不运行规则；安全扩展必须先解决真实依赖、可变性、显式初始化和独立测试接线。
