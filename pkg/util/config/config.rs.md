# `pkg/util/config/config.rs`

源文件：[`config.rs`](config.rs)

## 文件定位

`config.rs` 属于 `astersql-util-config` crate。crate 入口 `pkg/util/config/lib.rs` 将本模块的公开项全部再导出，并同时再导出 `logutil` 与 `variable`；`pkg/util/config/Cargo.toml` 表明它只直接依赖这两个工作区 crate 和 `toml 0.8`，没有 feature 条件。该文件承担 Plan Replayer 配置恢复中的一个窄职责：把 TOML 中记录的系统变量尝试写入一份可变的 `variable::SessionVars`。

公开入口是 `LoadConfigForPlanReplayerLoad`，公开适配边界是 `PlanReplayerSessionContext`，公开错误是 `ConfigLoadError`。当前 Rust 仓库中，对该入口的可确认直接调用位于独立测试 `pkg/util/config/migration_aster_unit_test.rs`；Rust Plan Replayer 的生产流程 `pkg/executor/plan_replayer.rs::loadVariables` 当前委托给 `PlanReplayerBackend::load_variables`，没有直接调用这里的函数。因此本文件是已经实现并经过单元测试的配置加载能力，但不能据此声称它已经接入 Rust Plan Replayer 的生产后端。

## 核心职责

- `LoadConfigForPlanReplayerLoad` 先完整读取并解析 TOML，再逐项恢复变量，避免输入读取失败或 TOML 语法错误时产生部分写入。
- `ignoredSystemVariablesForPlanReplayerLoad` 屏蔽 `innodb_lock_wait_timeout`、`tidb_low_resolution_tso`、`tidb_snapshot`、`tidb_read_staleness`。后面三个变量会改变加载会话的读时间戳；Go 端到端测试证明强行恢复 `tidb_low_resolution_tso` 会妨碍随后创建 schema。
- 对未知变量、校验失败和设置失败采取“记录变量名、写 Warn、继续处理”的尽力恢复策略；只有读取或 TOML 解码失败会使整个调用返回 `Err`。
- 变量值必须先经过 `SysVar::Validate` 的会话作用域校验/规范化，再交给 `SessionVars::SetSystemVar`，不能绕过变量类型、范围及设置钩子。

## 主要符号

- `ignoredSystemVariablesForPlanReplayerLoad: LazyLock<HashSet<&'static str>>`：进程内惰性初始化、只读共享的忽略集合。集合元素来自 `variable::vardef::InnodbLockWaitTimeout` 和三个字符串常量。
- `pub trait PlanReplayerSessionContext`：只要求 `fn GetSessionVars(&mut self) -> &mut variable::SessionVars` 的窄接口，用于避免依赖完整会话上下文。文件为 `variable::SessionVars` 自身提供实现，因此调用方可直接传可变变量容器。
- `pub enum ConfigLoadError { Io(std::io::Error), Toml(toml::de::Error) }`：区分输入读取与 TOML 解码失败；其 `Display` 和 `Error::source` 都保留底层错误。
- `fn warn(message: String, error: Option<String>)`：私有日志适配器；调用 `BgLogger().log(LogLevel::Warn, ...)`，只有错误存在时才附加名为 `error` 的字符串字段。
- `pub fn LoadConfigForPlanReplayerLoad(ctx, v) -> Result<Vec<String>, ConfigLoadError>`：主入口。成功返回未能加载的变量名列表；被明确忽略的变量不进入该列表。

## 执行流程

1. `LoadConfigForPlanReplayerLoad` 用 `Read::read_to_string` 把输入完整读入 `String`。失败立即映射为 `ConfigLoadError::Io`，尚未取得 `SessionVars`，也未写入任何变量。
2. `toml::from_str` 将全文解码为 `HashMap<String, String>`。失败立即返回 `ConfigLoadError::Toml`；因为逐变量循环尚未开始，不会留下部分更新。顶层值必须能反序列化为字符串。
3. 通过 `ctx.GetSessionVars()` 取得唯一的可变会话变量引用，然后遍历映射。`HashMap` 不承诺稳定顺序，所以设置顺序和返回列表顺序不是接口保证。
4. 若名称在 `ignoredSystemVariablesForPlanReplayerLoad` 中，只输出 `ignore set variable name:value` Warn 并继续；此分支既不写入，也不加入 `unLoadVars`。
5. 调用 `variable::GetSysVar(&name)` 查注册表。未知名称加入 `unLoadVars`，输出 `skip set variable name:value`，然后继续。
6. 已知变量通过 `sysVar.Validate(vars, &value, ScopeSession)` 校验和规范化。失败时加入 `unLoadVars`，日志为 `skip variable name:value` 并带底层错误字段；成功结果 `sVal` 可能不同于输入，例如测试中的无符号上界把 `99` 收敛为 `10`。
7. 用 `vars.SetSystemVar(&name, &sVal)` 应用规范化值。设置钩子拒绝时加入 `unLoadVars`，输出带错误字段的 `skip set variable` Warn；成功则无返回项和日志。
8. 遍历完毕返回 `Ok(unLoadVars)`。某个变量失败不会回滚此前成功设置的其他变量。

