# `pkg/executor/import_into.rs`

## 文件定位

`import_into.rs` 位于 `astersql-executor` crate，模块由 [`pkg/executor/lib.rs`](lib.rs) 的 `pub mod import_into` 公开。它把 `IMPORT INTO` 的顶层控制流移植成一个以 `ImportIntoRuntime` 为依赖注入边界的泛型 Rust 执行器：本文件决定校验、准备、任务提交、等待、结果回填和取消的先后顺序，具体会话、存储、导入器和分布式任务操作则交给运行时实现。

当前接线状态需要与“已存在 Rust 源码”区分：仓库搜索只找到 [`tests/realtikvtest/importintotest/job_runtime.rs`](../../tests/realtikvtest/importintotest/job_runtime.rs) 实现 `ImportIntoRuntime`，并在那里构造 `ImportIntoExec` 与 `ImportIntoActionExec`；未在 `pkg/executor` 的 Rust 生产构建器中找到对应构造调用。因此它是公开且可由测试集成的 Rust 编排层，但不能仅据此断言 Rust 服务器主链已经使用它。生产语义的直接对照仍是 [`pkg/executor/import_into.go`](import_into.go)。

## 核心职责

- `checkExprWithProvidedProps` 与 `ValidateImportIntoColAssignmentsWithEncodeCtx` 保证列赋值里的标量函数只依赖编码上下文实际提供的可选属性；校验会递归进入标量函数子节点，并保留赋值下标和函数名用于报错。
- `ImportIntoExec::Next` 实现文件导入的总编排：建立导入计划、校验赋值、建立控制器、选择同步或异步准备、用隔离会话做前置检查、初始化 TiKV 配置、提交任务、按 detached 状态决定是否等待，最后回填作业信息。
- 同一个入口在存在 `select_executor` 时改走 `importFromSelect`，由运行时完成 SELECT 生产者与导入消费者的并发管线，不再提交文件导入任务。
- `submitTask` 根据路径是否本地及分布式任务开关，在“本地切块后单机提交”“远端分布式提交”“远端单机提交”三条路径间选择。
- `waitTask` 处理连接上下文取消这一特殊失败：不再复用已取消的上下文，而是委托运行时在后台取消作业并等待结束。
- `ImportIntoActionExec` 为已有作业提供取消入口，在执行动作前检查用户权限与作业状态。

## 主要符号

- `ImportExpression`：表达式树的最小视图，暴露标量函数名、所需可选属性位掩码和子节点。`scalar_function_name() == None` 表示非标量节点，本文件不会继续遍历其子节点，这与 Go 版只对 `*expression.ScalarFunction` 递归一致。
- `UnsupportedImportFunction { function_name, assignment_index }`：属性不足时的结构化错误；其 `Display` 文本与 Go 测试约定一致。
- `checkExprWithProvidedProps(index, expression, provided_properties)`：单棵表达式树的递归校验函数。位集合判定为 `required | provided == provided`，即 required 必须是 provided 的子集。
- `ImportIntoRuntime`：本文件的环境接口，关联类型覆盖上下文、请求、计划、表、SELECT 执行器、导入计划、表达式、控制器、任务、作业信息和错误；方法按计划构建、准备、提交、等待、结果读取、资源关闭、权限检查和取消分组。
- `ValidateImportIntoColAssignmentsWithEncodeCtx(runtime, import_plan, assignments)`：先取得编码上下文的属性集合，再逐项构建表达式并校验；任一步错误立即返回。
- `ImportIntoExec<R>`：主执行器状态，持有 `runtime`、可选 `select_executor`、延迟创建的 `controller`、原始语句、逻辑计划、目标表和一次性结果标志 `data_filled`。
- `newImportIntoExec(...)`：只组装尚未执行的状态；不建导入计划、不访问文件，也不提交任务。
- `ImportIntoExec::{Next, fillJobInfo, submitTask, waitTask, importFromSelect, Close}`：分别负责总入口、结果回填、路由提交、等待/取消、SELECT 管线和资源关闭。
- `ImportIntoAction::{Cancel}` 与 `ImportIntoActionExec<R>`：当前动作枚举只有取消；`ImportIntoActionExec::Next` 先调用 `checkPrivilegeAndStatus`，再调用 `cancelAndWaitImportJob`。
- `cancelAndWaitImportJob(runtime, context, job_id)`：薄委托层，实际的分布式任务探测、取消、等待及悬空作业兜底必须由运行时实现。

