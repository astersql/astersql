# `pkg/testkit/testkit.rs`

## 文件定位

`pkg/testkit/testkit.rs` 是 `astersql-testkit` crate 的同步 SQL 测试门面。crate 根模块 `pkg/testkit/lib.rs` 以 `pub mod testkit` 装配本文件，并再导出 `NewTestKit`、`TestKit`、`TestSession`、`TestSessionVars` 和 `TestMemTracker`。它不实现解析器、优化器或存储，而是把测试代码使用的 Go 风格 API 适配到 `pkg/testkit/db_driver.rs` 的 `Database: Send + Sync + 'static` 抽象，再把查询结果交给 `pkg/testkit/result.rs::Result` 做断言。

该文件主要供 Rust 单元、集成和 RealTiKV 测试构造会话并执行 SQL。直接使用点包括 `tests/readonlytest/readonly_test.rs`、`tests/realtikvtest/txntest/*.rs` 和多个 RealTiKV harness；RustCodeGraph 将该文件标记为被 210 个文件使用。它是测试基础设施，不在数据库服务的线上请求主链中。

## 核心职责

- `TestKit` 保存共享存储、当前会话、连接 ID、诊断注释和最近一次执行结果，为测试提供 `Exec`/`Query`、`MustExec`/`MustQuery`、错误断言及执行计划断言。
- `TestSession` 暴露与 Go `Session()` 常用接口同形的会话观察和控制方法，但所有真实行为都委托给同一个 `Arc<dyn Database>`。
- `TestSessionVars`、`TestMemTracker`、`TestDiskTracker` 是只读快照视图，用于检查内存追踪、磁盘峰值和 CTE 临时状态是否已清理。
- `TestKit::new` 在创建会话前调用 `astersql_planner_core::InstallPlannerExpressionFactory`，补偿 Rust 没有 Go 包级初始化钩子的差异；随后调用 `Database::create_session` 派生独立会话。
- `MustUseIndex`、`MustNoIndexUsed`、`MustPointGet`、`HasPlan` 和 `HasKeywordInOperatorInfo` 通过执行 `EXPLAIN` 检查计划输出，不直接访问规划器内部结构。

本文件只覆盖已列出的 Rust API，不等价于 Go `pkg/testkit/testkit.go` 的完整功能集合。例如 Go 的 session manager 注册、`RefreshSession`、上下文版执行、eventually 断言和更多计划/分区辅助函数在本文件中没有对应实现。

## 主要符号

- `NEXT_CONNECTION_ID: AtomicU64`：从 1 开始为无法从具体 `Database` 取得连接 ID 的 TestKit 分配进程内递增后备值。
- `TestKit`：核心可克隆句柄。`store` 保留原始存储，`database` 指向派生会话；`comments` 用于失败诊断；`last_result` 支持 `CheckExecResult`。
- `TestSession`：会话代理。它包含 prepared statement 生命周期（`PrepareStmt`、`ExecutePreparedStmt`、`DropPreparedStmt`）、内部请求执行、事务/快照/提示/统计状态观察，以及认证、行编码、inspection cache、故障注入等测试钩子。
- `TestSessionVars`、`TestMemTracker`、`TestDiskTracker`：由 `GetSessionVars` 一次性读取后端状态形成的值快照；`GetChildrenForTest` 仅按数量构造空指针向量，以复现 Go 测试只检查长度的用法。
- `TestStatementHints`：最近语句的 `MemQuotaQuery` 和 `MaxExecutionTime` 值对象。
- `TestKit::new`、关联函数 `TestKit::NewTestKit`、包级 `NewTestKit`：三种构造入口，最终都进入同一初始化逻辑。
- `TestKit::Exec`/`Query`：最薄的执行边界；前者缓存 `ExecutionResult`，后者返回 `QueryRows`。
- `TestKit::MustExec`/`MustQuery`：将 `TestResult` 转为测试失败 panic；`MustQuery` 还把 `QueryRows::string_rows` 转成带注释的 `result::Result`。
- `ExecToErr`/`QueryToErr`、`MustExecToErr`/`MustQueryToErr`、`MustGetErrMsg`/`MustContainErrMsg`：预期失败路径及消息断言。
- `failure`：将 SQL、`TestError` 和已登记注释组成 panic 文案。

