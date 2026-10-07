# `build/linter/errcheck/analyzer.rs`

## 文件定位

本文件位于构建辅助目录 `build/linter/errcheck`，是同目录 [`analyzer.go`](./analyzer.go) 的机械迁移草稿。它试图保留一个很薄的适配层：转发第三方 errcheck analyzer、把仓库维护的 [`errcheck_excludes.txt`](./errcheck_excludes.txt) 写入 analyzer 的 `excludes` flag，再接入 AsterSQL/TiDB 的文件过滤和 lint 指令过滤（`Analyzer`、`excludesContent`、`init`）。它不处于 SQL 服务的请求、规划、执行或存储运行时路径中。

当前 Rust 文件没有进入可执行 Rust 构建链。根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 没有 `build/linter/errcheck` 成员或 Rust errcheck 依赖，仓库也没有 Rust 模块声明该文件；[`pkg/lib.rs`](../../../pkg/lib.rs) 仅在 `#[cfg(test)]` 下通过路径引入 [`analyzer_test.rs`](./analyzer_test.rs)，而测试只是用 `include_str!` 检查源文本。RustCodeGraph 对目标文件报告 `used by 0 files`。因此下文严格区分 Rust 草稿的迁移意图与 Go 侧已经接线的实际行为。

Go 侧由 [`BUILD.bazel`](./BUILD.bazel) 定义 `//build/linter/errcheck` library，并把 `analyzer.go` 与嵌入文件纳入目标；[`build/BUILD.bazel`](../../BUILD.bazel) 仅在 `//build:with_nogo` 条件成立时把该 library 加入 `tidb_nogo`。第三方依赖由 [`go.mod`](../../../go.mod) 和 [`DEPS.bzl`](../../../DEPS.bzl) 固定为 `github.com/kisielk/errcheck v1.10.0`，Bazel 版本还应用仓库补丁。

## 核心职责

- `Analyzer` 试图以本包名称转发 `errcheck::Analyzer`，保持上游 analyzer 的共享对象身份以及其中 `Flags`、运行回调和依赖等注册状态（`analyzer.rs:26-29`；Go 对照 `analyzer.go:25-26`）。本文件不实现“未检查错误返回值”的分析算法。
- `excludesContent` 用 `include_str!("errcheck_excludes.txt")` 表达编译期携带排除清单的意图，避免运行时依赖工作目录或外部文件（`analyzer.rs:31-34`）。清单目前包含 `fmt.Fprint*`、若干 `Close`、`Flush`、`Write*` 等允许忽略返回值的函数或方法。
- `init` 按固定顺序设置上游 analyzer 的 `excludes` flag、处理设置失败、套用配置文件过滤，再套用 lint 指令过滤（`analyzer.rs:36-51`）。顺序是语义的一部分，因为两个 wrapper 都会替换 analyzer 的运行回调。
- 真正的 errcheck 规则、flag 解析和诊断生成属于第三方 `github.com/kisielk/errcheck/errcheck`；本文件只承担仓库级配置和装配职责。

## 主要符号

- `pub static Analyzer: &analysis::Analyzer = errcheck::Analyzer`：公开静态共享引用，对应 Go 的 `var Analyzer = errcheck.Analyzer`。源注释说明上游 v1.10.0 暴露 `*analysis.Analyzer`，因此草稿选择引用而非复制 analyzer（`analyzer.rs:26-29`）。当前没有 Rust `analysis` 或 `errcheck` 依赖为这些名称提供定义。
- `pub static excludesContent: &str = include_str!("errcheck_excludes.txt")`：公开的进程级字符串切片。`include_str!` 在编译期读取相对当前文件的清单，成功后值具有静态生命周期；与 Go 的 `embed.FS` 相比，它没有运行时 `ReadFile` 步骤和对应错误分支（`analyzer.rs:31-34`）。
- `pub fn init()`：公开、无参数、无返回值的初始化函数。它依次取出嵌入文本、调用 `Analyzer.Flags.Set("excludes", data.to_string())`、对错误调用 `log::Fatal`，然后调用 `util::SkipAnalyzerByConfig` 与 `util::SkipAnalyzer`（`analyzer.rs:36-51`）。普通 Rust 函数名 `init` 不会像 Go `init()` 一样在包加载时自动执行，必须由未来装配代码显式调用或换成等价的一次性初始化机制。
- 文件没有结构体、枚举、trait、`impl`、条件编译项、私有辅助函数或本地 `Run` 回调。

