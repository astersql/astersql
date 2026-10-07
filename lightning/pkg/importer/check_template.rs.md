# `lightning/pkg/importer/check_template.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` library crate。crate 根在 `lightning/pkg/importer/lib.rs` 中通过 `#[path = "check_template.rs"] mod check_template` 装入模块，并以 `pub use check_template::*` 重新导出其公开符号。`lightning/pkg/importer/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `lightning/pkg/importer`；本文件的直接 Go 对照是同目录的 `check_template.go`。

它位于 Lightning 导入任务的预检查结果汇聚边界：检查器产生 `CheckResult` 后，`Controller::doPreCheckOnItem`（`check_info.rs:66`）把严重级别、通过状态和消息交给模板；控制器构造函数 `NewImportControllerWithPauser`（`import.rs:319`）创建默认模板并保存在 `Controller::checkTemplate`（`import.rs:241`）中；`Controller::preCheckRequirements`（`import.rs:881`）最终据此决定是否继续导入。它不执行检查，也不管理导入任务本身。

## 核心职责

- 用 `Template` trait 规定调用方所需的五项能力：收集结果、判断总体成功、按严重级别统计失败数、渲染表格、汇总关键失败消息。
- 用 `SimpleTemplate` 同步维护结构化行、总数、warning/critical 失败计数和消息分组，避免统计、错误文本与展示读取不同的数据源。
- 保持 Go 版的阻断规则：只有 `Critical` 失败使 `Success` 返回 `false`；`Warn`（值为 `"performance"`，定义于 `lightning/pkg/precheck/precheck.rs`）只计数和展示，不直接阻断。
- 生成面向终端的检查结果表格，包括最大列宽、整行最大可见宽度 170、长单元格换行，以及 warning 黄色、critical 红色的 ANSI 着色。

## 主要符号

- `pub trait Template: Send`：跨模块使用的抽象接口。`Send` 允许实现值在线程之间转移，但接口本身不提供共享同步。五个方法沿用 Go 命名：`Collect(CheckType, bool, String)`、`Success() -> bool`、`FailedCount(CheckType) -> i32`、`Output(&mut self) -> String`、`FailedMsg() -> String`。
- `TemplateRow`：私有的单行快照，保存从 1 开始的 `idx`、原始 `msg`、`typ` 和 `passed`。`#[derive(Clone, Debug)]` 只提供常规复制/调试能力；当前生产流程不直接导出它。
- `pub struct SimpleTemplate`：默认实现。公开字段 `count`、`warnFailedCount`、`criticalFailedCount`、`normalMsgs`、`criticalMsgs` 提供可观察状态，私有 `rows` 保留渲染顺序和完整行信息。
- `NewSimpleTemplate() -> Box<dyn Template>`：把所有计数和容器初始化为空，并以 trait object 隐藏具体实现。RustCodeGraph 显示其生产调用者为 `NewImportControllerWithPauser`，测试调用者包括 `check_template_test.rs`、`import_test.rs`、`chunk_process_test.rs`、`table_import_test.rs` 和 `parity_test.rs`。
- `display_width`、`wrap_cell`、`pad_cell`：私有渲染辅助函数，分别按 Unicode scalar value（`chars()`）计宽、按字符数切分每个源行、按列宽左右补空格；第一列右对齐，其余列左对齐。

## 执行流程

1. `NewImportControllerWithPauser` 调用 `NewSimpleTemplate`，将返回值存入控制器的 `Box<dyn Template>` 字段。
2. 每个预检查项目由 `Controller::doPreCheckOnItem` 构造并执行；有结果时调用 `Collect(result.Severity, result.Passed, result.Message)`，无结果时不追加行。
3. `Collect` 先递增总数。失败项按 `Critical` 或 `Warn` 增加对应计数；失败的 critical 消息进入 `criticalMsgs`，其余所有消息（通过项、warning 失败和未知类型失败）进入 `normalMsgs`；随后把同一结果追加到 `rows`。
4. `preCheckRequirements` 完成各检查后调用 `Success`。若存在 critical 失败，则用 `FailedMsg` 将 critical 消息按 `";\n"` 拼接并构造错误，从而停止后续导入。
5. 调用 `Output` 时，代码从 `rows` 生成四列文本。列宽先取表头与内容的最大显示宽度，再限制为 `[6, 130, 20, 6]`；若边框、空格和列内容总宽仍超过 170，则逐次缩减当前最宽列。
6. 每个单元格按换行符和列宽拆分，同行各单元格的最大分片数决定物理行高。失败 warning 行包裹黄色 ANSI 序列，失败 critical 行包裹红色 ANSI 序列，通过行和未知类型失败行不着色。每个逻辑行后追加分隔线，最终再追加一个空行。

