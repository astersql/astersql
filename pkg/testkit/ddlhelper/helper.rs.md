# `pkg/testkit/ddlhelper/helper.rs`

## 文件定位

本文件是 `astersql-testkit-ddlhelper` crate 的唯一业务实现文件，为测试代码提供“从已解析的 `CREATE TABLE` AST 构造 `model::TableInfo`”的窄门面。crate 根 `pkg/testkit/ddlhelper/lib.rs` 将这里的 `BuildTableInfoFromASTForTest` 重新导出；`pkg/testkit/ddlhelper/Cargo.toml` 则声明该 crate 属于 Go 包 `pkg/testkit/ddlhelper` 的 Rust 移植，并依赖 DDL、metabuild、meta model、parser 及 parser AST crate。

它位于测试辅助层，而不是线上 SQL DDL 执行链。函数只构造并校验尚未分配持久化标识的表元数据，不提交 DDL job，不更新 schema version，也不触发 owner、worker、回填或 schema sync。实际构造与校验逻辑位于 `pkg/ddl/create_table.rs` 的 `BuildTableInfoFromAST` 及其下游。

## 核心职责

- 接受借用的 `ast::CreateTableStmt`，避免复制或取得 AST 所有权。
- 创建不带自定义 option 的 `metabuild::Context<(), Infallible>`，从而使用 metabuild 的默认配置和默认静态表达式上下文。
- 把 AST 与上下文转交给正式入口 `ddl::BuildTableInfoFromAST`，返回完整的 `model::TableInfo` 或 parser 错误。
- 保留 Go 辅助函数的关键契约：此阶段不分配 `TableInfo.ID` 和各 partition definition 的 `ID`，这些值保持未初始化状态（当前测试断言为 `0`）。

本文件不自行解释列、索引、分区或表选项；这些语义由 DDL builder 统一实现，避免测试辅助路径与正式构造路径分叉。

## 主要符号

### `BuildTableInfoFromASTForTest`

```rust
pub fn BuildTableInfoFromASTForTest(
    s: &ast::CreateTableStmt,
) -> Result<model::TableInfo, parser::errors::Error>
```

这是文件中唯一的函数和公开 API；没有模块级常量、自定义类型、trait、`impl` 或条件编译项。它在 `lib.rs` 中通过 `pub use helper::BuildTableInfoFromASTForTest` 暴露给 crate 使用者。

参数 `s` 必须已经是 parser 产生或符合 parser AST 约束的 `CREATE TABLE` 节点。成功值是独立拥有的 `TableInfo`；失败值沿用 parser 的错误类型，与 `ddl::BuildTableInfoFromAST` 的 `BuildResult` 错误保持一致。

### 局部 `context`

`metabuild::NewContext::<(), Infallible>(Vec::new())` 创建函数调用期间使用的局部上下文。`()` 表示这里没有实际外部上下文对象，`Infallible` 表示该空依赖形状本身没有可产生的自定义错误；空 option 列表意味着不覆盖 metabuild 默认值。

## 执行流程

1. 调用者先把 SQL 解析为 `ast::CreateTableStmt`，再把该节点的共享引用传给 `BuildTableInfoFromASTForTest`。
2. 辅助函数用空 option 列表创建 `metabuild::Context<(), Infallible>`。根据 `pkg/meta/metabuild/context.rs::NewContext`，该构造过程先填入默认开关和默认数值；没有显式表达式上下文时，还会安装空 option 的静态 `ExprContext`。
3. 辅助函数调用 `pkg/ddl/create_table.rs::BuildTableInfoFromAST`。该入口固定使用默认数据库字符集 `utf8mb4`、空数据库 collation 和无 placement policy 引用。
4. `BuildTableInfoFromAST` 进入 `build_table_info_with_check`：先由 `BuildTableInfoWithStmt` 构造表元数据，再依次运行 `check_table_info_valid_with_stmt` 和 `check_table_info_valid_extra`。
5. 三步均成功时，返回拥有所有权的 `TableInfo`；任一步失败时，错误通过 `?` 和本辅助函数的直接返回原样向上传播。

这条路径是 metadata-only fast path，不包含持久化、schema state transition、reorg/backfill、取消/回滚或 delete-range 生命周期。

