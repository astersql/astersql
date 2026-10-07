# `build/linter/staticcheck/analyzer.rs`

## 文件定位

本文件是 Go 文件 [`build/linter/staticcheck/analyzer.go`](analyzer.go) 的 Rust 对照移植，位于构建期 linter 的 staticcheck 适配目录。它不实现任何具体静态检查算法，而是把一个由构建配置指定的检查名解析为共享的 `analysis::Analyzer`，再为该 analyzer 安装 AsterSQL/TiDB 的跳过规则。

当前生产接线仍在 Go/Bazel 一侧：[`build/linter/staticcheck/BUILD.bazel`](BUILD.bazel) 为 `def.bzl::ANALYZERS` 中的每个名字生成一个 `go_library`，用 `x_defs = {"name": analyzer}` 替换 Go 占位值；[`build/BUILD.bazel`](../../BUILD.bazel) 再通过 `staticcheck_analyzers(STATICHECK_ANALYZERS)` 把选中的目标加入 `tidb_nogo`。RustCodeGraph 对本文件报告 `used by 0 files`，根 Cargo 包也没有把它声明为生产模块。根 crate 的 [`pkg/lib.rs`](../../../pkg/lib.rs) 仅在 `cfg(test)` 下挂接 `analyzer_test.rs`，测试又只通过 `include_str!` 读取本文件文本。因此，这个 Rust 文件目前是迁移对照实现，不是实际 lint 执行链上的可运行入口。

## 核心职责

1. `name` 保存要选择的 staticcheck analyzer 名称；设计意图是由与 Go `x_defs` 等价的构建 stamping 替换占位字符串。
2. `Analyzer` 通过 `once_cell::sync::Lazy` 在首次访问时调用 `FindAnalyzerByName(name)`，得到上游 analyzer 的共享静态引用，避免复制 analyzer 或使用可变全局。
3. `init` 将选出的 analyzer 交给 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`，分别叠加配置级文件过滤与源码指令级跳过行为。

本文件只负责“按名称选择并包装”，真实 analyzer 集合与查找失败策略在 [`build/linter/staticcheck/util.rs`](util.rs)；过滤和报告抑制逻辑在 [`build/linter/util/util.rs`](../util/util.rs)。

## 主要符号

- `pub static name: &str`：构建 stamping 的占位入口，默认值为 `"dummy value please replace using x_defs"`。该默认值不是合法运行配置；若未替换便触发解析，会沿 `FindAnalyzerByName` 的失败分支 panic。
- `pub static Analyzer: once_cell::sync::Lazy<&'static analysis::Analyzer>`：进程级惰性单例。初始化闭包执行 `FindAnalyzerByName(name)`，返回 `Analyzers` 映射中已有 analyzer 的共享引用；`analyzer_test.rs::stamped_name_and_selected_analyzer_use_thread_safe_lazy_identity` 固定了其 `Lazy<&'static ...>` 形态，并禁止回退到 `static mut`、`unsafe` 或 `Option<&'static ...>`。
- `pub fn init()`：对选中 analyzer 依次调用两个跳过包装器。它没有参数和返回值，也不运行 analyzer 本身。
- `FindAnalyzerByName`（直接依赖，定义于 `util.rs`）：从惰性 `HashMap<String, &'static analysis::Analyzer>` 查找名称；映射按 quickfix、simple、staticcheck、stylecheck、unused 的顺序建立，同名项由后插入项覆盖；不存在时以 `not a valid staticcheck analyzer: {name}` panic。

文件没有类型、trait、常量、条件编译项或业务数据结构；公开面仅为上述两个静态项和 `init`。

## 执行流程

实际设计流程如下：

1. Bazel 从 `def.bzl::ANALYZERS` 逐名生成 Go staticcheck 目标，并通过 `x_defs` 注入 `name`；Rust 侧目前仅保留同名占位协议，尚无等价 Cargo/Bazel 生产目标。
2. 首次解引用 `Analyzer` 时，`Lazy` 调用 `FindAnalyzerByName(name)`。
3. `util.rs::Analyzers` 首次访问时遍历五组上游 analyzer，把 `a.Analyzer.Name` 映射到共享 `analysis::Analyzer`。
4. 名称存在时返回共享引用；名称缺失时立即 panic，不降级、不跳过，也不返回空值。
5. `init` 先调用 `SkipAnalyzerByConfig`，使原 `Run` 只接收配置允许的文件；再调用 `SkipAnalyzer`，增加 `Directives` 前置依赖，并依据 `nolint` 类指令过滤文件或抑制同一行、同一 linter 的诊断。
6. 后续真正运行检查时应调用包装后的 analyzer `Run`；本文件自身没有调度、遍历源码或报告诊断的逻辑。