## 执行流程

按源码表达的预期初始化流程：

1. `Analyzer` 指向第三方 errcheck analyzer 的共享实例，而不是在本文件创建规则对象。
2. `excludesContent` 在编译目标时将同目录排除清单嵌入二进制；`init` 把该静态字符串绑定为 `data`。
3. `Analyzer.Flags.Set("excludes", data.to_string())` 通过上游公开 flag 入口解析清单，而不是直接修改 analyzer 内部字段。这样应由上游负责解释每行函数/方法签名。
4. 若 flag 设置返回错误，`log::Fatal(err.unwrap_err())` 表达与 Go `log.Fatal(err)` 对齐的致命终止意图；该路径不会继续安装 wrapper。
5. 成功后先调用 `util::SkipAnalyzerByConfig(Analyzer)`，意图让 analyzer 只分析仓库配置允许的文件；再调用 `util::SkipAnalyzer(Analyzer)`，意图支持 `lint:ignore`、`nolint` 和整文件跳过指令。

当前 Rust 实际流程在构建前即中断：目标文件未被任何模块包含，相关命名空间未由 Cargo 依赖提供，而且两个 Rust wrapper 的签名都是 `&mut analysis::Analyzer`，目标文件却传入 `&analysis::Analyzer`。RustCodeGraph 对 `init` 的 callers/callees 没有返回调用边，精确 impact 只列出 `init` 自身。因此不能声称这些步骤已在 Rust 程序中执行。

## 数据与状态

本文件涉及两份静态状态：共享 analyzer 引用 `Analyzer`，以及静态排除文本 `excludesContent`。排除文本在编译后不可变；按 Go 语义，初始化则会修改共享 analyzer 的 flag 状态和 `Run` 回调链，属于进程级注册状态，不是每次分析临时创建的数据。

排除清单逐行描述允许忽略错误的函数或方法，例如 `fmt.Fprint`、`(*os.File).Close`、`(io.Closer).Close`、gRPC/SQL/TiDB 存储对象的 `Close` 以及 `bufio.Writer.Flush`。清单内容直接决定上游 errcheck 不报告哪些调用点，修改它会改变 lint 覆盖面，但不改变本适配层控制流。

两个 skip wrapper 都会保存旧 `Run`，再用闭包替换 analyzer 的 `Run`。按调用顺序，配置过滤先包裹原始 errcheck 回调，指令过滤再成为外层 wrapper；分析时外层先处理 lint 指令与文件，再进入配置过滤，最终调用第三方规则。目标 Rust 文件没有幂等标志，重复初始化按设计意图可能重复设置 flag、追加依赖并形成多层 wrapper。

## 依赖与调用关系

Rust 源码表达的下游关系为：

`init -> Analyzer.Flags.Set -> log::Fatal（仅错误分支） -> util::SkipAnalyzerByConfig -> util::SkipAnalyzer`

以及静态关系 `Analyzer -> errcheck::Analyzer`、`excludesContent -> errcheck_excludes.txt`。RustCodeGraph 能定位目标文件的两个静态符号和 `init`，但文件级结果为 `used by 0 files`，精确 callers/callees 也没有边；仓库搜索未发现 Rust 模块声明或调用 `errcheck::init`。[`pkg/lib.rs`](../../../pkg/lib.rs) 对同目录测试的引入只让测试读取源码字符串，不会编译目标文件。

Go 的实际构建链为：

`//build:with_nogo -> //build:tidb_nogo -> //build/linter/errcheck -> analyzer.go::Analyzer -> github.com/kisielk/errcheck/errcheck.Analyzer`

