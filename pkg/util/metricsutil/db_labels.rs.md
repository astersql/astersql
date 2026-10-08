# `pkg/util/metricsutil/db_labels.rs`

## 文件定位

本文件属于 `astersql-util-metricsutil` crate。crate 入口 `pkg/util/metricsutil/lib.rs` 以私有模块 `db_labels` 装载本文件，再公开重导出 `GetDBNames`；`pkg/session/lib.rs` 还会继续重导出该函数，供服务器会话路径使用。它位于 SQL 会话状态与 Prometheus 指标记录之间：读取 `SessionVars` 中本条语句涉及的数据库，生成指标的 database 标签值，但不创建、注册或写入指标。

`pkg/util/metricsutil/Cargo.toml` 将 Go 对照包声明为 `pkg/util/metricsutil`，并为本文件直接使用的配置和会话状态分别声明 `astersql-config` 与 `astersql-sessionctx-variable` 依赖；`SessionVars` 内部的 `StatementContext` 类型来自同一清单中的 `astersql-sessionctx-stmtctx`。

## 核心职责

唯一职责是由 `GetDBNames(vars: Option<&SessionVars>) -> Vec<String>` 选择 SQL 指标应使用的数据库标签：

- 没有会话变量，或全局 `status.record_db_label` 关闭时，返回单元素 `vec![""]`，以空标签维持调用端统一的“一次或多次记录”循环。
- 开关打开时，优先采用 `vars.StmtCtx.LogicalPlanTables()` 中所有 `TableEntry.DB`，并按数据库名去重。
- 逻辑计划没有记录任何表时，退回会话当前数据库 `vars.CurrentDB()`，且只对这个回退值转换为小写。

函数不解析 SQL，也不决定指标类型；表引用由规划/语句上下文预先写入，调用方负责把返回的每个标签传给具体指标记录函数。

## 主要符号

- `GetDBNames(vars: Option<&SessionVars>) -> Vec<String>`：公开 API，也是文件内唯一函数。`Option<&SessionVars>` 对应 Go 的可空 `*variable.SessionVars`；返回拥有所有权的字符串列表，使调用方不需要持有会话内部锁或借用。
- `SessionVars`：从 `astersql_sessionctx_variable::session` 导入。此处读取其非可空字段 `StmtCtx` 和方法 `CurrentDB()`。
- `BTreeSet<String>`：文件内唯一集合结构。收集 `TableEntry.DB` 时同时去重并按字符串自然顺序排序，最后通过 `into_iter().collect()` 转成 `Vec<String>`。

文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。`#[allow(non_snake_case)]` 只为保留 Go 风格公开名 `GetDBNames`，避免破坏移植 API。

## 执行流程

1. `GetDBNames` 首先匹配 `vars`。`None` 立即返回一个空字符串标签，不读取全局配置。
2. 对有效会话读取 `astersql_config::get_global_config().status.record_db_label`；开关关闭时同样立即返回空字符串标签。
3. 调用 `vars.StmtCtx.LogicalPlanTables()`，取得逻辑计划表列表的克隆；将每个 `TableEntry` 映射为其 `DB` 字段，并收集进 `BTreeSet`。
4. 如果集合为空，则读取 `vars.CurrentDB()`，执行 Unicode 小写转换 `to_lowercase()` 后插入集合。当前库也为空时，结果仍是一个空字符串标签。
5. 消耗集合并返回有序向量。若一条语句引用多个数据库，调用方会为每个唯一数据库分别记录指标。

生产路径中，`pkg/server/runtime.rs` 的 `SessionRequest::RecordCommandDuration` 分支通过 `astersql_session::GetDBNames`（会话 crate 的重导出）遍历结果，为每个数据库调用 `astersql_metrics::server::RecordCommandDuration`。`pkg/session/runtime/scan_adapter_runtime.rs` 的 `ObserveStatementDuration` 直接通过 `astersql_util_metricsutil::GetDBNames` 遍历结果，分别调用 `RecordQueryDuration` 与 `RecordQueryScanMetrics`。

## 数据与状态

函数自身无持久状态。输入数据来自三个快照来源：进程级全局配置、语句级逻辑计划表列表、会话当前数据库。

