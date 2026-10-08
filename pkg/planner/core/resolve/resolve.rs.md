# `pkg/planner/core/resolve/resolve.rs`

源文件：[`resolve.rs`](./resolve.rs)

## 文件定位

本文件属于 `astersql-planner-core-resolve` crate，是解析器 AST 与计划器之间的名称解析状态载体。crate 入口 `pkg/planner/core/resolve/lib.rs` 将解析器 AST 重新导出为 `ast`，将 `DBInfo`、`TableInfo` 重新导出为 `model`，并公开本文件的 `Context`、`NodeW`、`TableNameW`。`pkg/planner/core/resolve/Cargo.toml` 表明该 crate 只直接依赖 `astersql-parser-ast` 与 `astersql-meta-model`，没有 feature 分支。

它不执行 SQL 解析、InfoSchema 查找或逻辑计划构建；它保存这些阶段之间共享的“某个 AST 表名节点已绑定到哪些数据库/表元数据”这一结果。Rust 计划器接口已经以 `&resolve::NodeW` 接收语句，例如 `pkg/planner/optimize.rs::buildPlan` 和 `pkg/planner/core/planbuilder_runtime.rs::PlanBuilder::Build`；后者当前只把 `NodeW.node` 交给 `BuildNodeRef`。仓库搜索未发现本文件之外的非测试 Rust 代码调用 `NewNodeW`、`NewNodeWWithCtx` 或 `NodeW::new`，因此完整的 Rust 预处理填充链目前不能从生产调用点得到验证，不能把 Go 已有主链视为 Rust 已接通。

## 核心职责

1. `TableNameW` 把一个 `Rc<ast::TableName>` 与可选的 `DBInfo`、`TableInfo` 绑定在一起，表达名称解析的结果。
2. `NodeW` 把通用 `ast::NodeRef` 与一个 `Context` 组合，使替换 AST 根节点时仍可沿用同一份解析结果。
3. `Context` 以 AST `TableName` 的对象身份而非字段值为键保存 `TableNameW`。两个 schema/name 相同但由不同 `Rc` 分配的节点是两个键。
4. 同时提供惯用 Rust 命名与 Go 风格兼容入口。`AddTableName`、`GetTableName` 等仅委托给对应 snake_case 方法，不另建状态或改变语义。

该文件是状态容器而不是 resolver 算法：解析时机、InfoSchema 查询、重复表/别名校验和错误产生应由上游预处理器或计划构建器负责。

## 主要符号

### `TableNameW`

- `table_name: Rc<ast::TableName>`：键所对应的原始 AST 对象；`Context::add_table_name` 从这个 `Rc` 提取裸指针作为键。
- `db_info: Option<Rc<model::DBInfo>>`、`table_info: Option<Rc<model::TableInfo>>`：解析出的元数据。使用 `Option` 表示结构本身不强制两项一定存在；本文件也不校验二者的一致性。

### `NodeW`

- `node: ast::NodeRef` 是公开 AST 根节点。
- `resolve_ctx: Context` 是私有字段，只能通过构造函数、克隆函数和 getter 传播。
- `NodeW::new` 为节点创建全新的空 `Context`。
- `NodeW::new_with_ctx` 使用调用方提供的上下文。
- `NodeW::clone_with_new_node` 替换 AST 节点，同时浅克隆原上下文；它不是深拷贝解析表。
- `NodeW::get_resolve_context` 返回共享同一底层表的 `Context` 克隆。
- `CloneWithNewNode`、`GetResolveContext` 是对应方法的 Go 风格门面。

### `Context` 与 `TableNameMap`

- 私有别名 `TableNameMap = HashMap<*const ast::TableName, Rc<TableNameW>>` 明确了按对象地址键控。
- `Context { table_names: Rc<RefCell<TableNameMap>> }` 允许单线程共享所有权和运行时可变借用。
- `Context::new`/`NewContext` 创建空映射。
- `add_table_name`/`AddTableName` 插入绑定；同一 AST 指针再次插入会覆盖旧值。
- `get_table_name`/`GetTableName` 按 `Rc::as_ptr` 查找并克隆结果的 `Rc`，未找到时返回 `None`。
- `get_table_names`/`GetTableNames` 返回整个内部映射的 `RefMut`，调用方可新增、替换、删除或清空条目。
- `NewNodeW`、`NewNodeWWithCtx`、`NewContext` 是包级 Go 风格工厂。