## 执行流程

1. 测试把实现 `Database` 的存储传给 `TestKit::new` 或包级 `NewTestKit`。
2. 构造函数安装 planner expression factory，克隆原始 `store`，再调用 `create_session`。返回 `Some` 时使用独立会话；返回 `None` 时继续使用原对象；返回 `Err` 时立即 panic。构造完成后以 relaxed 原子递增生成后备连接 ID，并清空注释与最近执行结果。
3. 非查询路径 `MustExec -> Exec -> Database::execute`。成功时 `Exec` 更新 `last_result`，供 `CheckExecResult` 比较影响行数和自增 ID；错误时 `MustExec` 用 `failure` panic。
4. 查询路径 `MustQuery -> Query -> Database::query`。成功后每个 `DbValue` 被字符串化并包装为 `result::Result`；失败则附带 SQL 和注释 panic。
5. prepared statement 有两条入口：`TestKit::Prepare` 构造 `PreparedStatement` 便捷对象；`TestSession::{PrepareStmt, ExecutePreparedStmt, DropPreparedStmt}` 显式管理 statement ID。生命周期和错误均由后端实现负责。
6. `ExecuteInternal`/`QueryInternal` 先用 `astersql_kv::GetInternalSourceType` 拒绝没有内部来源标记的 context，再调用后端内部执行接口；context 只用于入口校验，实际内部作用域由 `Database` 实现建立。
7. 计划断言给原 SQL 加 `explain ` 前缀：索引断言检查 access-object 列中的 `index:<name>`，`HasPlan` 检查首列算子 ID，operator-info 检查标准第五列，并兼容四列紧凑输出的第四列；`MustPointGet` 要求恰有一行且首列含 `Point_Get`，然后执行原查询并返回结果。

## 数据与状态

`TestKit` 同时保存 `store` 和 `database`：二者初始共享同一后端，但后者可能是 `create_session` 返回的会话对象。`Store()` 总是返回原始存储，`Session()` 返回当前会话代理。两者以及代理内部都使用 `Arc`，所以克隆不复制底层数据库。

`connection_id` 是构造时取得的后备值；`ConnectionID` 优先返回后端的 `connection_id_for_test()`。原子使用 `Ordering::Relaxed`，只要求唯一递增分配，不承担跨线程状态同步。派生 `Clone` 会复制已有 `connection_id`、注释和 `last_result`，并共享两个 `Arc`；因此克隆现有 TestKit 不会分配新连接 ID，也不代表新会话。

`comments` 是 TestKit 本地的 `Vec<String>`，通过 `AddComment`/`ClearComment` 管理，只进入断言失败信息。`last_result` 仅由成功的 `Exec` 覆盖；`Query` 不修改它。SessionVars/Tracker 类型保存取得时的标量快照，后端状态随后变化不会更新旧快照。

## 依赖与调用关系

上游方面，`pkg/testkit/lib.rs` 对外再导出本文件 API；`pkg/testkit/testkit_test.rs` 直接构造 `TestKit`，RealTiKV 和事务测试也通过 `NewTestKit` 建立共享存储上的多个独立会话。精确搜索可见 `tests/realtikvtest/txntest/txn_state_test.rs` 等文件用同一 store 构造主会话和 observer，体现其测试协调角色。

下游方面：