- `get_global_config()` 克隆全局 `Arc<Config>` 快照，因此一次开关判断使用同一配置对象。
- `LogicalPlanTables()` 在 `StatementContext` 的 `logicalPlanBuild` 互斥锁下克隆 `Vec<TableEntry>`，锁在该方法返回前已经释放；后续去重不持有会话锁。
- `CurrentDB()` 在读锁下克隆当前数据库字符串，同样不会把锁生命周期延伸到返回值中。
- `BTreeSet` 决定输出无重复且稳定升序。表名字段 `TableEntry.Table` 不参与标签计算；表来源的 `DB` 字符串不会在本函数中改写大小写，只有当前库回退值会转小写。

因此，函数看到的是各读取点的快照，而不是跨配置、语句上下文和当前库的一次原子快照。正常调用发生在一条语句的指标收尾阶段，依赖上游在此之前完成语句上下文填充。

## 依赖与调用关系

直接下游依赖为：

- `astersql_config::get_global_config`：读取 `status.record_db_label` 门控；定义位于 `pkg/config/config.rs`。
- `SessionVars::StmtCtx.LogicalPlanTables`：定义位于 `pkg/sessionctx/stmtctx/stmtctx.rs`，返回逻辑计划表条目的克隆。
- `SessionVars::CurrentDB`：定义位于 `pkg/sessionctx/variable/session.rs`，返回当前数据库字符串的克隆。
- 标准库 `BTreeSet`：完成去重和确定性排序。

RustCodeGraph 对本文件给出 4 个符号，并报告文件被 `pkg/server/runtime.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/util/metricsutil/go_merge_30_test.rs` 使用。精确 `callers`/`callees` 查询未输出函数级边，因此又以这些图结果和 `rg` 交叉核对：生产调用点是 `pkg/server/runtime.rs:955` 与 `pkg/session/runtime/scan_adapter_runtime.rs:1938`；`pkg/session/lib.rs` 和本 crate 的 `lib.rs` 提供重导出；Rust 回归覆盖还包括 `pkg/session/main_test.rs`。

调用链可概括为：语句规划填充 `StatementContext` 的逻辑计划表 → 命令或语句完成路径调用 `GetDBNames` → 每个返回数据库名成为 server 指标的 database label。此文件不反向依赖指标 crate，因而标签选择逻辑可独立测试。

## 错误处理与边界

`GetDBNames` 不返回 `Result`，没有可恢复错误分支。明确边界如下：

- `vars == None` 与关闭标签开关均安全退化为 `vec![""]`，而不是空向量；调用方因此仍会记录一次不区分数据库的指标。
- 没有逻辑计划表时使用当前库；当前库为空则返回 `vec![""]`。
- 重复数据库名被去重；字符串比较区分大小写，所以不同大小写的表来源数据库名会被视为不同标签。本文件不校验数据库名合法性。
- 表来源非空时不会混入当前数据库，即使当前库与表所属库不同。
- Rust 的 `StmtCtx` 字段非可空，不存在 Go 版本的 `nil statement context` 分支；其表列表为空时达到相同回退效果。
- `LogicalPlanTables()` 与 `CurrentDB()` 对被毒化的内部锁采用 `PoisonError::into_inner` 继续读取；相反，`get_global_config()` 的全局配置读锁若被毒化会在配置模块内 `expect` 并 panic。本函数不捕获该 panic。

## 并发与资源生命周期

本函数不生成线程、异步任务、通道、事务或外部资源，也不保留引用。配置和会话数据都被克隆到拥有所有权的值中；集合在函数栈内创建，转换成返回向量后释放。