## 数据与状态

输入是实现 `Read` 的任意读取器及实现 `PlanReplayerSessionContext` 的可变上下文；解析中间态是 `HashMap<String, String>`，输出状态是 `Vec<String>`。该向量只收集未知、校验失败或设置失败的名称，不携带失败类别、原值或错误详情；具体原因只进入后台日志。

持久状态位于调用方提供的 `SessionVars`。函数自身不保存每次加载结果；唯一的模块级状态是只读的 `LazyLock<HashSet<_>>`。对合法变量的修改是逐项生效的，不具备事务或回滚语义。输入被完整缓存在内存中，因此空间开销与 TOML 文本和解析后映射大小成正比。

## 依赖与调用关系

上游公开路径为 `pkg/util/config/lib.rs` 的 `pub use config::*`。RustCodeGraph 对 `LoadConfigForPlanReplayerLoad`、`PlanReplayerSessionContext`、`ConfigLoadError` 的精确查询确认它们定义于本文件，但 callers/callees 查询未返回可用调用边；仓库文本引用进一步确认主函数目前只被 `pkg/util/config/migration_aster_unit_test.rs` 直接调用。`pkg/executor/Cargo.toml` 虽声明 `astersql-util-config` 依赖，`pkg/executor/plan_replayer.rs::loadVariables` 和 `updateLoadInfo` 当前走 `PlanReplayerBackend::load_variables`，两者之间的生产接线不可从现有代码确认。

下游依赖如下：`std::io::Read` 负责输入，`toml::from_str` 负责反序列化；`variable::GetSysVar` 查找定义，`SysVar::Validate` 执行会话作用域校验和规范化，`SessionVars::SetSystemVar` 执行实际设置及钩子；`logutil::log::BgLogger` 记录可恢复失败。Go 生产调用链则是 `pkg/executor/plan_replayer.go::loadVariables` 从归档打开 `variables.toml`，调用 `pkg/util/config/config.go::LoadConfigForPlanReplayerLoad`，并把非空的未加载列表追加为语句警告。

## 错误处理与边界

硬错误只有两类：读取器错误和 TOML 解码错误，分别由 `ConfigLoadError::Io`、`ConfigLoadError::Toml` 原样保留为 `source`。由于二者都发生在逐项修改之前，这两条路径满足“无部分写入”；`migration_aster_unit_test.rs::malformed_toml_and_reader_errors_abort_without_partial_updates` 同时验证了错误变体、错误显示文本及变量未设置。

变量级问题都是软失败。未知变量、`Validate` 失败和 `SetSystemVar` 失败进入返回列表并继续；忽略项只记日志。成功解析后不存在整体原子性：若后一个变量软失败，前一个成功变量仍保留。另一个边界是返回顺序不稳定，因为来源是 `HashMap`；调用方若需要确定输出，必须自行排序，单元测试正是如此。函数读取 UTF-8 `String`，非 UTF-8 字节会以 `std::io::Error` 形式从 `read_to_string` 返回。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务。调用期间独占借用上下文和 `SessionVars`，Rust 借用规则阻止同一变量容器被并发可变访问。`ignoredSystemVariablesForPlanReplayerLoad` 的 `LazyLock` 负责线程安全的一次性集合初始化，此后只读访问。

读取器按值传入并在函数返回时释放；该函数不负责显式关闭外部资源。与之对应，Go 调用方在 `pkg/executor/plan_replayer.go::loadVariables` 中打开 zip 条目并 `defer v.Close()`，资源所有权属于上游。日志器由 `BgLogger()` 获取，生命周期和刷新策略不在本文件管理。逐变量修改没有回滚保护，扩展为并行加载会破坏当前对单个可变 `SessionVars` 的顺序独占假设，不应直接并行化。