- `crate::db_driver::{Database, DbValue, ExecutionResult, PreparedStatement, QueryRows}` 定义全部 SQL、会话状态和测试钩子的后端契约；本文件不绕过该 trait 操作具体 session。
- `crate::result::Result` 承担结果集比较和错误展示；`MustQuery` 是从数据库行到断言对象的桥。
- `crate::{TestError, TestResult}` 是 crate 级统一错误边界。
- `astersql_planner_core::InstallPlannerExpressionFactory` 是构造期必需接线；`astersql_kv::Context` 和 `GetInternalSourceType` 约束内部请求；`astersql_session::runtime::RuntimeStaleReadState` 暴露 stale-read 状态。
- `pkg/testkit/Cargo.toml` 将本 crate 声明为 `astersql-testkit`，`lib.rs` 为 crate 根，并通过 workspace path 依赖连接 planner、session、kv、domain、store 等内部 crate；本文件实际直接使用的跨 crate 依赖集中在 planner-core、kv、session runtime 和 parser-auth 类型。

RustCodeGraph 的文件关系显示该文件被广泛使用，但当前索引对 Rust `impl` 中这些方法的 `callers`/`callees` 查询返回空结果；上游调用点因此以精确 `rg` 搜索补证，不能把空图边解释为“无人调用”。

## 错误处理与边界

可恢复后端错误统一为 `TestResult<T>`/`TestError`。`Exec`、`Query` 和大多数 `TestSession` 方法原样传播后端错误；`Database` 的可选测试能力常以默认值或“database does not expose ...”错误表示不支持，具体定义在 `pkg/testkit/db_driver.rs`。

作为测试断言门面，`Must*` 方法有意将不满足预期的结果转为 panic。构造期 planner factory 安装失败、`create_session` 出错，以及 `TestKit::StaleReadStateForTest` 后端不暴露状态也会 panic。`ExecToErr`/`QueryToErr` 在语句意外成功时 panic；消息断言分别要求完全相等或包含片段。

边界条件包括：`MustPointGet` 强制 EXPLAIN 只有一行；计划辅助方法依赖当前 Rust/Go 兼容的 EXPLAIN 列布局；`HasKeywordInOperatorInfo` 只显式兼容五列标准输出和恰好四列的紧凑输出。内部请求方法只接受带非空 internal-source 的 context。Tracker 默认值可能表示后端没有提供更具体的观测能力，调用者不应把所有零值都解释成执行过后的真实统计。

## 并发与资源生命周期

`Database` 要求 `Send + Sync + 'static`，并由 `Arc` 共享，因此多个 TestKit/Session 句柄可以持有同一后端。文件本身没有 mutex、channel、异步任务或后台线程；它也没有为 `TestKit` 声明额外并发安全保证。需要修改 `comments` 或 `last_result` 的 API 使用 `&mut self`，会话只读/委托方法多使用 `&self`，实际并发语义由具体 `Database` 实现决定。

资源生命周期由后端接口显式表达：构造时可派生 session；`TestSession::close` 关闭底层会话；prepared statement 必须经过 prepare、execute、drop。`TestKit` 自身没有 `Drop` 实现，离开作用域只释放 `Arc` 引用，不会自动调用 `Database::close`。独立测试 `TestMultiStatementInTk` 连续执行 100 次多语句查询，要求只返回第一份结果，并在每次执行后保持 MemTracker 子节点数为 0，验证语句上下文不会累积泄漏。

## 与 Go 版本的对应关系

Rust `TestKit` 对应 `pkg/testkit/testkit.go::TestKit` 的核心用途：保存 store/session，执行 SQL，提供 must-style 断言、错误断言及 EXPLAIN 辅助。`MustUseIndex`、`MustNoIndexUsed`、`CheckExecResult`、`MustPointGet`、`HasKeywordInOperatorInfo` 的判断意图与 Go 同名方法一致。`pkg/testkit/testkit_test.rs::TestMultiStatementInTk` 也逐步复刻 `pkg/testkit/testkit_test.go::TestMultiStatementInTk` 的 100 次多语句和 MemTracker 清理回归。

关键差异如下：

