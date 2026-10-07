# `lightning/pkg/precheck/precheck.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-precheck` crate 的核心公共契约层，对齐 Go 包 `lightning/pkg/precheck/precheck.go`。crate 入口 `lightning/pkg/precheck/lib.rs` 通过 `pub use precheck::*` 将这里的常量、类型、函数和 trait 暴露在 crate 根路径；同一入口还重导出 `stubs.rs` 提供的最小 `context` 与 `errors` 边界。

它位于 Lightning 导入前检查的调用方与具体检查实现之间：`lightning/pkg/importer/precheck.rs` 按 `CheckItemID` 构造具体检查器，`lightning/pkg/importer/check_info.rs::Controller::doPreCheckOnItem` 执行检查并收集结果；`lightning/pkg/importinto/precheck.rs::PrecheckRunner` 则持有并顺序执行 `Box<dyn Checker>`。本文件本身不访问存储、数据库或网络，也不实现任何具体检查算法。

`lightning/pkg/precheck/Cargo.toml` 将该 crate 声明为 workspace library，`[lib]` 指向 `lib.rs`，porting 元数据指向 Go 包 `lightning/pkg/precheck`；其 `[dependencies]` 为空，说明此契约 crate 当前刻意依赖本地桩，而非真实的 Go context/errors 对应实现。

## 核心职责

- 用 `CheckType`、`Critical` 和 `Warn` 固定检查严重级别的跨语言字符串协议。特别地，`Warn` 的值是 Go 版沿用的 `"performance"`，不是直觉上的 `"warning"`。
- 用 `CheckItemID` 及 14 个 `Check*` 常量为具体检查项提供稳定标识，使 builder 分派、日志、结果收集和测试断言使用同一组键。
- 用 `DisplayName` 将已知 ID 映射为面向用户的英文展示名，并保留 Go map 未命中返回空字符串的行为。
- 用 `CheckResult` 表达“检查执行完成后的业务结果”，将检查项、严重级别、是否通过和消息组合在一个值中。
- 用 `Checker` trait 规定具体检查器的最小接口，并区分“跳过检查”“检查未通过”和“检查执行出错”三种结果。

该文件是协议和数据模型，不是策略实现。某个检查何时启用、怎样读取集群状态以及怎样生成消息，属于 `lightning/pkg/importer/precheck_impl.rs`、`lightning/pkg/importinto/precheck.rs` 等实现文件。

## 主要符号

- `pub type CheckType = &'static str`：严重级别的静态字符串别名。`Critical = "critical"` 表示阻塞性问题；`Warn = "performance"` 表示性能类告警。
- `pub type CheckItemID = &'static str`：检查项 ID 的静态字符串别名。14 个公开常量覆盖大文件、源权限、目标表空置、源 schema、checkpoint、CSV header、集群资源/region/version、本地磁盘与临时 KV、CDC/PiTR，以及 PD/TiDB 同集群检查。
- `fn checkItemIDToDisplayName() -> HashMap<CheckItemID, &'static str>`：内部映射构造器。每次调用都新建映射，没有全局可变状态或惰性初始化。
- `pub fn DisplayName(c: CheckItemID) -> &'static str`：公开查表函数；已知 ID 返回静态展示名，未知 ID 和空 ID 返回 `""`。
- `pub struct CheckResult`：公开结果结构，包含 `Item`、`Severity`、`Passed`、`Message`。派生 `Clone`、`Debug`、`PartialEq`、`Eq`，便于转交、诊断和测试比较。
- `impl Default for CheckResult`：复刻 Go 结构体零值，两个字符串 ID/类型为空、`Passed=false`、消息为空。
- `CheckResult::new(item, severity)`：初始化 ID 与严重级别，仍将结果默认设为未通过且消息为空。
- `CheckResult::critical(item, passed, message)` 与 `CheckResult::warn(...)`：分别固定 `Severity` 为 `Critical`/`Warn` 的便捷构造器。当前仓库搜索未发现生产调用，属于可用的公共辅助 API。
- `pub trait Checker`：要求可变接收者实现 `Check(context::Context) -> Result<Option<CheckResult>, errors::Error>` 和 `GetCheckItemID() -> CheckItemID`。

## 执行流程

典型 Importer 流程如下：

1. 上层方法（例如 `Controller::StoragePermission`）选择一个 `CheckItemID`。
2. `Controller::doPreCheckOnItem` 调用 builder 的 `BuildPrecheckItem`，获得 `Box<dyn Checker>`。
3. 调用方把自身上下文转换为本 crate 的 `context::Context`，再调用 `Checker::Check`。
4. `Err(errors::Error)` 表示检查过程无法完成，调用方将其转成自己的错误并停止当前路径；`Ok(None)` 表示检查被跳过，不收集结果；`Ok(Some(result))` 表示检查已执行，调用方读取 `Severity`、`Passed` 和 `Message`。
5. Importer 将非空结果交给 `checkTemplate.Collect`；Import Into 的 `PrecheckRunner::Run` 则在 `Passed=false` 时立即返回错误，在通过时按消息是否为空记录日志。