本文件没有模块级常量、条件编译项或具体运行时实现；所有上述 trait、类型和函数均为公开符号，只有各 `impl` 内部字段访问与控制流属于实现细节。

## 执行流程

`ImportIntoExec::Next` 的文件来源流程如下：

1. `grow_and_reset_request` 清空并准备结果缓冲。若 `data_filled` 已为真，则返回空结果，维持 executor 的一次性输出语义。
2. `create_import_plan` 结合上下文、逻辑计划和目标表生成导入计划；`column_assignments` 取出赋值，再由 `ValidateImportIntoColAssignmentsWithEncodeCtx` 校验编码兼容性。
3. `create_controller` 消费导入计划并保存控制器。此后依赖控制器的方法都以 `controller` 已初始化为不变量，代码用 `expect` 表达该内部前置条件。
4. 若存在 `select_executor`，立即进入 `importFromSelect`：先置 `data_filled = true`，再把控制器和 SELECT 执行器交给 `import_from_select_pipeline`。trait 注释要求运行时并发执行生产/导入、使用非会话池 chunk、关闭通道、尽力刷新统计并设置影响行数和消息；本文件本身不实现这些细节。
5. 文件来源先询问 `should_use_async_prepare`。同步准备时调用 `initialize_data_files`；若 `next_generation_kernel` 为真，再调用 `calculate_resource_parameters`。异步准备会跳过这两步，把“文件初始化前”标志传给后续前置检查。
6. `check_requirements_in_new_session` 必须使用新建并关闭的会话，以免用户会话的 processlist 或 stale-read 状态污染预检查；之后 `initialize_tikv_configs` 设置存储侧配置。
7. `submitTask` 检查路径。本地路径先 `populate_chunks`，再以 `populated_chunks = true` 提交单机任务；远端路径在 `distributed_tasks_enabled()` 为真时提交分布式任务，否则以 `populated_chunks = false` 提交单机任务。
8. 非 detached 作业通过 `waitTask` 等待完成或暂停；普通错误原样返回，上下文取消则转为后台取消并等待。
9. `fillJobInfo` 先将 `data_filled` 置真，再用系统会话读取作业并写入请求。下一次 `Next` 只返回清空后的空请求。

取消流程为：`ImportIntoActionExec::Next` 调用 `checkPrivilegeAndStatus`，运行时结合 SUPER 权限读取可见作业；只有 `job_can_cancel` 为真才继续，否则返回 `invalid_cancel_operation`；最终由 `cancelAndWaitImportJob` 委托运行时完成取消和等待。虽然 `action` 字段目前只有 `Cancel`，当前代码并未按该字段做 `match`，新增动作时不能默认复用现有路径。

## 数据与状态

- `controller: Option<R::Controller>` 从 `None` 开始，在计划和赋值校验成功后才写入。SELECT 与文件两条路径共享同一个控制器，`Close` 仅在其存在时关闭。
- `data_filled` 是结果集幂等门闩：文件导入在 `fillJobInfo` 开始时置真；SELECT 导入在进入运行时管线前置真。后者即使管线返回错误也保持为真，与当前 Rust 源码一致。
- `statement` 保留原始 SQL，提交单机或分布式任务时传给运行时；`plan` 和 `table` 用于生成导入计划及控制器。
- `select_executor: Option<_>` 同时是数据源模式判定：`Some` 跳过文件初始化、任务提交、任务等待和作业行回填，`None` 才走文件任务路径。
- 属性位掩码使用 `u64`。校验只判断“所需位是否全部包含”，不解释每个位的业务含义；位定义和表达式构建属于具体运行时。
- `job_id` 贯穿等待、系统会话查询和取消；`Task` 只用于正常等待。管理执行器独立保存 `action` 与 `job_id`，不共享主执行器的控制器状态。