- Go 构造器接收 `testing.TB` 与 `kv.Storage`，建立 testify assertions、chunk allocator、真实 session，并把会话登记到 mock session manager；Rust 接收 `Arc<dyn Database>`，不持有测试框架对象或 allocator，靠 trait 适配具体后端。
- Go `RefreshSession` 会创建 session 并执行 `select 3` 强制加载系统变量；Rust 构造器通过 `create_session` 和 planner factory 安装完成必要接线，没有该 SQL 预热或 session manager 登记。
- Go `ExecWithContext` 自己负责解析、多语句结果关闭、prepared 参数路径、command 状态和 allocator 清理；Rust `Exec`/`Query` 把这些职责整体下沉给 `Database` 实现。
- Go `MustQuery` 对部分 information_schema 查询使用 failpoint 进行二次等价检查；Rust 本文件没有这一分支。
- Go API 范围明显更大。Rust 当前是按已迁移测试需要提供核心子集及额外的具体后端测试观测钩子，不应据此宣称完整移植。

## 扩展指南

新增通用 SQL 断言时，优先组合 `Exec`、`Query`、`MustQuery` 和 `result::Result`，并把断言放在 `TestKit` impl；新增会话内部观测或控制能力时，在 `db_driver.rs::Database` 增加清晰的能力接口，由 `TestSession` 做薄代理，并在具体数据库适配器中实现。默认实现必须明确区分“后端不支持”与合法零值，避免测试误判。

扩展 prepared statement 或资源操作时，应保留显式释放路径，并在独立测试文件中覆盖失败后的清理。不要把 Rust 测试写入本生产文件；核心回归应更新同目录 `pkg/testkit/testkit_test.rs`，涉及具体 session adapter 的行为则同步其独立测试。Go 同名行为存在时，应同时阅读 `pkg/testkit/testkit.go` 和 `pkg/testkit/testkit_test.go`，保持实际语义，而不是只复刻方法名。

修改计划断言时要基于 EXPLAIN 的真实列契约，并覆盖标准与紧凑行布局，尤其防止在 access-object 或 execution-info 中的诱饵文本被错当成算子/operator info。修改构造和 clone 语义时需评估共享 session、连接 ID 唯一性与并发测试隔离。添加跨 crate 类型或接口还需同步 `pkg/testkit/Cargo.toml`，并按仓库规则评估构建元数据影响。

## 验证依据

- Rust 源码：`pkg/testkit/testkit.rs`（579 行），核对全部模块级状态、6 个公开结构体、各 impl、两个 `NewTestKit` 入口及私有 `failure`。
- crate 边界：`pkg/testkit/lib.rs` 的模块声明、再导出和独立测试挂载；`pkg/testkit/Cargo.toml` 的 crate 名、根路径、porting 元数据和 workspace 依赖。
- 下游契约：`pkg/testkit/db_driver.rs::Database`、`QueryRows::string_rows`；`pkg/testkit/result.rs::Result`。
- Go 对照：`pkg/testkit/testkit.go` 的 `TestKit`、构造/session、执行、错误和 EXPLAIN 辅助；`pkg/testkit/testkit_test.go::TestMultiStatementInTk`。
- Rust 测试：`pkg/testkit/testkit_test.rs::TestMultiStatementInTk`、`plan_assertions_use_the_access_object_column`、`plan_and_operator_info_helpers_use_the_go_columns`。这些测试分别证明多语句资源清理、索引列选择、算子首列选择及标准/紧凑 operator-info 列兼容。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/testkit/testkit.rs` 找到目标；`node --file` 读取目标、crate 根、Go 对照和测试；`query TestKit`/`query NewTestKit`/`query MustPointGet` 确认同名 Rust 与 Go 符号。对 Rust impl 方法的 callers/callees 查询无返回，故使用 `rg -n --glob '*.rs' 'TestKit::new\\(|\\bNewTestKit\\(' pkg tests br cmd` 补充直接调用证据。
- 人工复核结论：该文件存在的原因是为 Rust 数据库测试提供统一、Go 兼容的 SQL 会话与断言门面；运行时由构造器建立后端会话，由 `Database` 执行真实操作，由 `Result` 断言结果；安全扩展应从 `TestKit` 组合层或 `Database` 能力边界接入，并同步独立测试。