## 数据与状态

`SimpleTemplate` 是有状态的累加器；没有重置方法。`count` 同时充当累计条目数和下一条记录的序号，`rows` 是渲染的权威顺序。失败计数只识别预检查 crate 导出的两个常量：`Critical = "critical"` 与 `Warn = "performance"`。其他 `CheckType`（该类型实际是 `&'static str`）即使失败也不会进入两类失败计数，`FailedCount` 对未知类型返回 0，且该消息会被归入 `normalMsgs`。

`criticalMsgs` 只保存失败 critical 的消息，是 `FailedMsg` 的唯一数据源。`normalMsgs` 包含通过的 critical、所有 warning 结果以及未知类型结果；当前渲染并不读取两个消息向量，而是读取 `rows`。因此扩展时必须同时维护计数、消息分组和 `rows`，否则错误文本、测试观察状态和终端表格会发生漂移。

## 依赖与调用关系

直接 Rust 依赖只有 `astersql_lightning_pkg_precheck` 中的 `CheckType`、`Critical` 和 `Warn`；该依赖由 `lightning/pkg/importer/Cargo.toml` 以本地路径 `../precheck` 声明。表格渲染仅使用标准库的 `String`、`Vec`、字符迭代和格式化，不依赖外部表格 crate。

主要调用链为：`NewImportControllerWithPauser` → `NewSimpleTemplate`；各预检查入口 → `Controller::doPreCheckOnItem` → `Template::Collect`；`Controller::preCheckRequirements` → `Template::Success` / `Template::FailedMsg`。`lib.rs` 的重新导出使 importer crate 内测试能通过 `use crate::*` 取得这些公开符号。RustCodeGraph 的文件视图还确认本文件被 `import.rs` 及多份独立测试引用；精确符号查询确认 `NewSimpleTemplate` 直接实例化 `SimpleTemplate`。

当前 Rust 主流程在 `preCheckRequirements` 中消费成功状态和错误消息；Go 的 `import.go` 还在对应流程中打印 `Output()`。Rust 端的 `Output` 已由单元测试及表导入测试调用，但从本次检查到的 Rust 生产调用边中没有发现对应的统一打印入口，因此不能声称 Rust 主流程目前一定展示完整表格。

## 错误处理与边界

本文件的方法不返回 `Result`，也不主动产生业务错误；它把 critical 失败压缩为布尔状态和字符串，真正的错误构造发生在 `Controller::preCheckRequirements`。空模板的 `Success` 为 `true`、两类失败计数为 0、`FailedMsg` 为空字符串；`Output` 仍会渲染表头和边框。

`FailedMsg` 不转义消息中的分号或换行，只按 `";\n"` 连接。渲染宽度是 `value.chars().count()`，不是终端 grapheme/East Asian Width，也不会剥离消息自身携带的 ANSI 控制序列；因此组合字符、宽字符或内嵌控制码可能与真实终端列宽不同。`wrap_cell` 保留显式空行并按字符块拆分，不按单词边界换行。

宽度缩减循环依赖固定四列表头保证每列初始宽度非零；若未来改变列集合或允许零宽列，需要防止 `widths[widest] -= 1` 下溢。`Output` 的签名使用 `&mut self` 以对齐 trait/Go 可变 writer 语义，但当前 Rust 实现只读取状态并构造新字符串，不缓存渲染结果。

## 并发与资源生命周期

`Template: Send` 仅保证所有实现可被移动到其他线程。`SimpleTemplate` 的修改方法需要独占 `&mut self`，本文件没有 `Arc`、锁、通道、异步任务或内部可变性；共享并发访问必须由外层提供同步，不能仅凭 `Send` 推断为 `Sync` 或无锁线程安全。

