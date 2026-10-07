# `build/linter/allrevive/analyzer.rs`

## 文件定位

本文件是 [`analyzer.go`](analyzer.go) 的 Rust 机械迁移草稿，属于 `build/linter/allrevive` 这一构建期静态检查器目录。它试图把 revive 的多条 Go lint 规则聚合成名为 `all_revive` 的 `go/analysis` analyzer，再把 revive 产生的位置转换成 `analysis::Pass` 可报告的位置。源码第 15～17 行明确说明它“当前不保证可编译”，且不会真正读取 Go 包、启动 revive 或执行 lint，因此它不是当前可运行的 Rust lint 实现。

实际接线分为两侧：Go 侧由 [`BUILD.bazel`](BUILD.bazel) 声明 `go_library(name = "allrevive")`，并在 [`../../BUILD.bazel`](../../BUILD.bazel) 的 `with_nogo` 分支加入 `//build/linter/allrevive`；Rust 根包 `astersql` 的 `[lib]` 指向 [`../../../pkg/lib.rs`](../../../pkg/lib.rs)，该文件只在 `#[cfg(test)]` 下引入 [`analyzer_test.rs`](analyzer_test.rs)，没有把 `analyzer.rs` 声明为生产模块。根 [`Cargo.toml`](../../../Cargo.toml) 也没有为 `build/linter/allrevive` 定义独立 crate 或对应 revive/analysis 依赖。

## 核心职责

按当前源码表达的迁移意图，本文件承担四项职责：

1. `Analyzer` 保存 analyzer 名称、说明和 `run` 回调；`init` 依次套用配置文件过滤与 `nolint`/跳过指令支持。
2. `defaultRules` 和 `allRules` 组装 revive 规则集；当前 Rust 与 Go 都列出 29 个条目，其中 `BlankImportsRule` 在扩展列表和默认列表各出现一次，测试要求保留顺序和重复项。
3. `run` 从 `analysis::Pass` 收集文件名，固定使用 Go 1.21、置信度 `0.8` 和 error 严重度来调用 revive，并在格式化前丢弃低置信度 failure。
4. revive JSON formatter 给出行列信息后，`run` 重新读文件、计算字节偏移，再通过 `pass.Reportf` 报告 `规则名: 失败信息`。

这些是代码结构和 Go 对照所证明的目标语义，不等同于 Rust 侧已经具备运行能力。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：导出的 analyzer 描述符，名字为 `all_revive`，`run` 字段绑定本文件的 `run`。当前写成不可变 `static`，但相邻 Rust 工具函数 `SkipAnalyzerByConfig`/`SkipAnalyzer` 接受 `&mut analysis::Analyzer`，这是尚未解决的可编译性冲突。
- `pub fn init()`：按 Go 的初始化顺序先调用 `util::SkipAnalyzerByConfig`，再调用 `util::SkipAnalyzer`。前者基于 `build/nogo_config.json` 的 `all_revive.exclude_files` 过滤文件；后者包装报告和输入文件，以处理 `nolint`/跳过指令。
- `pub struct jsonObject`：JSON 反序列化中间结构，包含 `lint::Severity` 与显式的 `lint::Failure` 字段。Go 使用匿名内嵌 `lint.Failure`，Rust 后续访问因此写成 `res.Failure.Position`、`res.Failure.RuleName`。
- `pub fn defaultRules() -> Vec<Box<dyn lint::Rule>>`：返回 10 个默认规则对象。
- `pub fn allRules() -> Vec<Box<dyn lint::Rule>>`：先创建 19 个扩展规则对象，再以 `rules.extend(defaultRules())` 追加默认规则，合计 29 个条目。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：主执行入口；成功时返回 `Ok(None)`，诊断通过 `pass.Reportf` 产生，而非作为返回值携带。

文件没有 trait、`impl`、条件编译项或模块级规则常量；与 Go 的切片变量不同，两组规则在 Rust 草稿中每次调用都会重新分配 trait object。

## 执行流程

`run` 的预期流程如下：

1. 遍历 `pass.Files`，用 `pass.Fset.PositionFor(file.Pos(), false).Filename` 得到每个 Go AST 文件的路径，并包装成 revive 需要的“包列表中的文件列表”。
2. 用 `goversion::NewVersion("1.21")` 构造固定版本；以 `os::ReadFile` 和并发参数 `1024` 创建 revive linter；建立 `lint::Config`，其中生成文件不忽略、置信度为 `0.8`、严重度为 `error`，错误码和警告码均为 `-1`。
3. 遍历 `allRules()`，用每条规则的 `Name()` 填充 `conf.Rules`。随后覆盖 `defer` 的配置，启用 `loop`、`method-call`、`immediate-recover`、`return` 四个参数。因为配置存入按名称索引的 map，重复的 `BlankImportsRule` 最终对应同一个配置键。
4. `config::GetLintingRules` 将配置解析成可执行规则，`revive.Lint` 对当前 pass 的文件运行规则并返回 failure 流。
5. 创建 failure 通道和格式化输出通道，取得 JSON formatter，并启动线程执行 `formatter.Format(format_rx, formatter_conf)`。格式化错误会被记录为日志并转换为空字符串；线程最后把字符串发送回主线程。
6. 主线程遍历 failures，仅发送 `Confidence >= 0.8` 的项；随后销毁发送端以通知 formatter 输入结束，再阻塞接收 JSON 输出，形成完成同步。
7. `json::Unmarshal` 将输出解析成 `Vec<jsonObject>`。对每个结果，`util::ReadFile` 把源文件加入当前 `FileSet`，`util::FindOffset` 把一基的行/列转换为字节偏移，最后以 `token::Pos(tf.Base() + offset)` 调用 `pass.Reportf`。