当前 Rust 仓库没有执行步骤 1、5、6 的生产接线。尤其 `Analyzer` 暴露不可变共享引用，而 `build/linter/util/util.rs` 中两个 Rust 包装函数当前接收 `&mut analysis::Analyzer`；本文件的 `*Analyzer` 不能满足该可变借用契约。该事实与 RustCodeGraph 的零使用者结果一致，不能把此文件描述为已经可编译运行的 linter。

## 数据与状态

- `name` 是静态字符串切片，本身不拥有堆内存；其内容应在构建产物生成时确定，运行时不变化。
- `Analyzer` 是线程安全的一次初始化容器。成功初始化后保存的是上游全局 analyzer 的 `&'static` 引用，所有调用者应观察到同一身份。
- analyzer 名称表由相邻 `util.rs::Analyzers` 持有，是另一个 `Lazy<HashMap<...>>`；查找不复制 analyzer。
- 本文件不维护请求级状态、文件列表、诊断集合、缓存淘汰状态或持久化数据。
- 包装器会修改 analyzer 的 `Requires` 与 `Run`。Go 版本持有可变的 `*analysis.Analyzer`，可以原地改写；Rust 版本若要真正接线，必须先明确共享静态对象如何安全获得可变初始化权，不能通过未经证明的 `unsafe` 绕过。

## 依赖与调用关系

上游关系：

- Go/Bazel：`build/linter/staticcheck/BUILD.bazel` 生成每个 analyzer 的库，`build/BUILD.bazel::tidb_nogo` 消费其中选定目标。这是当前真实生产链。
- Rust：RustCodeGraph 显示 `analyzer.rs` 被 0 个文件使用；`pkg/lib.rs` 仅引入独立测试文件，测试以文本方式观察本文件，未把它编译成模块。

下游关系：

- `FindAnalyzerByName` 来自同目录 `util.rs`，依赖其中的 `Analyzers` 映射以及 quickfix/simple/staticcheck/stylecheck/unused 五组上游 analyzer。
- `analysis::Analyzer` 表达与 Go `golang.org/x/tools/go/analysis.Analyzer` 对齐的 analyzer 数据模型。
- `once_cell::sync::Lazy` 提供并发安全的一次初始化。
- `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer` 来自 `build/linter/util/util.rs`，前者按 linter 配置裁剪 `Pass.Files`，后者按 `Directives` 过滤文件与诊断。

Cargo 边界：目录下不存在独立 `Cargo.toml`；根 [`Cargo.toml`](../../../Cargo.toml) 定义 `astersql` 包且 `[lib] path = "pkg/lib.rs"`，但没有把本文件或相邻实现声明为生产模块，也未见支撑本文件所列外部 Rust API 的直接接线。Bazel 文件只编译 `.go` 源。因此不能仅凭 `.rs` 文件存在推断它属于一个可构建 Rust crate。

## 错误处理与边界

- 名称无效：`FindAnalyzerByName` panic，文本固定为 `not a valid staticcheck analyzer: {name}`；`analyzer_test.rs::lookup_returns_shared_analyzer_and_preserves_panic_text` 对该文本做源级校验。
- stamping 缺失：默认占位字符串不会匹配正常 analyzer 名称，因此一旦真正触发 `Lazy`，会走同一 panic 路径。这里采用快速失败，避免悄悄运行错误或空 analyzer。
- 初始化失败缓存：`Lazy` 初始化闭包 panic 时不产生有效 `Analyzer`；本文件没有恢复、回退或错误返回通道。
- 可变性边界：当前 `Analyzer` 的不可变共享引用与两个包装器的 `&mut` 参数不兼容，是尚未完成生产接线时必须解决的编译/所有权边界。
- 测试边界：现有 Rust 测试检查关键源字符串、顺序、panic 文本和版权头，不执行查表、stamping、并发初始化或包装后的分析流程；其通过不能证明生产可运行。

## 并发与资源生命周期

`once_cell::sync::Lazy` 保证 `Analyzer` 在并发首次访问时只初始化一次，之后返回同一 `&'static analysis::Analyzer`。相邻的 `Analyzers` 映射也使用同步 `Lazy`，因此名称表同样只构造一次。两者都存活到进程结束，没有显式释放步骤。

本文件不创建线程、异步任务、通道、锁、文件句柄、网络连接或事务。潜在并发风险集中在“对共享 analyzer 安装会改写 `Run`/`Requires` 的包装器”：Go 初始化阶段以包级 `init` 串接；Rust 迁移若继续采用全局共享身份，应在发布引用前完成唯一一次可变配置，或改用明确拥有 analyzer 的初始化结构。不能在多个线程可见后再无同步修改，也不能为取得 `&mut` 而引入别名可变引用。