## 依赖与调用关系

RustCodeGraph 对本文件的内部边显示：`ImportIntoExec::Next` 调用 `ValidateImportIntoColAssignmentsWithEncodeCtx`、`submitTask`、`waitTask`、`fillJobInfo`、`importFromSelect` 以及相应运行时钩子；校验函数调用 `checkExprWithProvidedProps`，后者递归调用自身并访问 `ImportExpression`；动作执行器的 `Next` 调用 `checkPrivilegeAndStatus` 和 `cancelAndWaitImportJob`。

crate 边界由 [`pkg/executor/Cargo.toml`](Cargo.toml) 确认：库入口是 `lib.rs`，`nextgen` feature 转发到 `astersql-dxf-importinto/nextgen`，并声明了 importer、DXF handle/storage/importinto、表达式、对象存储、会话和 TiKV 等相关依赖。不过 `import_into.rs` 没有直接 `use` 这些 crate；它通过 `ImportIntoRuntime` 隔离具体依赖，不能把 Cargo 清单中的每个包都视为本文件的直接调用对象。

上游证据分两类：

- Rust 模块由 `lib.rs` 公开；[`tests/realtikvtest/importintotest/job_runtime.rs`](../../tests/realtikvtest/importintotest/job_runtime.rs) 的 `execute` 构造 `newImportIntoExec` 并调用 `Next`/`Close`，`cancel` 构造 `ImportIntoActionExec`，同文件的 `Runtime` 是仓库搜索到的唯一 trait 实现。
- Go 生产链由 `newImportIntoExec` 构造执行器并实现 `exec.Executor`，其 `Next` 直接连接 importer、DXF、对象存储、会话和 TiKV 配置。本 Rust 文件保留了该编排形状，但具体生产适配尚不能由现有 Rust 引用证明。

## 错误处理与边界

- 几乎所有外部操作都返回 `Result` 并用 `?` 原样短路；运行时错误类型必须实现 `From<UnsupportedImportFunction>`，使表达式错误可进入统一错误通道。
- 不支持函数的错误包含规范化函数名和零基赋值下标，格式固定为 `FUNCTION ... is not supported in IMPORT INTO column assignment, index ...`。
- 递归只发生在标量函数节点。非标量节点即使暴露了 `children()` 也不会下降，这是刻意复刻 Go 类型判断的边界，而不是通用表达式遍历器。
- `controller.as_mut().expect(...)` 和 `select_executor.as_mut().expect(...)` 依赖本文件内部顺序；若将辅助方法改成可被任意状态调用，必须先消除或显式维护这些不变量，否则会 panic。
- `waitTask` 只把运行时明确识别为上下文取消的错误转换为后台取消；任务失败、暂停相关错误和其他等待错误保持原样。
- `fillJobInfo` 与 `importFromSelect` 都在可能失败的后续工作之前设置 `data_filled`。重试语义若要改变，必须同步审查 Go 行为和 executor 的重复 `Next` 协议。
- 权限/状态检查先于取消：非 SUPER 用户的作业可见性由 `get_job_for_action` 决定；不可取消状态必须返回运行时构造的非法操作错误。
- Go 版 `cancelAndWaitImportJob` 还包含“先探测 DXF 任务，存在则事务取消并等待；缺失则仅取消仍 pending 的悬空作业；其他探测错误直接返回”的竞态保护。Rust 顶层函数只定义委托接口，真正对应实现目前位于独立的 [`pkg/executor/import_into_storage.rs`](import_into_storage.rs)，不应误认为这段兜底逻辑写在本文件内。

