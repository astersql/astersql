# `pkg/dxf/framework/testutil/task_util.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-testutil` crate（见 `pkg/dxf/framework/testutil/Cargo.toml`），是 DXF 测试代码向任务表写入子任务时使用的轻量适配层。模块由 `pkg/dxf/framework/testutil/lib.rs` 以 `pub mod task_util` 声明，并通过 `pub use task_util::*` 再导出，因此其他 Rust 测试 crate 可以直接从 `astersql_dxf_framework_testutil` 取得这里的公开符号。

它不实现生产调度器或真实 SQL 存储，而是把测试所需字段组装成 `NewSubtask`，再交给 `TaskTable::insert_subtask`。同一文件还提供任务提交测试使用的 keyspace 选择函数 `getTaskKS`。当前文件没有条件编译项；测试本身位于独立文件，而非内嵌在源文件中。

## 核心职责

1. `CreateSubTask` 提供最常用的“新建待执行子任务”入口：固定状态为 `SubtaskState::Pending`，然后转交 `InsertSubtask`。
2. `InsertSubtask` 允许测试显式指定任意 `SubtaskState`，构造无 summary、无开始时间的 `NewSubtask`，并调用存储抽象。
3. `CreateSubTaskWithSummary` 与 `InsertSubtaskWithSummary` 组成带摘要的对应路径；后者把已序列化的摘要保存为 `Some(summary_json)`，并把 `has_start_time` 设为 `true`。
4. `getTaskKS` 把测试显式传入的内核模式转换为任务 keyspace：next-gen 返回 `"SYSTEM"`，classic 返回空串。

这些函数的定位是测试数据构造器，不负责校验字段组合、序列化业务元数据、分配 ID、开启事务或决定真实表结构；这些职责由调用方和 `TaskTable` 实现承担。

## 主要符号

- `pub struct NewSubtask`：一次插入请求的完整值对象，派生 `Clone`、`Debug`、`Eq`、`PartialEq`，便于测试桩保存并逐字段断言。字段包括父任务 `task_id`、阶段 `step`、执行节点 `exec_id`、业务载荷 `meta`、生命周期 `state`、`task_type`、并发提示 `concurrency`、可选 `summary_json` 以及 `has_start_time`。
- `pub fn CreateSubTask(&dyn TaskTable, i64, Step, &str, Vec<u8>, &str, usize) -> Result<i64, DxfError>`：默认 Pending 的便捷入口。保留 Go 风格命名，因此用 `#[allow(non_snake_case)]`。
- `pub fn InsertSubtask(...) -> Result<i64, DxfError>`：无摘要的通用插入入口；因参数表达完整表字段而使用 `#[allow(clippy::too_many_arguments)]`。
- `pub fn CreateSubTaskWithSummary(...) -> Result<i64, DxfError>`：带摘要便捷入口，但不改变调用方给定的状态。
- `pub fn InsertSubtaskWithSummary(...) -> Result<i64, DxfError>`：带摘要的底层组装入口，设置 `summary_json = Some(...)` 和 `has_start_time = true`。
- `pub fn getTaskKS(bool) -> &'static str`：纯函数，根据显式内核模式返回静态 keyspace 字符串。

文件内没有模块级常量、trait 或 `impl`；`Step`、`SubtaskState`、`DxfError` 来自 `context.rs`，存储边界 `TaskTable` 来自 `table_util.rs`。

## 执行流程

普通创建路径如下：

1. 调用方把 `TaskTable`、任务/步骤/节点/元数据/类型/并发度交给 `CreateSubTask`。
2. `CreateSubTask` 补上唯一默认值 `SubtaskState::Pending`，其余参数原样转发给 `InsertSubtask`。
3. `InsertSubtask` 将借用的 `exec_id`、`task_type` 复制为拥有所有权的 `String`，移动 `meta`，并固定 `summary_json = None`、`has_start_time = false`。
4. 完整 `NewSubtask` 被传给 `TaskTable::insert_subtask`；返回的 ID 或 `DxfError` 不经改写直接向上传播。

摘要路径同样由 `CreateSubTaskWithSummary` 转发给 `InsertSubtaskWithSummary`。差异只有两个字段：调用方提供的 `summary_json` 被包装为 `Some`，`has_start_time` 固定为 `true`；状态仍由调用方决定。RustCodeGraph 的成功查询确认了 `CreateSubTask -> InsertSubtask` 与 `CreateSubTaskWithSummary -> InsertSubtaskWithSummary` 两条内部边。