文件没有模块级常量、trait、enum、条件编译项或异步函数。

## 执行流程

典型的预期流程由本文件的 API、Go 对照实现与 Rust 测试共同证明：

1. 调用 `NodeW::new(node)` 创建 AST 包装和独立的空解析上下文；或者用 `NodeW::new_with_ctx(node, context)` 接入已有上下文。
2. 上游解析阶段为 AST 中的表名取得 `DBInfo`/`TableInfo`，构造持有同一个 `Rc<ast::TableName>` 的 `TableNameW`。
3. `Context::add_table_name` 通过 `Rc::as_ptr(&table_name_w.table_name)` 取得对象身份，写入共享 `HashMap`。
4. 后续阶段必须持有原来的 `Rc<ast::TableName>`，再由 `get_table_name` 用同一地址查询。仅重新构造一个字段值相同的 `TableName` 不会命中。
5. AST 被改写成新根节点但解析结果仍适用时，`clone_with_new_node` 生成新 `NodeW`；两个包装通过克隆的 `Context` 看到同一映射。
6. Rust 当前的计划入口中，`pkg/planner/optimize.rs::buildPlan` 将 `&NodeW` 传给 `PlanBuilder::Build`，`Build` 在 `pkg/planner/core/planbuilder_runtime.rs` 中读取 `node.node` 并分派 INSERT、DELETE、UPDATE、EXPLAIN 或结果集构建。现有非测试 Rust 调用证据没有显示这些路径读取 `resolve_ctx`，故解析上下文在完整 Rust 主链中的消费仍属于未接线/未验证部分。

Go 侧的已实现主链更完整：`pkg/planner/core/preprocess.go` 在预处理阶段通过 `AddTableName` 填充绑定；`logical_plan_builder.go`、`point_get_plan.go` 读取并在 AST 改写时复制绑定；`optimizer.go` 遍历 `GetTableNames`。这些是迁移语义的对照证据，不是 Rust 已实现证据。

## 数据与状态

`Context::clone` 只克隆外层 `Rc`，所以所有克隆共享一个 `RefCell<HashMap<...>>`。`NodeW::new` 每次调用则创建新的 `Rc`，不同新建节点默认互不污染。值也是 `Rc<TableNameW>`，查询只增加引用计数，不复制 AST 或元数据对象。

键是从 `Rc<ast::TableName>` 得到的裸指针。映射值持有该 `Rc`，因此条目存在期间键所指对象不会因引用计数归零而释放；正常通过 API 插入时不会形成悬空键。若调用方经 `get_table_names` 直接构造与值中 `table_name` 不一致的裸指针键，本文件无法维护这一不变量，因此这种底层修改应只用于与 Go 返回内部 map 相同的必要操作。

重要不变量如下：

- 查找身份由分配地址决定，不由 `Schema`/`Name` 值决定。
- 同一地址重复插入保留最后一个 `TableNameW`。
- 替换 `NodeW.node` 不会自动重算或清空解析上下文；调用方负责确认旧绑定仍适用于新 AST。
- `db_info` 和 `table_info` 可独立为 `None`，本文件不保证完整绑定。

## 依赖与调用关系

下游直接依赖很小：标准库的 `Rc`、`RefCell`、`RefMut`、`HashMap`，以及 crate 入口提供的 `ast::{NodeRef, TableName}` 和 `model::{DBInfo, TableInfo}`。本文件无网络、存储、InfoSchema、会话或表达式依赖。

上游关系分为三层：