并发安全主要由下游读取 API 提供：全局配置使用 `RwLock<Arc<Config>>`，逻辑计划表使用互斥锁，当前数据库使用读写锁。函数分别读取它们，因此不会长时间持锁，也没有锁嵌套；代价是不能保证三个状态来自同一原子时刻。调用者应继续在语句生命周期的稳定点调用，不能把本函数当成会话状态同步原语。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/util/metricsutil/db_labels.go`，核心语义一致：空会话或关闭 `RecordDBLabel` 时返回空标签；否则从语句上下文表集合收集数据库，集合为空时以小写当前库回退。

可观察差异与 Rust 数据模型适配包括：

- Go 使用 `map[string]struct{}`，遍历顺序未指定；Rust 使用 `BTreeSet`，输出稳定升序。Go 测试 `TestGetDBNamesLabels` 用 `ElementsMatch` 回避顺序约束，而 Rust 测试直接断言 `vec!["db_a", "db_b"]`。
- Go 的 `SessionVars` 和 `StmtCtx` 都可能为 `nil`；Rust 用 `Option<&SessionVars>` 表达前者，但 `SessionVars.StmtCtx` 是实体字段。因此 Go 的空语句上下文与 Rust 的空逻辑计划表在结果上都回退到当前库，类型层面的空值处理不同。
- 两版都只对当前库回退值做小写转换，不规范化逻辑计划表携带的数据库名。

Go 测试 `pkg/util/metricsutil/db_labels_test.go` 还覆盖多类 SQL 后当前库标签保持正确、空会话、关闭开关、空表集合、空当前库和跨库去重。Rust 的直接独立测试 `pkg/util/metricsutil/go_merge_30_test.rs` 覆盖门控、空会话、回退、去重和确定顺序；`pkg/session/main_test.rs` 的 `get_db_names_matches_go_fallback_deduplication_and_label_gate` 从会话 crate 重导出层重复验证同一契约。

## 扩展指南

若要改变数据库标签选择规则，应优先修改 `GetDBNames`，并同时评估两个生产消费点对“返回多个元素”的放大效应：每增加一个标签，命令耗时、查询耗时及扫描指标都可能增加一个时序，存在 Prometheus 基数与性能风险。不要把 SQL 解析或指标写入职责移入本文件。

安全扩展时应保持以下兼容契约：关闭开关仍返回恰好一个空标签；无表信息仍有当前库回退；重复库名不导致重复记录；公开名及 `lib.rs` 重导出保持兼容。若引入大小写统一、过滤空库名或排序策略变化，需要明确这是可观察行为变更，并与 Go 版本同步。

测试应放在独立文件而非源文件内：直接逻辑优先扩充 `pkg/util/metricsutil/go_merge_30_test.rs`；跨 crate 重导出与会话接线可扩充 `pkg/session/main_test.rs`；Go 语义改变时同步 `pkg/util/metricsutil/db_labels_test.go`。建议覆盖混合大小写表来源、空字符串与非空数据库混合、多库高重复率，以及配置切换前后的行为。由于测试会修改进程级全局配置，必须沿用 `restore_func()`/恢复原配置的清理模式，避免污染并行或后续测试。

## 验证依据

- 目标源码：`pkg/util/metricsutil/db_labels.rs`，确认唯一公开函数、短路分支、`BTreeSet` 去重排序与当前库回退。
- crate 边界：`pkg/util/metricsutil/Cargo.toml`、`pkg/util/metricsutil/lib.rs`、`pkg/session/lib.rs`，确认包映射、直接依赖和两层重导出。
- 状态 API：`pkg/sessionctx/stmtctx/stmtctx.rs` 的 `TableEntry`、`SetLogicalPlanTables`、`LogicalPlanTables`；`pkg/sessionctx/variable/session.rs` 的 `SessionVars`、`CurrentDB`；`pkg/config/config.rs` 的 `get_global_config`。
- 生产调用：`pkg/server/runtime.rs` 的 `SessionRequest::RecordCommandDuration`；`pkg/session/runtime/scan_adapter_runtime.rs` 的 `ObserveStatementDuration`。
- Go 对照与测试：`pkg/util/metricsutil/db_labels.go`、`pkg/util/metricsutil/db_labels_test.go`。
- Rust 独立测试：`pkg/util/metricsutil/go_merge_30_test.rs`、`pkg/session/main_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/metricsutil/db_labels.rs` 与按文件 `node` 确认目标有 4 个符号及 3 个使用文件；`query GetDBNames --kind function --json` 确认 Rust 函数签名及 Go 对照签名。精确函数 ID 的 `callers`/`callees` 无输出，故调用边由图报告的使用文件与文本引用交叉验证，不把缺失图边推断为“无调用”。
- 本任务只新增文档，不修改运行时代码，按计划不运行 Cargo；交付前使用任务指定命令验证恰有 11 个固定二级章节，并人工复核所有行为结论均可回溯到以上源码、调用点或测试。