`getTaskKS` 没有访问全局状态：`true` 立即返回 `"SYSTEM"`，`false` 返回 `""`。`disttest_util.rs::SubmitAndWaitTask` 将该结果传给 `DistributedTaskRuntime::submit_task`，随后才等待任务完成或暂停。

## 数据与状态

`NewSubtask` 是短生命周期、按值传递的插入描述，不是运行中子任务实体。`meta` 和 `summary_json` 都是字节向量；本模块不解释其格式。尤其是 summary 路径接受已经序列化的 `Vec<u8>`，不会在本地检查 JSON 是否有效。`task_type` 也只是字符串，本模块不做枚举映射。

两个布尔/可选字段形成明确不变量：

- 无摘要路径恒为 `summary_json == None && !has_start_time`。
- 摘要路径恒为 `summary_json == Some(调用值) && has_start_time`，即使摘要字节为空也仍是 `Some(Vec::new())`。

`CreateSubTask` 额外保证状态为 `Pending`；其他三个插入/摘要入口不会推导或修改调用方提供的业务字段。ID 不在 `NewSubtask` 中预分配，而由 `TaskTable::insert_subtask` 返回。

## 依赖与调用关系

直接下游只有两个仓库内边界：`context::{DxfError, Step, SubtaskState}` 提供错误和领域值，`table_util::TaskTable` 提供插入接口。目标源码不直接使用 `Cargo.toml` 中列出的外部/兄弟 crate；那些依赖服务于整个 testutil crate 的其他模块。`TaskTable: Send + Sync`，其 `insert_subtask(NewSubtask)` 契约返回新 ID 或 `DxfError`。

模块入口 `lib.rs` 公开再导出本文件的 API。RustCodeGraph 与源码检索确认的直接上游包括：

- `migration_aster_unit_test.rs::table_and_task_helpers_preserve_insert_fields_and_cleanup` 调用 `CreateSubTask`、`InsertSubtaskWithSummary` 和 `getTaskKS`，逐字段检查组装结果。
- `disttest_util.rs::SubmitAndWaitTask` 调用 `getTaskKS`，把 keyspace 送入任务提交接口。
- `integrationtests/framework_test.rs::framework_keyspace_and_task_terminal_states_match_kernel_modes` 从 testutil crate 再导出中调用 `getTaskKS`。

仓库中另有部分迁移中的 Rust 测试以 Go 兼容签名调用同名辅助函数；本文只把已由当前目标文件签名和上述可执行回归测试验证的调用边视为当前契约，不把签名不匹配的迁移文本当成已接线事实。

## 错误处理与边界

四个插入函数的错误边界都是 `Result<i64, DxfError>`。本文件不捕获、不包装也不记录 `TaskTable::insert_subtask` 的错误，因此错误语义和是否发生持久化完全取决于具体 `TaskTable` 实现。字符串复制和 `Vec<u8>` 移动本身没有业务错误分支。

本模块不验证 `task_id` 是否存在、`step` 是否允许、`exec_id` 是否为空、`concurrency` 是否非零、状态与 summary 是否一致，也不保证同任务同阶段元数据唯一。调用测试辅助时必须由夹具建立这些前置条件。`getTaskKS` 只有两个穷尽分支，不返回错误；`"SYSTEM"` 是当前 Rust 对 Go `keyspace.System` 的直接字符串对应。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、会话或事务。`NewSubtask` 的 `Vec`/`String` 所有权在调用 `insert_subtask` 时整体转移；因此调用返回后，本模块不保留对输入或插入请求的引用。`exec_id` 和 `task_type` 在边界处复制，避免让 `TaskTable` 依赖调用方借用的生命周期。