其中 `BUILD.bazel` 声明 `//build/linter/util` 和 `@com_github_kisielk_errcheck//errcheck`，并用 `embedsrcs` 携带排除文件。`build/BUILD.bazel` 把 errcheck 放在 `with_nogo` 的 `select` 分支，而不是基础 analyzer 列表；未启用该构建设置时，Go `tidb_nogo` 不依赖此 analyzer。

## 错误处理与边界

唯一显式错误分支来自 `Flags.Set`。Go 版本忽略 `embed.FS.ReadFile` 的错误，然后在 flag 解析失败时调用 `log.Fatal` 终止进程；Rust 草稿通过 `include_str!` 把文件缺失变为编译期错误，因此运行时只保留 flag 设置失败的致命路径（`analyzer.go:31-38`；`analyzer.rs:36-51`）。`init` 本身不返回 `Result`，调用者无法恢复或自行处理错误。

本文件不验证排除项是否仍匹配依赖版本中的函数签名，也不生成未检查错误的诊断。上游 analyzer 如何识别函数、接口方法、类型断言和排除字符串属于第三方边界；仓库特有边界由排除清单和两个 skip wrapper 叠加形成。扩大清单会减少诊断，删除或拼错清单项则可能引入新的构建 lint 失败。

Rust 草稿还存在明确的可用性边界：未解析的 `analysis`、`errcheck`、`log`、`util` 命名空间；共享不可变引用无法满足 wrapper 的 `&mut analysis::Analyzer` 参数；普通 `init` 不会自动运行；根 Cargo 没有相应 crate/feature/依赖。文档不能以 Go 目标可运行推导 Rust 文件可编译或可执行。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、网络连接或文件句柄。`include_str!` 不在运行时打开文件，`excludesContent` 与 `Analyzer` 都预期贯穿进程生命周期，也没有清理动作。

共享 analyzer 的可变配置必须在任何并行分析开始前完成。Go 的包初始化天然先于使用该包导出的 analyzer，但重复装配仍会多次改写 `Run`；Rust 草稿既没有自动初始化保证，也没有 `Once`/`Lazy` 等一次性同步机制，且无法从不可变静态引用安全取得 wrapper 所需的可变借用。若未来接线，应先明确单次初始化、共享所有权及并发读取不与注册期修改重叠的不变量。

真正的文件遍历和诊断可能由 nogo 并行调度，但资源属于 analysis framework 和第三方 analyzer；本适配文件只配置共享元数据与回调链。

## 与 Go 版本的对应关系

Rust 的三个符号逐项对应 [`analyzer.go`](./analyzer.go)：`Analyzer` 对应包级上游 analyzer 指针，`excludesContent` 对应 `//go:embed` 的 `embed.FS`，`init` 对应自动包初始化函数。两侧都保持“设置 excludes，失败即致命终止，随后依次配置过滤与通用跳过”的顺序。独立 Rust 测试明确断言共享引用形状、六个关键调用片段、两个 wrapper 的顺序，以及调用时不应额外写 `&Analyzer`。

关键差异是执行语义。Go 文件有真实 import、Bazel target、上游 v1.10.0 依赖和自动 `init`，在 `with_nogo` 构建配置下参与分析；Rust 文件是未接线、当前不保证编译的草稿。Go 的 `embed.FS.ReadFile` 在运行时返回字节和错误（此处错误被忽略），Rust `include_str!` 在编译期读取 UTF-8 文本。Go analyzer 指针可被 `Flags.Set` 和 wrapper 原地修改；Rust 共享不可变引用与现有 `&mut` wrapper 签名不兼容。

[`analyzer_test.rs`](./analyzer_test.rs) 是迁移形状的源码文本测试，不会把 `analyzer.rs` 作为 Rust 模块编译，也不运行 errcheck 行为。因此它能防止关键文本和顺序漂移，但不能证明类型正确、初始化执行、排除规则生效或第三方算法与 Go 等价。

## 扩展指南

