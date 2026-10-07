# `build/linter/revive/analyzer.rs`

## 文件定位

本文件是 Go 包 `build/linter/revive` 中 `analyzer.go` 的 Rust 对照实现，目标是把 revive 规则包装成 `go/analysis` 风格的分析器：入口静态量为 `Analyzer`，实际工作由 `run` 完成（`analyzer.rs:34-41,111-203`）。它属于构建/静态检查工具面，而不是 SQL 请求、规划或存储运行时的一部分。

当前接线必须区分语言版本。Go 目标 `//build/linter/revive` 由 `build/linter/revive/BUILD.bazel` 声明，并在 `build/BUILD.bazel` 的 `tidb_nogo` 依赖列表中仅于 `//build:with_nogo` 条件成立时启用。根 `Cargo.toml` 的 `astersql` crate 指向 `pkg/lib.rs`；该 crate 只在测试配置下通过 `pkg/lib.rs:115-117` 引入 `analyzer_test.rs`，测试再以 `include_str!("analyzer.rs")` 读取本文件。仓库中没有把本文件声明为 Rust 模块的直接证据。因此，本文件目前有源码和文本级回归测试，但不能据此宣称 Rust 版本已进入可执行 linter 主链。

## 核心职责

- `Analyzer` 描述名为 `revive`、无前置 analyzer 的分析器，并把回调绑定到 `run`（`analyzer.rs:34-41`）。
- `init` 依次调用 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`，表达“按仓库配置排除文件”和“兼容 nogo 跳过指令”的初始化意图（`analyzer.rs:43-47`；对应 Go `analyzer.go:39-42`）。
- `defaultRules` 与 `allRules` 固定启用规则及其顺序；被注释的规则明确不属于当前启用集合（`analyzer.rs:57-109`）。
- `run` 将当前 analysis pass 的 AST 文件转换为文件路径，构造 revive 配置，执行检查，过滤低置信度结果，经 JSON formatter 归一化后再映射回 `token::Pos` 并报告诊断（`analyzer.rs:111-203`）。
- `sanitizeForOffset` 在保留原始字节长度的前提下处理非法 UTF-8，以免诊断行列映射的字节偏移发生漂移（`analyzer.rs:205-221`）。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：公开分析器描述符，名称为 `revive`，`requires` 为空，回调为 `run`（`analyzer.rs:36-41`）。RustCodeGraph 将其所在文件列为仅被 `analyzer_test.rs` 使用；仓库搜索没有发现生产 Rust 模块引用。
- `pub fn init()`：初始化适配入口，保留 Go 版两个 skip wrapper 的先后顺序（`analyzer.rs:44-47`）。注意当前签名和调用写法是否能与 `build/linter/util/util.rs` 的可变 analyzer API 一起编译，尚无模块接线或 Cargo 验证证据。
- `pub struct jsonObject`：对应 formatter JSON 中的 severity 与内联 failure；`run` 反序列化为 `Vec<jsonObject>` 后读取 `Failure.Position`、`RuleName` 和消息（`analyzer.rs:49-55,179-200`）。
- `pub fn defaultRules()`：返回 5 条基础规则，顺序为 `VarDeclarationsRule`、`DotImportsRule`、`ExportedRule`、`IncrementDecrementRule`、`ContextKeysType`（`analyzer.rs:58-70`）。
- `pub fn allRules()`：先放 8 条扩展规则，再 `extend(defaultRules())`，最终形成 13 条规则的有序列表（`analyzer.rs:74-109`）。RustCodeGraph 的调用边为 `run -> allRules -> defaultRules`。
- `pub fn run(&mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：分析主入口；成功时总是返回 `Ok(None)`，诊断通过 `pass.Reportf` 产生副作用（`analyzer.rs:112-203`）。
- `pub(super) fn sanitizeForOffset(&[u8]) -> String`：逐个把非法 UTF-8 起始字节替换成 NUL，直到剩余后缀合法；RustCodeGraph 确认调用边为 `run -> sanitizeForOffset`（`analyzer.rs:207-221`）。

## 执行流程

1. `run` 遍历 `pass.Files`，通过 `pass.Fset.PositionFor(file.Pos(), false).Filename` 收集磁盘路径，并包装成 revive 接受的“包列表中的文件列表”二维结构（`analyzer.rs:113-119`）。
2. 固定解析 Go 版本 `1.21`，以 `os::ReadFile` 和并发参数 `1024` 创建 revive lint 实例；随后构造置信度 `0.8`、severity `error`、错误/警告码均为 `-1` 的配置（`analyzer.rs:120-133`）。
3. 遍历 `allRules()`，按每条规则的 `Name()` 注册空配置；再覆盖/补充 `defer` 规则参数，启用 `loop`、`method-call`、`immediate-recover`、`return` 四类检查（`analyzer.rs:134-142`）。因为 `defer` 已由默认规则名称注册时，这次插入会替换其配置；若此前不存在，则新增。
4. `config::GetLintingRules` 将配置解析为实际规则，`revive.Lint` 对收集到的文件执行检查（`analyzer.rs:144-145`）。
5. 创建 failure 通道和 JSON formatter，并启动 formatter 线程消费通道。主线程遍历 failures，只把 `Confidence >= 0.8` 的项发送给 formatter（`analyzer.rs:147-172`）。
6. 主线程显式丢弃发送端以关闭通道，再 `join` formatter 线程取得完整 JSON；之后 `json::Unmarshal` 得到 `Vec<jsonObject>`（`analyzer.rs:174-180`）。关闭发送端是 formatter 完成的生命周期条件。
7. 对每条结果重新读取源文件，以 `sanitizeForOffset` 保持字节长度，再用 token file 的 base 加 `util::FindOffset` 计算位置，最终报告 `"规则名: 失败消息"`（`analyzer.rs:180-201`）。

## 数据与状态

`Analyzer` 是包级静态描述符；规则集合不是全局可变数组，而是每次调用 `defaultRules`/`allRules` 都新建装箱的 trait object 列表。因此单次 `run` 会独立构造规则对象和 `lint::Config`，不会在不同 pass 之间共享配置状态（`analyzer.rs:36-41,58-109,125-143`）。

`packages` 的形状为 `Vec<Vec<String>>`，当前 pass 的全部文件始终被视为一个包（`analyzer.rs:113-119`）。`conf.Rules` 以规则名称为键，因此同名规则后插入者覆盖先前配置；文件中明确利用这一点为 `defer` 写入非空参数（`analyzer.rs:134-142`）。

formatter 阶段的数据所有权通过 `std::sync::mpsc` 转移：主线程拥有发送端并移动 `lint::Failure`，formatter 线程拥有接收端、formatter 与配置副本；最终只把格式化字符串作为 join 返回值带回主线程（`analyzer.rs:147-177`）。

## 依赖与调用关系

上游入口在设计上是 `Analyzer.run = run`，初始化适配由 `init` 完成（`analyzer.rs:36-47`）。Go 生产链的直接装配证据是 `build/linter/revive/BUILD.bazel` 定义 `go_library(name = "revive")`，以及 `build/BUILD.bazel` 在 `with_nogo` 分支把该目标加入 `tidb_nogo`。RustCodeGraph 对 Rust 文件只报告 `analyzer_test.rs` 使用，因此 Rust 生产调用者当前未验证。

下游依赖可按职责分组：

- analysis/token：`analysis::Analyzer`、`analysis::Pass`、`token::Pos` 负责 analyzer 生命周期和诊断位置。
- revive：`lint::New`、`lint::Config`、`lint::Rule`、`config::GetLintingRules`、`config::GetFormatter` 与各 `rule::*` 类型负责规则选择、执行和格式化。
- 仓库工具：`util::SkipAnalyzerByConfig`、`util::SkipAnalyzer`、`util::ReadFile`、`util::FindOffset` 负责排除策略、文件读取和偏移计算。
- 基础设施：`goversion::NewVersion`、`os::ReadFile`、`json::Unmarshal`、`log::Error`/`zap::Error`、标准库 channel 与线程负责版本配置、I/O、序列化、日志及并发。

`build/linter/revive/BUILD.bazel` 只描述 Go 目标及其 Go 外部依赖；根 `Cargo.toml` 没有为此目录声明独立 crate，也没有可据以确认上述 Rust 命名空间真实依赖来源的条目。这是当前 Rust 迁移状态的边界，不应由 Go Bazel 依赖反推 Rust 已可链接。

## 错误处理与边界

可恢复错误通过 `?` 传播：规则解析、lint 执行、formatter 获取和 JSON 反序列化失败都会令 `run` 返回 `analysis::Error`（`analyzer.rs:144-149,179`）。成功路径返回 `Ok(None)`，业务结果体现为报告到 pass 的诊断而非返回值（`analyzer.rs:180-203`）。

三个路径会 panic：固定字符串 `"1.21"` 解析失败；重新读取诊断对应文件失败；formatter 发送端异常或 formatter 线程 panic（`analyzer.rs:120-123,169-177,182-186`）。formatter 自身返回错误时仅记录日志并返回空字符串，随后空字符串通常会在 JSON 反序列化阶段转成传播错误（`analyzer.rs:151-159,179`）。

置信度严格小于 `0.8` 的 failure 被丢弃，等于阈值的结果保留（`analyzer.rs:162-166`）。`IgnoreGeneratedHeader: false` 表示配置不因生成文件头自动忽略文件（`analyzer.rs:125-133`）。

`sanitizeForOffset` 的不变量是输出长度与输入字节长度相同：每次只把检测到的一个非法字节替换为单字节 NUL，而保留所有合法字节。因此 `FindOffset` 所依据的 UTF-8 字节位置不会因 lossy replacement 的多字节替代字符而移动（`analyzer.rs:205-221`）。该函数不保留非法字节内容，只保留边界。

## 并发与资源生命周期

每次 `run` 创建一个无界 MPSC 通道和一个 formatter 线程（`analyzer.rs:147-160`）。主线程是唯一 producer，formatter 线程是唯一 consumer；发送端在所有满足置信度的 failure 发送完毕后由 `drop(format_tx)` 显式关闭。formatter 的 `Format` 依赖通道关闭判断输入结束，主线程随后 `join`，所以 JSON 被反序列化前 formatter 必须已退出（`analyzer.rs:162-179`）。

这段代码没有长期后台任务、锁或共享可变状态；通道、线程、配置副本和结果字符串都局限于一次 `run`。无界通道可在 producer 明显快于 formatter 时积累待处理 failure；同时 revive 构造参数 `1024` 的具体并发/容量含义属于下游 API，本文件没有进一步约束，文档不推断其实现。

异常生命周期包括：发送失败直接 panic；formatter 线程 panic 在 `join` 时 panic；formatter 正常返回错误则记录日志并让后续 JSON 解析决定 `run` 的错误结果。独立测试 `formatter_thread_returns_owned_output_after_all_failures_are_sent` 仅检查源码包含关闭、join 和反序列化顺序，并未真实启动本实现（`analyzer_test.rs:54-71`）。

## 与 Go 版本的对应关系

Rust 的 `Analyzer`、`init`、`jsonObject`、规则顺序、Go 1.21、revive 配置、置信度过滤、JSON formatter 和最终 `Reportf` 消息形状，逐项对应 `build/linter/revive/analyzer.go:32-170`。Rust 将 Go 的包级规则切片改为按调用新建 `Vec<Box<dyn lint::Rule>>`，但通过“扩展规则在前、默认规则在后”保持最终顺序（Rust `analyzer.rs:58-109`；Go `analyzer.go:50-91`）。

Go 用 goroutine、`formatChan` 和额外 `exitChan` 协调格式化；Rust 用线程、MPSC 通道关闭和 `join` 返回 owned `String`，避免跨线程写外部可变字符串（Rust `analyzer.rs:147-177`；Go `analyzer.go:129-155`）。这属于同一生命周期语义的语言适配。

位置转换存在有意增强：Go 直接把 `[]byte` 转为 string 后交给 `FindOffset`；Rust 先调用 `sanitizeForOffset`，逐字节替换非法 UTF-8，以同时满足 Rust `String` 合法性和 Go 字节偏移语义（Rust `analyzer.rs:181-199,205-221`；Go `analyzer.go:161-168`）。

错误路径并非全部等价：Go 在 formatter 出错时写共享 `err`，但完成后仍先尝试 unmarshal；Rust formatter 线程记录错误并返回空字符串。Rust 还把 channel 发送失败和线程 panic 明确处理为 `expect` panic。由于当前测试为文本断言而非行为测试，这些差异没有端到端验证。

## 扩展指南

新增或调整 revive 规则时，应修改 `defaultRules` 或 `allRules`，并同步 `analyzer_test.rs::analyzer_and_rule_order_match_go` 的名称与顺序断言；若 Go 仍是权威生产实现，还必须同步 `analyzer.go`，确认规则默认参数和插入覆盖顺序一致。`defer` 参数应在 `run` 的专用配置块修改，并同步 `configuration_and_error_paths_match_go`（`analyzer.rs:134-142`；`analyzer_test.rs:89-107`）。

改变 formatter、置信度筛选或线程协议时，应重点保护三个不变量：低置信度 failure 不进入 formatter、发送端关闭后 formatter 才结束、JSON 只在 join 之后解析。应为这些行为增加独立 Rust 行为测试，不能只扩充 `include_str!` 文本断言。

改变位置映射时，应修改 `sanitizeForOffset`/`util::FindOffset` 接缝，并覆盖 ASCII、多字节 UTF-8、CRLF、空文件、行列边界和多个/连续非法字节；测试仍应放在独立的 `analyzer_test.rs`，不要嵌入生产文件。性能风险主要是每条诊断重新读取完整源文件和每次 run 新建 formatter 线程；优化时必须保持 `token::Pos` 与 revive 行列语义一致（`analyzer.rs:180-200`）。

若要把 Rust 实现真正接入应用，首先应增加明确的 Rust 模块/crate 边界和带 tag 的外部依赖，而不是把 Go `BUILD.bazel` 当作 Rust 构建声明；之后需要真实编译和行为测试验证 `Analyzer` 可变性、revive API、JSON inline 语义与错误类型。该接线超出本次纯文档任务。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter/revive` 找到 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`。
- RustCodeGraph `node --file build/linter/revive/analyzer.rs`：读取完整 221 行，并报告该文件被 `build/linter/revive/analyzer_test.rs` 使用。
- RustCodeGraph `node build/linter/revive/analyzer.rs::allRules`：确认 `run -> allRules -> defaultRules` 调用链；`node ...::run` 确认 `run -> allRules` 与 `run -> sanitizeForOffset`。
- 已读生产/构建证据：`build/linter/revive/analyzer.rs`、`build/linter/revive/analyzer.go`、`build/linter/revive/BUILD.bazel`、`build/BUILD.bazel`、根 `Cargo.toml`、`pkg/lib.rs`。
- 已读测试证据：`build/linter/revive/analyzer_test.rs`。其 5 个测试验证 analyzer/规则顺序、formatter 协议、字节偏移代码形状、配置/错误路径和许可证/占位文本；均为源码字符串断言，不是对 `run` 的编译或执行测试。同目录不存在 Go 测试文件。
- 人工事实复核：仓库搜索仅发现 Go Bazel 目标被 `tidb_nogo` 条件依赖，Rust 侧仅 `pkg/lib.rs` 引入独立测试；没有发现生产 Rust 模块直接包含 `analyzer.rs`。
- 本任务按计划不运行 Cargo；文档结构使用任务规定的 11 标题命令验证。