虽然 `TaskTable` 要求实现者满足 `Send + Sync`，本文件没有为一次插入提供额外同步或原子性保证。独立测试中的 `Table` 用 `Mutex<Vec<NewSubtask>>` 记录请求只是测试桩的并发策略，不属于本模块的运行时行为。真实资源清理、数据库 session 与事务生命周期也不在此层管理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/dxf/framework/testutil/task_util.go`。两版都保留 `CreateSubTask -> InsertSubtask`、`CreateSubTaskWithSummary -> InsertSubtaskWithSummary` 的两层 API，并保持普通创建默认 Pending、摘要插入同时写 summary 与开始时间、next-gen 选择系统 keyspace 的核心意图。

实现边界存在刻意差异：Go 版直接通过 `storage.TaskManager.WithNewSession` 执行 `mysql.tidb_background_subtask` 的 SQL，并查询 `@@last_insert_id`；Rust 版只组装 `NewSubtask` 后委托 `TaskTable`。Go 版在辅助函数内用 `json.Marshal` 序列化 `*execute.SubtaskSummary`，Rust 版要求调用方先提供 `summary_json: Vec<u8>`。Go 用 `require.NoError` 把错误转成测试失败，Rust 用 `Result` 交给调用方决定断言方式。Go 的 `getTaskKS` 读取 `kerneltype.IsNextGen()` 全局配置，Rust 显式接收 `bool`，从而使分支可直接测试。

因此 Rust 版已对齐“字段语义和测试用途”，但不是 Go SQL/session 实现的逐语句复刻；是否写入数据库取决于注入的 `TaskTable` 实现。`Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/dxf/framework/testutil"` 进一步确认了该 Go 包对应关系。

## 扩展指南

- 新增插入字段时，应先扩展 `NewSubtask`，再同步修改两个底层组装函数 `InsertSubtask`、`InsertSubtaskWithSummary`；若字段有普通/摘要路径不同的默认值，应在两个便捷入口的契约中明确说明。
- 存储行为应扩展 `TaskTable::insert_subtask` 的实现或其请求类型，而不要把 SQL/session 逻辑塞回本测试辅助文件，以维持当前可注入边界。
- 修改 Pending、summary 或开始时间语义时，必须同步独立测试 `pkg/dxf/framework/testutil/migration_aster_unit_test.rs::table_and_task_helpers_preserve_insert_fields_and_cleanup`；错误透传可在同目录独立测试文件中用返回 `Err(DxfError)` 的 `TaskTable` 桩补充，禁止把单元测试内嵌回本源文件。
- 修改 keyspace 策略时，应同步 `disttest_util.rs::SubmitAndWaitTask` 的调用契约以及 `pkg/dxf/framework/integrationtests/framework_test.rs::framework_keyspace_and_task_terminal_states_match_kernel_modes`。
- 兼容风险主要是 Go/Rust 字段默认值或摘要序列化责任漂移；正确性风险是新增字段只接入一条插入路径；性能风险目前限于 `exec_id`、`task_type` 和字节载荷的分配/移动，若引入大摘要复制需单独评估。

## 验证依据

- 目标源码：`pkg/dxf/framework/testutil/task_util.rs`，核对 1 个结构体、5 个公开函数、所有字段赋值、属性和分支；该目录不存在 `doc.go`。
- crate 边界：`pkg/dxf/framework/testutil/Cargo.toml` 与 `pkg/dxf/framework/testutil/lib.rs`，核对 crate 名、Go 包移植元数据、模块声明及公开再导出；Cargo 未声明 feature。
- 存储与领域边界：`pkg/dxf/framework/testutil/table_util.rs::TaskTable::insert_subtask`、`pkg/dxf/framework/testutil/context.rs::{DxfError, Step, SubtaskState}`。
- Go 对照：`pkg/dxf/framework/testutil/task_util.go`，核对默认状态、SQL 字段意图、summary 序列化、开始时间和 keyspace 行为。
- 独立 Rust 测试：`pkg/dxf/framework/testutil/migration_aster_unit_test.rs::table_and_task_helpers_preserve_insert_fields_and_cleanup`，验证两次插入返回 ID、所有普通/摘要字段及两个 keyspace 分支；`pkg/dxf/framework/integrationtests/framework_test.rs::framework_keyspace_and_task_terminal_states_match_kernel_modes` 再次验证 keyspace。
- 直接调用者：`pkg/dxf/framework/testutil/disttest_util.rs::SubmitAndWaitTask`，验证 keyspace 结果进入任务提交参数。
- RustCodeGraph：`status` 显示目标仓库已索引（目标目录包含 `task_util.rs` 的 7 个节点）；`explore`/`query` 确认目标符号、两条内部调用链及上述 Rust 调用者。对精确符号执行 `callers/callees` 时命令在 30 秒内未返回输出，因此调用关系又以源码和独立测试检索交叉确认，未据超时结果作推断。
- 按任务约束未运行 Cargo；最终仅执行任务指定的 11 章节结构验证，并人工检查未把测试建议写入源文件、未宣称不存在的真实 SQL 实现。