`DisplayName` 是独立的纯查表流程：调用时构造映射、按传入 ID 查询、复制静态字符串引用，未命中经 `unwrap_or("")` 返回空串。`CheckResult` 的三个构造入口只组装字段，不执行检查，也不自动调用 `DisplayName`。

## 数据与状态

所有协议标识均为 `&'static str`，因此 ID 和严重级别必须是编译期或静态生命周期字符串，不能直接承载运行时动态 ID。这与当前 Go 版允许任意 `string` 的类型能力略有差异，但仓库内注册的正式检查项均使用公开常量；测试 mock 也只传字符串字面量。

`CheckResult` 是拥有 `String` 消息的普通值对象，不保存资源句柄、上下文或检查器引用。`Default` 的 `Passed=false` 只是 Go 零值对齐，并不等于已经执行并判定失败；调用方必须结合 `Option` 和 `Result` 判断状态。

`checkItemIDToDisplayName` 返回一次性 `HashMap`，每次 `DisplayName` 调用都有小规模分配和填充成本。它没有缓存、锁或共享可变状态，行为确定；若展示名查询进入高频路径，才有理由评估静态表或 `match`，修改时必须保持未知 ID 返回空串。

`Checker::Check` 使用 `&mut self`，允许具体检查器在一次调用中更新内部缓存、消耗注入错误或记录调用次数；trait 本身不承诺可重入、线程安全或幂等。

## 依赖与调用关系

直接下游依赖只有标准库 `std::collections::HashMap`，以及 crate 根由 `stubs.rs` 重导出的 `context`、`errors`。`context::Context` 可克隆，包含取消标志与由 `Arc<HashMap<String, Arc<dyn Any + Send + Sync>>>` 保存的类型擦除值；`errors::Error` 是字符串错误桩。`precheck.rs` 不知道这些边界的真实外部实现。

主要上游消费方包括：

- `lightning/pkg/importer/precheck.rs::BuildPrecheckItem`：以全部公开 `CheckItemID` 常量做分派并返回具体 `Checker`。
- `lightning/pkg/importer/check_info.rs::Controller::doPreCheckOnItem`：调用 `GetCheckItemID` 对应的构造路径，执行 `Check` 并将 `Some` 结果送入模板。
- `lightning/pkg/importer/precheck_impl.rs`：为各类具体检查项实现 `Checker`，生产 `CheckResult`。
- `lightning/pkg/importinto/precheck.rs::PrecheckRunner` 与 `CheckpointCheckItem`：分别消费 trait object、实现 trait，并依据 `Passed`/`Message` 决定控制流和日志。

RustCodeGraph 对文件的索引显示其被 `lightning/pkg/importer/check_template.rs`、`lightning/pkg/importer/precheck.rs`、命令入口桩及相关测试等多个文件使用；图中确认 `DisplayName -> checkItemIDToDisplayName` 的直接调用边。对 trait 的动态分派无法完整表示为单一静态调用边，因此具体消费关系同时以文件源码和 `rg` 引用核验。

## 错误处理与边界

`DisplayName` 不返回错误：未知 ID、空 ID 或未来未加入映射的 ID 都静默得到空字符串。这是 Go map miss 的兼容语义；若调用方需要区分“合法但空展示名”和“未知 ID”，必须在更高层显式校验。

`Checker::Check` 的三层返回值具有不可互换的含义：

- `Err(errors::Error)`：检查过程出错，未产生可信业务结果；Import Into 会添加 `precheck <id> failed` 上下文并终止。
- `Ok(None)`：主动跳过检查，对应 Go 的 `nil *CheckResult, nil error`；调用方应继续，而不是当作失败。
- `Ok(Some(CheckResult { Passed: false, .. }))`：检查已正常完成但前置条件不满足；这是业务失败，不是执行错误。
- `Ok(Some(CheckResult { Passed: true, .. }))`：通过；非空消息仍可作为告警或说明输出。

本文件不验证 `CheckResult.Item` 是否与 `GetCheckItemID()` 相等，不限制 `Severity` 是否为两个公开常量，也不强制失败结果携带消息。这些是不变量期望而非类型系统保证，具体实现和测试需要负责维持。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务、文件或网络连接。常量与结果值没有显式资源生命周期；临时展示名 `HashMap` 在 `DisplayName` 返回前即被释放，返回值因是静态字符串引用而继续有效。

`Checker` 没有 `Send`、`Sync`、`Clone` 或关闭方法约束。当前 `PrecheckRunner` 将检查器保存在 `Vec<Box<dyn Checker>>` 中并通过 `&mut self` 顺序执行，遇首个错误或未通过结果即停止；因此本契约不能被解读为允许同一实例并发调用。资源清理由具体检查器或拥有它的上层负责，`parity_test.rs` 中的 `MockChecker::Close` 只是测试辅助方法，不是 trait 合约。