- crate 边界：`pkg/planner/core/resolve/lib.rs` 公开 `Context`、`NodeW`、`TableNameW`；`pkg/planner/core/Cargo.toml` 以 `resolve-dependency` 引入该 crate，`pkg/planner/Cargo.toml`、`pkg/planner/memo/Cargo.toml`、规则和多个测试 crate 也声明路径依赖。
- Rust 生产接口：`pkg/planner/optimize.rs` 的优化服务 trait、`buildPlan`、`optimize`、诊断函数等以 `&resolve::NodeW` 传递 AST；`pkg/planner/core/optimizer_runtime.rs::OptimizeAstNodeFn` 和 `pkg/planner/core/planbuilder_runtime.rs::PlanBuilder::Build` 也暴露该类型。`pkg/planner/core/plan_cache_utils.rs` 在缓存结构中保存可选 `resolve_dependency::Context`。
- Go 对照调用者：`preprocess.go` 负责写入；`logical_plan_builder.go`、`point_get_plan.go`、`optimizer.go`、`plan_cache.go` 和 `plan_cache_utils.go` 负责传播或读取。这些文件说明此数据在 TiDB 规划阶段的设计位置。

RustCodeGraph 将目标文件列为被 18 个文件使用，并能确认 `NodeW::new -> Context::new`、Go 风格门面到 snake_case 方法、`PlanBuilder::Build -> BuildNodeRef` 等内部边；但对 `Context`、`new` 等同名符号的精确 callers/callees 查询出现歧义，因此跨文件调用结论以限定路径的 `rg` 结果和源码节点为准。

## 错误处理与边界

所有公开函数均不返回业务错误。缺失绑定由 `get_table_name -> Option<Rc<TableNameW>>` 表达；这比 Go 的 map miss 返回 `nil` 等价且显式。Go 注释说明正常情况下预处理应先登记绑定，若不存在，错误应已在预处理阶段返回；Rust 本文件没有实施这一前置条件，调用方仍必须处理 `None` 或证明预处理已成功。

`RefCell` 把借用规则推迟到运行时：当一个 `RefMut` 仍存活时再次借用或可变借用同一映射会 panic。尤其不要在持有 `get_table_names()` 返回值的表达式范围内调用 `add_table_name` 或 `get_table_name`。本文件没有把此 panic 转换为 `Result`。

其他边界包括：

- `add_table_name` 不拒绝元数据为空或逻辑上不匹配的包装。
- `clone_with_new_node` 不验证新旧 AST 是否共享相同的表名节点。
- `get_table_names` 暴露完整可变映射，绕过了 `add_table_name` 的键生成规则。
- 裸指针只用于哈希和相等比较，本文件不解引用它；安全性依赖值持有相应 `Rc` 的不变量。

## 并发与资源生命周期

`Rc` 和 `RefCell` 都不是线程安全原语，因此 `Context`、`NodeW`、`TableNameW` 不适合跨线程共享，也不会自动实现 `Send`/`Sync`。文件内没有锁、通道、任务、事务、文件句柄或异步生命周期；适用模型是单线程 AST 遍历期间的共享可变状态。