## 与 Go 版本的对应关系

Rust 主循环逐分支对齐 `pkg/util/config/config.go::LoadConfigForPlanReplayerLoad`：同一忽略集合、同样的系统变量查找、`ScopeSession` 校验、`SetSystemVar` 调用、软失败列表和 Warn 文案。`PlanReplayerSessionContext` 对应 Go 的 `sessionctx.Context` 中本函数实际使用的 `GetSessionVars` 子集；Rust 的泛型 `Read` 对应 Go 的 `io.ReadCloser`，但 Rust 函数不要求或执行关闭。

主要差异是错误建模和接线状态。Go 用单一 `error` 并由调用方 `errors.AddStack`，Rust 显式区分 `Io`/`Toml`；Go 的 TOML decoder 直接读取流，Rust 先读成 UTF-8 字符串再解析。Go 生产路径已在 `pkg/executor/plan_replayer.go::loadVariables` 调用此逻辑，且 `pkg/executor/test/planreplayer/plan_replayer_test.go::TestPlanReplayerLoadIgnoresLowResolutionTSO`（由相邻注释和断言可定位）验证变量被归档但加载时保持关闭，以确保后续 DDL 可执行。Rust 的同名 Plan Replayer 流程当前由 backend 抽象承担变量恢复，未验证它调用本 crate；因此两端不能视为已具有相同的生产接线。

## 扩展指南

- 增减忽略变量时修改 `ignoredSystemVariablesForPlanReplayerLoad`，并同步 Go 文件的同名集合。先判断变量是否会改变加载时 schema/时间戳可见性，再在 `migration_aster_unit_test.rs` 增加“不写入且不进入 unloaded”的断言；影响完整加载顺序时还应同步 Go/Rust Plan Replayer 独立测试。
- 增加新的变量失败策略时优先修改 `LoadConfigForPlanReplayerLoad` 的四分支流程，并保持“解析硬错误发生在任何写入之前”的不变量。若改变返回数据结构，需要同步 Go 调用方把失败变量转成语句警告的兼容语义。
- 若要接入 Rust 生产 Plan Replayer，应在真实 `PlanReplayerBackend::load_variables` 实现处建立桥接，而不是在本文件假设归档或 executor 类型。接线测试应放在 `pkg/executor/plan_replayer_test.rs` 或 `pkg/executor/test/planreplayer/plan_replayer_test.rs`；本 crate 的分支级测试继续放在独立文件 `pkg/util/config/migration_aster_unit_test.rs`，不要嵌入生产源文件。
- 不要把遍历顺序作为 API。若用户可见警告需要稳定顺序，应在边界排序，同时评估是否会偏离 Go map 的无序语义。
- 大输入当前会被完整读取和解析两份表示；若优化为流式解析，必须保留读取/语法失败不产生部分写入的行为，否则需要明确引入事务式暂存。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件；`files --filter pkg/util/config` 列出 `config.rs`、`config.go`、`lib.rs` 和 `migration_aster_unit_test.rs`。
- RustCodeGraph `node --file pkg/util/config/config.rs`：核对全部 154 行及主要符号；精确 `query` 得到 `LoadConfigForPlanReplayerLoad`、`PlanReplayerSessionContext`、`ConfigLoadError`，对应的 `callers`/`callees` 未返回可用边，因此调用关系又以精确文本引用核验。
- crate 与入口：`pkg/util/config/Cargo.toml`、`pkg/util/config/lib.rs`；生产/抽象入口：`pkg/executor/plan_replayer.go::loadVariables`、`pkg/executor/plan_replayer.rs::loadVariables`、`updateLoadInfo`、`pkg/executor/Cargo.toml`。
- Go 对照：`pkg/util/config/config.go`。Rust 独立测试：`pkg/util/config/migration_aster_unit_test.rs` 的三个测试覆盖忽略、未知、校验、设置钩子、范围规范化、日志以及读取/TOML 硬错误。Go 端到端证据：`pkg/executor/test/planreplayer/plan_replayer_test.go` 第 544–576 行验证低精度 TSO 被归档但加载时忽略。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以源码、图查询、对照测试阅读和固定章节结构检查作为验证。