## 数据与状态

长期状态只有包级 `Analyzer`；规则对象、文件列表、配置、failure、JSON 字符串和解析结果都局限于一次 `run` 调用。`conf.Rules` 是按规则名索引的配置 map；先统一写空配置，再专门覆盖 `defer` 参数是重要装配顺序。`jsonObject.Severity` 被反序列化但未参与报告，报告内容取自 `Failure.RuleName` 与 `Failure.Failure`。

`init` 的目标是修改 analyzer 的 `Requires` 和 `Run` 包装链。相邻 [`../util/util.rs`](../util/util.rs) 表明两个跳过函数都会取走旧 `Run` 并安装 closure，因此调用顺序决定运行时先执行哪一层过滤；Rust 当前不可变 `static` 不能满足这两个函数的可变借用要求。配置数据的 Rust 对照位于 [`../../config.rs`](../../config.rs)，通过 `LazyLock` 读取 [`../../nogo_config.json`](../../nogo_config.json)；其中 `all_revive` 有 `exclude_files`，没有 `only_files`。

## 依赖与调用关系

RustCodeGraph 将本文件识别为 6 个符号，并确认文件内主边为 `defaultRules → allRules → run`：`allRules` 追加 `defaultRules`，`run` 遍历 `allRules`。`Analyzer` 保存 `run` 作为回调入口，`init` 调用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`。索引没有找到其他 Rust 生产文件使用本文件，这与 [`../../../pkg/lib.rs`](../../../pkg/lib.rs) 仅引入测试文件的事实一致。

下游概念依赖包括 `analysis`、`lint`、`rule`、`config`、`goversion`、`json`、`token`、`os`、`log`、`zap` 和本仓库 `util`。这些名称在文件中是迁移占位依赖；当前根 Cargo manifest 没有把该目录建成独立 crate，也没有为本文件建立生产模块链。Go/Bazel 侧的真实依赖则由本目录 `BUILD.bazel` 明确列出，包括 revive 的 `config`/`lint`/`rule`、`go-version`、PingCAP log、zap、`go/analysis` 与 `build/linter/util`。

上游真实入口是 Bazel `nogo` analyzer 集合：[`../../BUILD.bazel`](../../BUILD.bazel) 仅在 `//build:with_nogo` 选择分支中加入本 Go library。源码中的 `//nolint:all_revive` 注释是该 analyzer 的消费侧抑制标记，不构成对 Rust `Analyzer` 的调用。

## 错误处理与边界

- `config::GetLintingRules`、`revive.Lint`、`config::GetFormatter` 和 `json::Unmarshal` 使用 `?` 传播错误，目标是让 `analysis::Error` 返回给调用框架。
- 固定 Go 版本解析失败、诊断文件重读失败会 `panic!`；failure 通道发送失败、formatter 输出发送或接收失败会因 `expect` panic。这些都是致命路径，不是可恢复诊断。
- formatter 自身失败只写 `log::Error` 并返回空字符串；随后空字符串通常会在 JSON 反序列化阶段变成返回错误。因此日志记录不是成功降级。
- 置信度恰好为 `0.8` 的 failure 会被保留，只有严格小于阈值的项被过滤。空文件集合仍会作为一个空的包文件列表交给 revive。
- 位置映射依赖 formatter 返回有效的一基行列。相邻 `util::FindOffset` 在找不到位置时返回 `-1`，本文件没有检查该哨兵值，直接与 file base 相加；这是扩展或真正接线时必须处理的边界。
- 当前草稿还与 `util.rs` 的实际接口不一致：`ReadFile` 需要 `&mut FileSet` 且返回裸 `*mut token::File`，`FindOffset` 接受 `&[u8]`，而本文件传入 `&pass.Fset`、按对象调用 `tf.Base()`，并把内容转换成 `&str`。因此不能把现有文本测试视为编译证明。

## 并发与资源生命周期

并发只出现在 JSON 格式化阶段。`format_tx/format_rx` 传递通过阈值的 `lint::Failure`，`output_tx/output_rx` 传递唯一的格式化字符串并承担完成信号。主线程必须在发送完 failure 后 `drop(format_tx)`；否则 formatter 无法观察输入结束，主线程在 `output_rx.recv()` 上会一直等待。formatter 线程持有接收端、配置副本和输出发送端，发送结果后自然退出；代码没有保存或 join 线程句柄，完成性完全由输出通道保证。