上下文按值传入。当前本地桩的克隆共享 `str_values` 的 `Arc`，但复制布尔取消快照；Importer 的转换保留值表和取消状态，Import Into 则只桥接取消状态。取消能否中断 I/O 取决于具体检查实现，本 trait 只提供传递入口。

## 与 Go 版本的对应关系

`lightning/pkg/precheck/precheck.go` 是直接对照：两边拥有相同的 `Critical`/`Warn` 字面值、14 个检查 ID、展示名表、四字段结果和两方法检查器接口。Rust 的 `DisplayName(c)` 是自由函数，对应 Go 的 `(CheckItemID).DisplayName()` 方法；两者未知 ID 都返回空串。

Rust 用 `Result<Option<CheckResult>, errors::Error>` 对应 Go 的 `(*CheckResult, error)`：`None` 对应 nil result，`Err` 对应非 nil error。Rust 的 `Default` 显式复现 Go 结构体零值；额外的 `new`、`critical`、`warn` 构造器在 Go 文件中没有同名 API，只是 Rust 便捷层，不应改变公共字段语义。

值得注意的差异是 Rust 的两个别名采用 `&'static str`，比 Go 的任意 `string` 更严格；Rust trait 使用 `&mut self`，Go interface 方法签名本身没有可变借用概念；Rust 当前上下文和错误来自本地最小桩，而 Go 直接使用 `context.Context` 和标准/项目错误链。`Cargo.toml` 的注释明确这是避免引入 kv/domain/kvproto/grpcio 的当前移植边界，不应把桩能力描述为完整生产等价物。

## 扩展指南

新增检查项时，应首先在 Go 对照确认或定义稳定 ID 与展示名，再在本文件增加 `CheckItemID` 常量和 `checkItemIDToDisplayName` 映射项；随后在 `lightning/pkg/importer/precheck.rs::BuildPrecheckItem` 接入构造分支，并在独立的具体实现文件中实现 `Checker`。不要把检查算法或测试代码放进本文件。

同步测试至少应更新 `lightning/pkg/precheck/parity_test.rs`，锁定新 ID、展示名及未知 ID 边界；若接入 Importer，更新相应独立 `lightning/pkg/importer/*_test.rs`；若接入 Import Into runner，更新 `lightning/pkg/importinto/precheck_test.rs`。还应与 Go 的 `lightning/pkg/precheck/precheck.go` 及其调用方测试保持语义一致。

调整返回协议时必须同时审计 `Controller::doPreCheckOnItem` 和 `PrecheckRunner::Run`：把 `None`、`Passed=false` 或 `Err` 混为一谈会改变跳过、业务拒绝和基础设施故障的控制流。改变 ID/严重级别字面值会破坏 builder 分派、日志与兼容断言；改变 `CheckType`/`CheckItemID` 生命周期会扩大 API 和所有实现的类型影响。

性能方面，只有证据表明 `DisplayName` 是热点时才应替换每次构造的 `HashMap`；替换方案需保持精确字符串和值缺失行为。并发方面，在给 `Checker` 增加 `Send + Sync` 或异步接口前，必须逐个审计现有实现的内部状态与资源生命周期。

## 验证依据

- RustCodeGraph：`status` 确认仓库索引包含 7,032 个 Rust 文件且目标目录已索引；`files --filter lightning/pkg/precheck` 找到 `lib.rs`、`precheck.rs`、`stubs.rs`、`parity_test.rs` 和 Go 对照；`explore "lightning/pkg/precheck/precheck.rs precheck symbols callers callees"` 确认公共符号、消费方和 `DisplayName` 调用关系；`node --file` 读取目标、Go 对照、crate 入口、桩、Importer/Import Into 消费方及 Rust 测试。
- 目标源码：`lightning/pkg/precheck/precheck.rs`，核对 2 个类型别名、2 个严重级别常量、14 个检查 ID、展示名映射、`CheckResult` 三组实现入口和 `Checker` trait。
- crate 边界：`lightning/pkg/precheck/Cargo.toml`、`lightning/pkg/precheck/lib.rs`、`lightning/pkg/precheck/stubs.rs`；确认 library 路径、Go 包元数据、空依赖表、重导出方式和桩能力边界。
- Go 对照：`lightning/pkg/precheck/precheck.go`；逐项核对常量、展示名、零值可观察行为、结果字段和 `Checker` 接口。
- 直接调用证据：`lightning/pkg/importer/check_info.rs`、`lightning/pkg/importer/precheck.rs`、`lightning/pkg/importer/precheck_impl.rs`、`lightning/pkg/importinto/precheck.rs`。
- 独立测试：`lightning/pkg/precheck/parity_test.rs` 验证常量、展示名、未知 ID、零值、跳过、错误与失败结果；`lightning/pkg/importinto/precheck_test.rs` 验证 checkpoint 分支、runner 的错误/未通过短路和取消状态传递。Go 侧相关覆盖见 `lightning/pkg/importinto/precheck_test.go` 与 `lightning/pkg/importer/precheck_impl_test.go`。
- 结构验收使用任务指定命令，要求文档存在且恰好含 11 个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