## 并发与资源生命周期

- `Next` 自身是顺序编排；并发边界被封装在 `import_from_select_pipeline`、任务提交/调度和等待钩子内。本类型没有声明 `Send`/`Sync` 约束，也没有内部锁或通道。
- SELECT 管线的资源协议由 trait 注释固定：生产者和表导入器并发运行，生产者退出时关闭通道，chunk 容量增长但不超过最大值，统计刷新是 best effort。Go 对照用 `errgroup.WithContext` 传播任一协程失败、容量为 1 的通道传递 chunk，并在退出时关闭 table importer；Rust 运行时必须保持这些语义，不能把 trait 调用理解成同步逐行导入。
- 文件任务若非 detached，调用方持有执行器直到任务完成/暂停；detached 仍会立即读取并返回作业信息，但不会等待任务终态。
- 用户连接取消后，原上下文已不可用，`waitTask` 明确走 background 取消钩子。具体运行时必须同时保证取消请求和等待完成，避免遗留活动导入作业。
- `Close` 先关闭已创建的控制器，再关闭基础执行器；即使控制器关闭没有返回值，基础关闭错误仍会传播。调用者仍负责在 `Next` 成功或失败后调用 `Close`；RealTiKV 适配器的 `execute` 展示了这一调用次序。
- Go 版前置检查和 SELECT 导入都创建并 `defer CloseSession` 新会话。本 trait 用 `check_requirements_in_new_session` 和 `import_from_select_pipeline` 的契约要求运行时承担相同的新会话创建/关闭责任。

## 与 Go 版本的对应关系

[`pkg/executor/import_into.go`](import_into.go) 是逐符号对照来源：Rust 的 `ImportIntoExec`、`newImportIntoExec`、`Next`、赋值校验、`fillJobInfo`、`submitTask`、`waitTask`、`importFromSelect`、`Close`、`ImportIntoActionExec` 与取消入口沿用了 Go 名称和主流程。主要差异如下：

- Go 类型直接嵌入 `exec.BaseExecutor` 并调用具体包；Rust 以 `ImportIntoRuntime` 关联类型和方法抽象全部外部状态，因此这里只能证明编排顺序，具体实现质量必须另查运行时。
- Go `Next` 为上下文标注 internal source type，并在新会话间继承 materialized-view maintenance 标志；Rust trait 没有单独暴露这两个动作，只能由较粗粒度的运行时钩子内部实现，当前测试运行时对此多为简化实现。
- Go `importFromSelect` 在函数开头也设置 `dataFilled = true`，尽管旧注释曾说“不需要”；Rust与实际赋值行为保持一致，并把完整并发管线压缩到一个运行时方法。
- Go `cancelAndWaitImportJob` 的 DXF 探测与 dangling-job 竞态兜底不在 Rust 本文件中；Rust 通过 `cancel_and_wait_import_job` 委托，相关存储实现与 nextgen 测试位于 `import_into_storage.rs` 和 `import_into_test.rs`。
- Rust `ImportIntoAction` 已建模为枚举，但 `Next` 目前没有分派枚举值；Go 用 `ast.ImportIntoActionTp` 保存动作，同样由当前构造路径限定为 CANCEL。
- Go 新增的 `inheritMViewMaintenanceFlag` 及其回归测试没有在本文件形成细粒度 Rust API。若生产接线要求完全对齐，应在运行时实现或 trait 契约中明确验证，而不能假设自动继承。

测试对照方面，[`pkg/executor/import_into_test.go`](import_into_test.go) 的 `TestImportIntoValidateColAssignmentsWithEncodeCtx` 给出支持/拒绝矩阵与精确错误文本；Rust 独立测试 [`pkg/executor/import_into_test.rs`](import_into_test.rs) 覆盖递归子函数、赋值下标、错误文本、非标量不递归以及 `uncompress(compress(@raw))`。后者带 `nextgen` 的取消测试主要验证 `import_into_storage.rs`，不是本文件薄委托自身的单元逻辑。