## 数据与状态

- 输入状态：只读的 `CreateTableStmt`，本文件不修改 AST。
- 临时状态：栈上局部变量 `context`。其默认配置来自 `metabuild::NewContext`，包括 clustered-index 模式、auto-increment/generated-column 开关、shard row ID bits、pre-split regions、错误抑制开关以及默认表达式上下文。
- 输出状态：拥有所有权的 `model::TableInfo`，包含正式 DDL builder 从 AST 派生的列、索引、表注释、分区等结构。
- 标识不变量：辅助函数没有 ID allocator，也没有存储事务；`TableInfo.ID` 与 partition definition ID 因此保持未初始化。`helper_aster_unit_test.rs` 分别验证表 ID 为 `0` 和所有分区 ID 为 `0`。
- 全局状态：本文件没有 `static`、缓存、注册表或环境变量读写，不保存跨调用状态。

## 依赖与调用关系

上游关系：

- `pkg/testkit/ddlhelper/lib.rs` 声明私有 `helper` 模块并重新导出公开函数。
- RustCodeGraph 将直接 Rust 调用定位到 `pkg/testkit/ddlhelper/helper_aster_unit_test.rs` 的 `helper_builds_the_same_table_metadata_as_the_ddl_entry_point` 与 `helper_leaves_partition_ids_uninitialized`。
- 当前仓库的普通 Rust 源码未发现其他调用者；因此这个 Rust 门面目前主要由自身回归测试覆盖。Go 对应函数仍被 `pkg/planner/core/rule/rule_partition_pruning_test.go` 的两个测试辅助调用点使用。

下游关系：

- `std::convert::Infallible`：表达空上下文依赖不会产生自定义错误。
- `astersql_meta_metabuild::NewContext`：提供 DDL 元数据构造配置与表达式上下文。
- `astersql_ddl::BuildTableInfoFromAST`：执行真实的 TableInfo 构造和合法性校验。
- `astersql_parser_ast::CreateTableStmt`：定义输入 AST。
- `astersql_meta_model::TableInfo`：定义成功输出。
- `astersql_parser::errors::Error`：定义失败输出。

Cargo 依赖全部是仓库内路径依赖；本文件没有 feature gate。工作区根 `Cargo.toml` 将 `pkg/testkit/ddlhelper` 列为 member，并以 `facade_testkit_ddlhelper` 别名提供工作区依赖入口。

## 错误处理与边界

本文件不捕获、不包装也不降级错误。`ddl::BuildTableInfoFromAST` 返回错误时，辅助函数立即把同一错误返回给调用者。具体失败边界由正式 builder 决定，包括 AST 到表元数据的构造失败，以及 statement-aware 和额外元数据合法性检查失败。

需要注意的边界：

- 函数只接受已经下转为 `CreateTableStmt` 的 AST；SQL 解析、statement 类型判断和 downcast 失败不属于本函数，现有测试在调用前完成这些步骤。
- 空 option 上下文不会提供调用方定制的 infoschema、表达式上下文或 metabuild 开关；需要这些环境依赖的场景不应假设本 helper 等同于真实 session 上下文。
- 默认字符集/collation/placement-policy 参数由下游 `BuildTableInfoFromAST` 固定；若测试要验证数据库级默认值或显式 policy 上下文，应调用更合适的 DDL 构造入口，而不是在此处暗中增加环境状态。
- 未初始化 ID 是有意契约，不应在 helper 内补分配。需要唯一持久化 ID 的测试必须显式使用相应 allocator/DDL 执行路径。

## 并发与资源生命周期

该函数是同步、无异步任务的纯构造式门面。每次调用都会创建独立的 metabuild 上下文并返回独立的 `TableInfo`；输入仅被不可变借用。文件内没有锁、原子变量、channel、线程、runtime handle、文件句柄、网络连接或存储事务。

因此它没有显式清理阶段：局部 context 在返回时按 Rust 所有权规则释放，错误路径也不持有待回滚资源。并发安全最终取决于调用者提供的 AST 引用满足 Rust 的类型约束以及下游 builder 的实现；本 helper 自身不共享可变状态，也不提供额外同步保证。

## 与 Go 版本的对应关系