所有消息与行数据由 `SimpleTemplate` 自有的 `String`/`Vec` 保存，生命周期与控制器中的 `Box<dyn Template>` 相同。`Collect` 会克隆一次消息，以同时存入消息分类向量与行记录；`Output` 又为渲染体克隆各行消息，故超长消息和大量检查项会产生线性内存/复制开销。没有文件、网络、数据库句柄或显式清理动作，模板随控制器析构自动释放。

## 与 Go 版本的对应关系

Rust 的 `Template`、`SimpleTemplate`、`NewSimpleTemplate` 及五个方法逐项对应 `lightning/pkg/importer/check_template.go`。计数顺序、只由 critical 决定成功、critical 失败消息单独收集、`FailedMsg` 的 `";\n"` 拼接、未知类型计数为 0，以及行内容 `序号/消息/类型/是否通过` 均保持一致。

实现差异集中在渲染层。Go 使用 `go-pretty/v6/table.Writer`，构造时配置四列最大宽度，在 `Output` 中设置允许行长 170 和 row painter；Rust 用 `rows` 与三个私有辅助函数复刻所需子集。Go 每次 `Collect` 直接向 writer 追加行和分隔符，Rust 延迟到 `Output` 时从 `rows` 重建文本。Rust 表头字面量为大写，以匹配 Go 默认样式渲染后的大写表头；颜色规则保持 warning 黄、critical 红。

独立 Rust 测试 `check_template_test.rs` 验证 warning/critical 失败计数、critical 消息拼接、总体失败判定、Go 风格大写表头和所有输出行可见宽度不超过 170。`table_import_test.rs` 进一步从控制器检查流程观察 warning 计数和输出消息。Go 的更广泛行为证据位于 `check_info_test.go`、`table_import_test.go` 和 `import_test.go`；它们不是本文件的 Rust 独立测试，扩展时应按触及的调用链选择对照。

## 扩展指南

- 新增严重级别时，优先在 `lightning/pkg/precheck` 明确其阻断语义，再同步修改 `Collect`、`Success`、`FailedCount`、着色规则和 Go 对照；不能只增加一种颜色，否则统计与错误传播会不一致。
- 新增或调整列时，应同时修改 `headers`、`TemplateRow`、`body` 构造、最大宽度数组、对齐规则和 170 列限制算法，并在独立的 `check_template_test.rs` 增加长文本、多行、Unicode 和颜色转义后的可见宽度覆盖。不要把 Rust 测试内嵌进生产源文件。
- 若要减少大消息复制，可重新设计消息所有权或直接渲染 `rows` 引用，但必须保持公开观察字段、消息分组与 Go 行顺序兼容，并评估公开字段变更对现有测试的影响。
- 若要让 Rust 主流程展示表格，应在控制器的输出/日志边界接入 `Output`，而不是让 `Collect` 产生 I/O；同时验证错误路径不会重复渲染或丢失 ANSI/换行格式。
- 若引入共享并发收集，需要在控制器层封装同步或修改 trait 约束和方法签名，并评估顺序稳定性；现有 `idx` 和 `rows` 默认串行收集顺序是输出协议的一部分。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file lightning/pkg/importer/check_template.rs` 读取了完整 270 行实现；`query/node` 确认 `SimpleTemplate`、`NewSimpleTemplate` 及其直接实例化关系；`explore`/调用轨迹确认控制器与测试调用者。
- 源码：`lightning/pkg/importer/check_template.rs`（trait、状态、收集与渲染算法）、`check_info.rs:66-90`（检查结果收集）、`import.rs:223-265,319-409,881-900`（控制器持有、构造和阻断判断）、`lib.rs`（模块装配与测试分离）。
- crate 与依赖：`lightning/pkg/importer/Cargo.toml`（library crate、Go 包映射、对 `astersql-lightning-pkg-precheck` 的本地路径依赖）；`lightning/pkg/precheck/precheck.rs:29-35`（`CheckType`、`Critical`、`Warn` 的实际定义）。
- Go 对照：`lightning/pkg/importer/check_template.go`；主流程对照调用点为 `lightning/pkg/importer/import.go:1998-2007`，结果收集点为 `lightning/pkg/importer/check_info.go:56`。
- 测试：`lightning/pkg/importer/check_template_test.rs`；补充调用链证据来自 `check_info_test.rs`、`table_import_test.rs`、`import_test.rs` 以及对应 Go 测试。任务为纯文档分析，按计划未运行 Cargo。