调整排除策略时，优先修改 [`errcheck_excludes.txt`](./errcheck_excludes.txt)，逐项确认签名符合上游 v1.10.0 的格式，并评估减少诊断带来的正确性风险；若改变加载方式或初始化顺序，应同步核对 `analyzer.go::init`、`BUILD.bazel` 的 `embedsrcs`、[`../util/util.go`](../util/util.go) 的 wrapper 语义和 [`analyzer_test.rs`](./analyzer_test.rs) 的顺序断言。升级 Go 依赖还应同时核对 `go.mod`、`go.sum`、`DEPS.bzl` 及仓库补丁。

若要让 Rust 版本真正可用，最可能修改的是 `Analyzer` 的所有权/可变初始化模型、上游 errcheck/analysis 的真实依赖适配和 `init` 的一次性显式注册入口。不要通过不安全的全局可变别名绕过 `&mut` 冲突；应设计初始化后只读的对象，保证 flag 设置及 wrapper 安装只发生一次，并把目标模块加入明确的 Cargo crate。

Rust 测试逻辑必须继续放在独立文件中。现有 `analyzer_test.rs` 应从文本契约测试扩展为真实模块测试或另设接线测试，至少覆盖：排除文本确实传入 flag、错误分支策略、wrapper 顺序、重复初始化行为，以及一个应报告和一个被排除的错误返回值场景。若第三方规则仍不在 Rust 侧实现，应如实限定测试为装配契约，不能用字符串包含断言代替编译和行为证据。

兼容性风险集中在排除项格式、上游 flag API 和 nogo wrapper 顺序；正确性风险是过度排除漏报错误或初始化未执行；性能风险较小，主要是重复 wrapper 导致每次 pass 重复过滤文件和诊断。

## 验证依据

- 已读源与直接证据：[`analyzer.rs`](./analyzer.rs) 全部 52 行、[`analyzer.go`](./analyzer.go)、[`analyzer_test.rs`](./analyzer_test.rs)、[`errcheck_excludes.txt`](./errcheck_excludes.txt)、[`BUILD.bazel`](./BUILD.bazel)、[`../util/util.rs`](../util/util.rs) 与 [`../util/util.go`](../util/util.go)。它们分别证明符号、初始化顺序、测试契约、排除数据、Go target 以及 wrapper 的实际签名和过滤行为。
- 已读装配与版本证据：根 [`Cargo.toml`](../../../Cargo.toml)、[`pkg/lib.rs`](../../../pkg/lib.rs)、[`build/BUILD.bazel`](../../BUILD.bazel)、根 [`go.mod`](../../../go.mod) 和 [`DEPS.bzl`](../../../DEPS.bzl)。这些文件证明 Rust workspace/模块和依赖缺席、测试仅按路径读取、Go 的条件化 nogo 接线，以及 errcheck v1.10.0 与 Bazel 补丁配置。
- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter build/linter/errcheck` 列出 Go/Rust 源和 Rust 测试。
- RustCodeGraph `node --file build/linter/errcheck/analyzer.rs --offset 1 --limit 220`：读取完整文件并报告 `used by 0 files`；`node build/linter/errcheck/analyzer.rs::init` 确认 `init` 源码；精确 callers/callees 无输出，精确 impact 对该符号只列出其自身。名称级 impact 同时返回大量无关同名 `init`，未用作目标调用关系证据。
- 仓库 `rg` 核对：Rust 侧唯一入口关联是 `pkg/lib.rs` 引入 `analyzer_test.rs`；Go 侧 `build/BUILD.bazel` 在 `with_nogo` 分支依赖 `//build/linter/errcheck`。未发现 Rust 模块声明或 `init` 调用者。
- 人工复核结论：本文件为何存在——复刻 Go errcheck 的配置适配层；如何运行——当前 Rust 侧不运行，Go 侧由条件化 nogo target 自动初始化；如何安全扩展——保持排除 flag 与 wrapper 顺序，先解决真实依赖、可变所有权、单次初始化和行为测试接线。