## 扩展指南

- 新增或改变列赋值支持时，应修改具体 `ImportExpression`/运行时表达式适配，而不是放宽 `checkExprWithProvidedProps` 的子集判定；同步扩展 Rust `import_into_test.rs` 和 Go `TestImportIntoValidateColAssignmentsWithEncodeCtx`，尤其覆盖嵌套标量、非标量边界、规范化函数名和赋值下标。
- 增加新的数据源或提交策略时，接入点是 `submitTask` 及 `ImportIntoRuntime` 的路径/提交钩子。需保持本地文件先切块、远端是否分布式由开关决定的现有路由，并评估大文件切块内存、远端重试和 detached 可观测性。
- 修改准备阶段时，应保持“同步初始化后计算 nextgen 资源”和“异步准备在文件初始化前做对应检查”的顺序；新会话隔离、TiKV 配置初始化和错误短路不可被悄然省略。
- 实现生产 `ImportIntoRuntime` 时，应逐项对照 Go 具体实现，特别验证 internal source type、maintenance flag 继承、系统会话权限隔离、SELECT 通道背压/取消、统计刷新、影响行数和消息。仅让 trait 编译通过不构成行为对齐。
- 新增作业动作时，必须扩展 `ImportIntoAction` 并在 `ImportIntoActionExec::Next` 显式分派；同时定义各动作的权限、合法状态和错误，并添加独立 Rust 测试，不能继续无条件调用取消函数。
- 调整取消流程时，应同时审查 `import_into_storage.rs` 及 nextgen 竞态测试，保持“任务存在时取消并等待、确认为未找到时才尝试 pending 作业兜底、探测失败不触碰作业、状态已变化则拒绝取消”的安全边界。
- 测试逻辑应继续放在独立的 `pkg/executor/import_into_test.rs` 或适当集成测试文件，不要嵌回生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/executor/import_into.rs`；`node --file pkg/executor/import_into.rs --offset 1 --limit 500` 读取了完整 439 行，并标出测试文件引用；`query` 区分了 Go/Rust 同名的 `ImportIntoExec`、`newImportIntoExec`、`ValidateImportIntoColAssignmentsWithEncodeCtx`、`cancelAndWaitImportJob`；`callees` 核对了 `Next`、校验、提交、等待、结果回填和动作取消的内部边。callers 对同名 Go/Rust 符号没有可靠消歧，所以上游结论另以仓库引用搜索核验。
- Rust 源与装配：[`pkg/executor/import_into.rs`](import_into.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/Cargo.toml`](Cargo.toml)。Cargo 确认 crate 名、库入口、`nextgen` feature 与相关依赖；模块入口确认生产模块公开和独立测试模块装配。
- Rust 调用与测试：[`tests/realtikvtest/importintotest/job_runtime.rs`](../../tests/realtikvtest/importintotest/job_runtime.rs) 提供当前可检索到的 `ImportIntoRuntime` 实现及主/取消执行器调用；[`pkg/executor/import_into_test.rs`](import_into_test.rs) 提供表达式边界与 nextgen 取消竞态证据；[`pkg/executor/import_into_storage.rs`](import_into_storage.rs) 是取消存储语义的真实 Rust 实现位置。
- Go 对照：[`pkg/executor/import_into.go`](import_into.go) 核对完整生产编排和资源生命周期；[`pkg/executor/import_into_test.go`](import_into_test.go) 核对赋值校验矩阵、maintenance flag 和预检查边界。
- 人工复核结论：本文区分了本文件直接实现、trait 契约委托、独立存储模块实现和 Go 生产实现；未把 Cargo 依赖清单或 Rust 测试适配器描述为已经接入的 Rust 生产服务器路径。