`std::sync::mpsc::channel` 是无界通道，生产 failure 时不会提供背压；大量诊断可能积累内存。配置通过 `clone` 交给 linter 和 formatter，避免线程共享可变配置。文件读取句柄由 `util::ReadFile` 在函数内按 RAII 释放；AST、`FileSet` 和报告回调的所有权属于传入的 `analysis::Pass`。当前实现没有取消、超时或线程 panic 转换：formatter 若在发送前 panic，主线程只会看到 `recv` 失败并再次 panic。

## 与 Go 版本的对应关系

[`analyzer.go`](analyzer.go) 是本文件最直接且权威的语义对照。名称、说明、两层跳过包装、29 个活动规则的顺序与重复项、Go 1.21、置信度、defer 参数、JSON formatter、过滤条件以及最终报告文本均被 Rust 草稿保留。独立 [`analyzer_test.rs`](analyzer_test.rs) 通过 `include_str!` 同时读取两份源码，验证活动规则顺序、两个通道端点、`Failure` 显式字段访问，以及配置、过滤、日志、JSON、偏移和报告等关键源码片段。

仍存在重要差异：Go 的 `defaultRules`/`allRules` 是包级切片，Rust 改为每次构造 `Vec<Box<dyn Rule>>`；Go 的匿名内嵌 failure 在 Rust 中成为命名字段；Go 用 goroutine 和 `exitChan` 配合共享 `output/err`，Rust 用线程加输出通道传值；Go 的 formatter 错误写入外层 `err` 后并未在格式化完成后显式返回检查，Rust 则把错误转成空输出并可能在反序列化时返回错误。最关键的是 Go 文件已由 Bazel 注册并具有真实第三方依赖，而 Rust 文件没有生产接线且存在接口不匹配。

本目录没有 Go 测试文件；现有直接测试证据只有 Rust 的源码一致性测试。它能证明迁移文本保留了指定形状，不能证明 revive 执行、并发终止、错误传播或位置映射在 Rust 中可运行。

## 扩展指南

若只调整规则集合，应同时修改 `defaultRules` 或 `allRules`、Go 对照文件及 [`analyzer_test.rs`](analyzer_test.rs) 的顺序预期；特别注意重复名称进入配置 map 后会覆盖同键，而规则列表仍可能保留重复对象。若增加参数化规则，应在统一登记规则名之后像 `defer` 一样覆盖对应 `RuleConfig`，并覆盖默认值、错误配置和阈值边界测试。

若要把草稿升级为可运行 Rust 实现，不能只补少量占位类型：需要先确定 canonical crate 和模块入口，在 Cargo 中引入有 tag 的真实上游依赖或实现等价适配层，解决 `Analyzer` 可变初始化、`analysis::Analyzer` 字段形状、`ReadFile`/`FindOffset`/裸指针接口、revive 输入输出和错误类型。按仓库规则，外部 Rust 依赖必须在独立上游仓库移植、提交并打 tag，不能复制到 `vendor`/`third_party` 或用本地 `[patch]`。

测试必须继续放在独立的 `analyzer_test.rs`，不要嵌入生产文件。建议新增可执行测试覆盖：空输入、阈值等于/低于 `0.8`、formatter 错误与线程 panic、无效 JSON、不可读文件、Unicode 行列和不存在的位置、`nolint:all_revive` 以及 `nogo_config.json` 排除路径。兼容风险主要是 Go/Rust 规则版本与名字漂移；正确性风险是位置偏移或包装顺序错误；性能风险是每次分配规则对象、重复读文件和无界 failure 队列。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/allrevive` 找到 Go/Rust 源与 Rust 测试；`node --file build/linter/allrevive/analyzer.rs` 覆盖全部 228 行并列出 6 个符号。
- RustCodeGraph `query defaultRules`、`query allRules` 与 `explore`：精确定位本文件符号，并给出 `defaultRules` 被 `allRules` 调用、`allRules` 被 `run` 调用的内部边；对精确符号执行 `callers`/`callees` 未产生额外生产调用结果。
- 已核读 [`analyzer.rs`](analyzer.rs)、[`analyzer.go`](analyzer.go)、[`analyzer_test.rs`](analyzer_test.rs)、本目录 [`BUILD.bazel`](BUILD.bazel)、[`../../BUILD.bazel`](../../BUILD.bazel)、根 [`Cargo.toml`](../../../Cargo.toml)、[`../../../pkg/lib.rs`](../../../pkg/lib.rs)、[`../util/util.rs`](../util/util.rs)、[`../util/util.go`](../util/util.go)、[`../../config.rs`](../../config.rs)、[`../../config_test.rs`](../../config_test.rs) 与 [`../../nogo_config.json`](../../nogo_config.json)。
- 人工核对活动规则：Rust 与 Go 均为 29 个条目，顺序一致，`BlankImportsRule` 均出现两次；本目录文件清单确认不存在 Go 测试文件或 `doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；验证重点是事实来源、链接和固定章节结构，不能据此声称 Rust analyzer 已编译或执行。