`pkg/testkit/ddlhelper/helper.go::BuildTableInfoFromASTForTest` 的实现是：以 `metabuild.NewContext()` 创建默认上下文，再直接调用 `ddl.BuildTableInfoFromAST`。Rust 版本保持相同的两步转发形状：

- Go 的空可变参数对应 Rust 的 `Vec::new()`。
- Go 的 `*ast.CreateTableStmt` 对应 Rust 的 `&ast::CreateTableStmt`。
- Go 的 `*model.TableInfo` 对应 Rust 按值返回的 `model::TableInfo`，由调用者获得所有权。
- Go 的 `error` 对应 Rust 的 `parser::errors::Error`。
- 两个版本都不在 helper 中分配 TableID 或 PartitionID。

Rust 的显式泛型 `::<(), Infallible>` 是对 Go 接口/错误动态形状的静态表达，并非额外业务分支。`helper_builds_the_same_table_metadata_as_the_ddl_entry_point` 验证 helper 与 Rust DDL 正式入口在表名、列、索引、注释等代表性字段上一致；`helper_leaves_partition_ids_uninitialized` 则覆盖 Go 注释明确承诺的分区 ID 语义。

## 扩展指南

- 若 DDL builder 的签名或错误类型变化，应优先保持本函数为薄转发层，同步修改 `BuildTableInfoFromASTForTest` 的参数/返回类型及 `pkg/testkit/ddlhelper/helper_aster_unit_test.rs`，不要在 helper 中复制构造逻辑。
- 若 Go helper 新增 option、上下文依赖或 ID 语义，应同时核对 `helper.go`、`metabuild::NewContext` 和 Rust helper；添加独立测试覆盖新增分支。Rust 单元测试继续放在 `helper_aster_unit_test.rs`，不要内嵌到生产源文件。
- 若新增消费者需要数据库级 charset/collation、infoschema 或 placement policy，应先确认是否应直接使用 DDL 的更完整入口。改变这个共享 helper 的默认上下文可能影响所有测试，并造成与 Go 版本不一致。
- 若未来接入真实 DDL job 执行，本 helper 的职责边界应保持为 AST → metadata；job 提交、ID 分配、schema state 和事务生命周期应由 DDL/session 测试设施负责。
- 兼容性风险主要是 Go/Rust 默认值或错误类型漂移；正确性风险集中在绕过正式校验或意外分配 ID；性能风险很低，但在高频测试中每次重建默认表达式上下文仍是固定成本，不应在没有证据时引入全局缓存。

## 验证依据

- 目标实现：`pkg/testkit/ddlhelper/helper.rs`，RustCodeGraph 文件节点确认共 39 行、仅一个公开函数。
- crate 边界：`pkg/testkit/ddlhelper/Cargo.toml` 与 `pkg/testkit/ddlhelper/lib.rs`；根 `Cargo.toml` 确认 workspace member 和 facade dependency。
- Go 对照：`pkg/testkit/ddlhelper/helper.go::BuildTableInfoFromASTForTest`。
- Rust 调用与行为测试：`pkg/testkit/ddlhelper/helper_aster_unit_test.rs::helper_builds_the_same_table_metadata_as_the_ddl_entry_point`、`helper_leaves_partition_ids_uninitialized`。
- Go 使用证据：`pkg/planner/core/rule/rule_partition_pruning_test.go` 中两处 `ddlhelper.BuildTableInfoFromASTForTest` 调用。
- 上下文构造：RustCodeGraph 节点 `pkg/meta/metabuild/context.rs::NewContext`，确认默认字段、option 应用顺序与缺省静态表达式上下文。
- DDL 下游：RustCodeGraph 节点 `create_table.rs::BuildTableInfoFromAST` 和 `create_table.rs::build_table_info_with_check`，确认默认 charset/collation/policy 参数以及“构造后执行两层校验”的调用链。
- DDL 架构边界：`docs/agents/ddl/README.md` 仅作为入口说明，并以以上源码和测试核验；本 helper 没有进入其中的持久化 job 执行链。
- 本任务是纯文档分析，未修改或运行 Rust 代码，也未运行 Cargo。交付前按任务指定命令验证文档恰有 11 个固定二级标题，并人工复核重要结论均可回溯到上述符号或文件。