## 与 Go 版本的对应关系

- Go `name` 与 Rust `name` 保留相同占位文本；Go 的 `x_defs` 已在 `BUILD.bazel` 中落地，Rust 尚无对应构建注入机制。
- Go `Analyzer *analysis.Analyzer` 在 `init` 中赋值；Rust 用 `Lazy<&'static analysis::Analyzer>` 把查找推迟到首次访问，并避免 `static mut`。这是所有权实现差异，但目标仍是保持上游共享 analyzer 身份。
- Go `init` 的顺序是 `FindAnalyzerByName`、`SkipAnalyzerByConfig`、`SkipAnalyzer`；Rust 的 `Lazy` 加 `init` 表达同一顺序意图。
- Go 的 analyzer 指针可被两个包装器原地修改；Rust 当前返回共享不可变引用，而包装器要求可变引用，所以行为尚未形成等价可编译实现。
- Go/Bazel 会为完整 `ANALYZERS` 清单生成库，再由 `STATICHECK_ANALYZERS` 选择 `tidb_nogo` 实际启用项；Rust 文件没有对应模块生成、目标选择或运行器接线。

结论：名称协议、查找身份、包装顺序和失败文本已在 Rust 源码及源级测试中保留；构建 stamping、可变初始化和真实运行链尚不能视为已移植完成。

## 扩展指南

- 新增或移除 staticcheck 检查时，先更新上游清单来源与 `def.bzl::ANALYZERS`，再核对 `build/BUILD.bazel::STATICHECK_ANALYZERS` 是否应实际启用；不要在本文件硬编码分支。
- 若建立 Rust 生产接线，应为本目录建立明确 crate/module 边界与真实依赖，并设计等价的构建期名称注入。必须消除默认占位值进入运行时的可能。
- 解决包装器可变性时，应保持“共享 analyzer 身份”和“包装只执行一次”两个不变量。可选设计需通过安全所有权模型提供初始化期可变访问；不要引入 `static mut` 或无依据的 `unsafe`。
- 若查找策略改变，同步修改 `util.rs::FindAnalyzerByName` 和独立测试，保留清晰的非法名称行为；若改变 analyzer 家族顺序，要评估同名覆盖语义。
- 测试必须继续放在独立的 `build/linter/staticcheck/analyzer_test.rs`，不要嵌入生产源文件。接通生产模块后，应补充行为测试：有效/无效名称、并发首次访问、只包装一次、配置过滤、指令抑制，以及注入清单中至少一个真实 analyzer 的执行。
- 兼容风险主要是 analyzer 名称与诊断行为变化；正确性风险是错误选择或重复包装 analyzer；性能风险较低，名称表仅初始化一次，但不应在每个分析 pass 重建全量映射。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；可用于本次文件与符号导航。
- RustCodeGraph `files --filter build/linter/staticcheck`：确认目录内 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`、`util.go`、`util.rs` 五个直接证据文件。
- RustCodeGraph `node --file build/linter/staticcheck/analyzer.rs`：确认本文件共 38 行、`used by 0 files`，以及 `name`、`Analyzer`、`init` 的完整源码。
- RustCodeGraph `node FindAnalyzerByName` 与 `node --file build/linter/staticcheck/util.rs`：确认 Go/Rust 查表、共享引用、家族顺序、覆盖规则和 panic 文本。
- RustCodeGraph `node SkipAnalyzerByConfig`、`node SkipAnalyzer`：确认两个包装器对 `Run`、`Files`、`Requires` 和诊断回调的影响，也确认 Rust 参数为 `&mut analysis::Analyzer`。
- [`build/linter/staticcheck/analyzer.go`](analyzer.go)：核对 Go 的变量类型、`init` 顺序与真实移植语义。
- [`build/linter/staticcheck/BUILD.bazel`](BUILD.bazel)、[`build/linter/staticcheck/def.bzl`](def.bzl)、[`build/BUILD.bazel`](../../BUILD.bazel)：核对逐 analyzer 目标、`x_defs` stamping、完整清单与 `tidb_nogo` 消费链。
- 根 [`Cargo.toml`](../../../Cargo.toml) 与 [`pkg/lib.rs`](../../../pkg/lib.rs)：核对根 crate 边界以及 Rust 测试仅在 `cfg(test)` 下以独立文件挂接的事实。
- [`build/linter/staticcheck/analyzer_test.rs`](analyzer_test.rs)：核对线程安全惰性身份、家族顺序、共享查找、panic 文本与版权要求；该测试是源文本契约测试，不是运行时行为测试。
- 结构校验要求：目标文档存在，且固定二级标题恰好为 11 个。按照任务约束，本次纯文档分析不运行 Cargo。