生命周期由引用计数管理：`NodeW`/`Context` 克隆延长共享映射生命周期，映射中的 `Rc<TableNameW>` 再延长 AST 表名与元数据生命周期。清除映射或释放最后一个 `Context` 后，条目引用计数随之下降。不存在显式清理函数；若调用方长期保留上下文，全部绑定也会被长期保留。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/resolve/resolve.go`。字段和行为基本逐项对应：

| Go | Rust | 语义说明 |
| --- | --- | --- |
| `*ast.TableName` | `Rc<ast::TableName>` | 保留可共享的 AST 对象身份 |
| `*model.DBInfo` / `*model.TableInfo` | `Option<Rc<...>>` | Go 的 `nil` 对应 Rust 的 `None` |
| `ast.Node` | `ast::NodeRef` | 通用 AST 节点引用 |
| `*Context` | 可克隆的 `Context`，内部 `Rc<RefCell<_>>` | 克隆后共享同一内部映射 |
| `map[*ast.TableName]*TableNameW` | `HashMap<*const ast::TableName, Rc<TableNameW>>` | 都按对象身份键控，而非值相等 |
| map miss 返回 `nil` | `get_table_name` 返回 `None` | 缺失绑定的语言惯用表达不同 |
| `GetTableNames` 返回内部 map | 返回 `RefMut<TableNameMap>` | 都允许直接修改；Rust 额外受动态借用检查约束 |

Rust 额外提供 snake_case API，同时保留 Go 风格名称以降低迁移接线成本。返回 `RefMut` 比 Go map 引用更严格：借用期间重入访问会 panic。另一方面，Rust 使用 `Option` 明示可空元数据和查询失败。

迁移测试 `pkg/planner/core/resolve/migration_aster_unit_test.rs` 验证了指针身份、覆盖、内部 map 可变性、共享/独立上下文和 Go 风格门面。Go 侧没有同目录专门的 `resolve_test.go`；其行为由 `pkg/planner/core/preprocess_test.go`、`planbuilder_test.go`、`logical_plans_test.go`、`physical_plan_test.go` 等规划器测试间接覆盖。

## 扩展指南

- 新增解析结果类别时，应在 `Context` 增加独立、类型明确的映射和访问方法；先确认键需要 AST 身份还是稳定业务 ID，不能默认复用裸指针方案。
- 修改表名键控规则时，必须同步 `add_table_name`、`get_table_name`、`get_table_names` 的契约，并扩展 `migration_aster_unit_test.rs`，覆盖同值不同对象、重复插入和删除/清空行为。
- 新增 `NodeW` 改写方法时，应明确上下文是共享、清空还是重建。若新节点不是从原 AST 派生，直接复用旧 `Context` 可能产生无法命中的旧键或错误绑定。
- 若需要跨线程规划，不能仅给类型强加 `Send`/`Sync`；应整体评估将 `Rc<RefCell<_>>` 迁移为 `Arc` 加锁结构的代价、借用粒度和 AST/元数据类型的线程安全性。
- 若要收紧不变量，优先减少 `get_table_names` 的可变暴露或提供受控的遍历/删除 API；但这会改变 Go 兼容语义，必须同时审查所有 Go/Rust 调用者。
- 生产接线应从 Rust 预处理入口开始：创建 `NodeW`，解析时使用同一 `Rc<TableName>` 填充 `Context`，再让计划构建和点查路径消费它。不能以测试中的手工填充替代真实 InfoSchema 解析。
- 测试逻辑继续放在独立的 `migration_aster_unit_test.rs`，不要内嵌到 `resolve.rs`。兼容性风险主要是对象身份、共享上下文和 Go 的可变 map 语义；性能风险主要是 `Rc` 克隆、哈希表增长与不受控的上下文保留。

## 验证依据

- 目标源码：`pkg/planner/core/resolve/resolve.rs`，共 161 行；RustCodeGraph `node --file` 核对了全部类型、方法、工厂和无条件编译结构。
- crate 边界：`pkg/planner/core/resolve/Cargo.toml` 与 `pkg/planner/core/resolve/lib.rs`；确认包名、两项直接依赖、模块和公开再导出。
- Rust 调用证据：RustCodeGraph 节点读取 `pkg/planner/core/planbuilder_runtime.rs::PlanBuilder::Build`、`pkg/planner/optimize.rs::{buildPlan,optimize}`、`pkg/planner/core/optimizer_runtime.rs::OptimizeAstNodeFn`；限定非测试 `.rs` 的引用搜索确认类型已用于接口，但本文件的构造/填充 API尚无外部生产调用。
- Go 对照：`pkg/planner/core/resolve/resolve.go`；引用搜索进一步核对 `pkg/planner/core/preprocess.go`、`logical_plan_builder.go`、`point_get_plan.go`、`optimizer.go`、`plan_cache.go` 的写入、读取与传播位置。
- 独立 Rust 测试：`pkg/planner/core/resolve/migration_aster_unit_test.rs`，覆盖对象身份、重复覆盖、可变内部 map、共享与独立上下文、兼容入口；相邻 `result.rs` 只补充同 crate 的列绑定结构，不改变本文对象的行为。
- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的六个 Rust/Go 文件均在索引中。自然语言 `explore` 命中噪声较多，精确 callers/callees 又受同名符号歧义影响，故跨文件结论使用路径限定搜索复核，并在本文明确区分已接入类型与未验证的生产构造链。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有十一个固定二级标题，并人工复查没有把 Go 行为误称为 Rust 已支持行为。
