# `pkg/ddl/constraint.rs`

## 文件定位

[`constraint.rs`](constraint.rs) 属于 `astersql-ddl` crate；[`pkg/ddl/Cargo.toml`](Cargo.toml) 的 `[lib]` 将 crate 根设为 `lib.rs`，而 [`lib.rs`](lib.rs) 通过 `pub mod constraint` 公开本模块。文件提供一套针对 CHECK 约束的内存元数据模型和单步状态推进函数，并复用 [`column.rs`](column.rs) 的 `SchemaState`、`TableInfo`，以及 [`generated_column.rs`](generated_column.rs) 的简化表达式树。

必须区分“模块已导出”和“已接入完整 DDL 主链”：截至本次分析，全仓 Rust 使用点中，`advance_add_check_constraint` 只有 [`constraint_test.rs`](constraint_test.rs) 的回滚测试直接调用；`advance_drop_check_constraint`、`advance_alter_check_constraint` 及两个列依赖检查函数没有目标文件外的调用者。真实 Rust 建表路径使用的是 `astersql_meta_model::ConstraintInfo`（例如 [`create_table.rs`](create_table.rs)），不是本文件自定义的 `ConstraintInfo`。因此，本文件当前是可复用的局部状态机实现，但不能据此断言 SQL 入口、持久化 job、owner worker、schema 同步或 DML 检查已经通过它接线。

## 核心职责

本文件围绕以下职责组织：

1. 用 `ConstraintInfo` 和 `ConstraintTableState` 表达单表 CHECK 约束、最大约束 ID 及中间 schema 状态。
2. 校验约束名和依赖列，提取表达式引用列，并为匿名约束生成不冲突的 `<table>_chk_<n>` 名称。
3. 由 `advance_add_check_constraint`、`advance_drop_check_constraint`、`advance_alter_check_constraint` 每次推进一个状态，返回 `ConstraintOutcome`，由调用者负责重复驱动、持久化和失败后的后续动作。
4. 在删列、改列名前检查 CHECK 约束依赖，分别实现“多列约束阻止删列”和“任意引用阻止改名”的规则。

它不负责解析 SQL、把表达式恢复成 SQL 文本、扫描真实表数据、提交/持久化 DDL job、等待集群 schema 同步或在 DML 路径求值。`remaining_records_valid` 是外部存量扫描结果的布尔输入，而不是扫描实现。

## 主要符号

- `MAX_CONSTRAINT_IDENTIFIER_LENGTH: usize = 64`：约束名上限。`check_constraint_name` 对 `to_ascii_lowercase()` 后的 UTF-8 **字节长度**应用该上限。
- `ConstraintInfo`：模块自己的约束记录，包含单调 ID、规范化名称、表名、依赖列、表达式文本、`enforced`、`in_column` 和 `SchemaState`。它与 `pkg/meta/model/table.rs` 中供真实表元数据使用的 `ConstraintInfo` 是不同类型。
- `ConstraintTableState`：某张表的约束向量和 `max_constraint_id`。向量包含中间状态对象；查找名称时状态机使用 ASCII 大小写无关比较。
- `ConstraintError`：列举名称过长、重名、未找到、未知列、数据违反约束、非法状态和列仍被约束依赖七类领域错误；本文件没有实现到 TiDB 错误码或 SQL 文本的映射。
- `check_constraint_name`：只做名称长度检查，不做空名、保留字或跨表唯一性校验。
- `find_dependent_columns`：调用 `generated_column::find_column_names_in_expr` 递归收集列引用，再转为 `BTreeSet`，因而结果小写、去重且有序。
- `build_constraint_info`：构造 ID 为 0 的约束；名称和传入依赖列转 ASCII 小写，表达式文本原样保存。ID 延迟到添加状态机首次落入表状态时分配。
- `set_names_for_constraints`：只处理空名约束，按调用期间共享的 `existing_names` 跳过冲突编号。计数器对整个切片递增。
- `validate_dependencies`：私有前置检查，要求每个依赖名能精确匹配 `TableInfo.columns` 中处于 `Public` 的列。
- `ConstraintOutcome`：报告本步结束后的状态、版本、是否完成以及是否以回滚结束；它不携带持久化事务或 job 信息。
- `advance_add_check_constraint`：添加或回滚添加约束的单步状态机。
- `advance_drop_check_constraint`：删除约束的两步状态机。
- `advance_alter_check_constraint`：切换 `ENFORCED` 属性的状态机。
- `ensure_column_droppable_with_check_constraint`：仅在被多列约束引用时拒绝删列。
- `ensure_column_renameable_with_check_constraint`：被任何约束引用即拒绝改名。

## 执行流程

添加约束由 `advance_add_check_constraint` 驱动：

1. 若 `rolling_back` 为真，按名称仅移除目标约束，schema 版本加一，返回 `None / finished / rollback_done`。[`constraint_test.rs`](constraint_test.rs) 的 `rollback_add_constraint_removes_only_the_named_constraint_like_go` 验证同表其他约束保留。
2. 正常路径先按名称搜索已有记录。已有 `Public` 记录报 `ConstraintExists`；已有中间态记录继续推进；首次进入则校验依赖列、补匿名名称、递增 `max_constraint_id`、写回 `incoming.id` 并克隆进 `state.constraints`。
3. `NOT ENFORCED` 约束直接进入 `Public` 并完成。强制约束每次调用推进一阶：`None -> WriteOnly -> WriteReorganization -> Public`；最后一阶仅在 `remaining_records_valid` 为真时通过。
4. 每个实际状态变化和回滚都令 `schema_version += 1`；失败不递增版本。

删除约束由 `advance_drop_check_constraint` 驱动：按名称找不到时报错；`Public` 第一次降至 `WriteOnly`，第二次从向量移除并返回 `None / finished`。其他起始状态报 `InvalidState`。这表达了中间态仍需约束新写入的语义，但本文件本身不执行写入检查。

修改约束由 `advance_alter_check_constraint` 驱动：回滚时把 `enforced` 恢复为目标值的反值并回到 `Public`；目标值已满足且状态为 `Public` 时是零版本变化的幂等完成；切到 `NOT ENFORCED` 一步完成；切到 `ENFORCED` 按 `Public -> WriteReorganization -> WriteOnly -> Public` 推进，并在最后一步检查 `remaining_records_valid`。

列操作辅助函数不改变状态。删列检查只遍历依赖列数大于 1 的约束；单列约束预期随列一起删除。改名检查对任意依赖命中立即返回 `ColumnNeededByConstraint`，因为本文件没有重写表达式文本的能力。

## 数据与状态

所有状态都由调用者持有并通过 `&mut` 原地修改，没有全局单例或隐藏缓存。`ConstraintTableState.max_constraint_id` 只增不减，即使约束随后被删除也不回收 ID。`constraints` 使用 `Vec` 保留插入顺序；查找遇到第一个同名项即停止，因此调用者应维持名称唯一不变量。

规范化规则并不完全统一：构造函数把名称和依赖列转 ASCII 小写，名称查找使用 `eq_ignore_ascii_case`，但 `validate_dependencies` 及两个列操作检查使用字符串精确相等。这意味着绕过 `build_constraint_info` 手工构造含大写依赖名的值，可能与小写表列不匹配。`set_names_for_constraints` 也假定表名和已有名称集合已按调用约定规范化。

状态与版本的不变量是“成功改变一步才加一”。添加/删除/修改完成状态分别是 `Public`、`None`、`Public`。幂等 ALTER 不增加版本。错误可能保留先前调用已经提交到内存的中间状态：例如添加约束的存量校验失败时记录仍在 `WriteReorganization`，启用 ENFORCED 的校验失败时仍为 `WriteOnly` 且 `enforced == true`；调用者必须显式再次以回滚模式驱动或采用自己的恢复流程。

## 依赖与调用关系

直接下游依赖只有标准库集合与同 crate 模块：

- `std::collections::{BTreeSet, HashSet}`：分别保证依赖列输出有序、匿名名称去重。
- `crate::column::{SchemaState, TableInfo}`：提供共享的简化 schema 状态和表/列视图；`TableInfo` 本身没有 CHECK 约束字段，所以本模块另持有 `ConstraintTableState`。
- `crate::generated_column::{ExpressionNode, find_column_names_in_expr}`：提供简化 AST 和递归列名收集。

RustCodeGraph 给出的内部边包括 `advance_add_check_constraint -> validate_dependencies`、`advance_add_check_constraint -> set_names_for_constraints`、`build_constraint_info -> check_constraint_name` 和 `find_dependent_columns -> find_column_names_in_expr`。图及 `rg` 使用搜索均未发现三个 `advance_*` 函数接入 `pkg/ddl` 的 job worker；仅 `advance_add_check_constraint <- rollback_add_constraint_removes_only_the_named_constraint_like_go` 是直接测试调用边。

真实 SQL/DML CHECK 约束的 Rust 元数据及求值还分布在 [`pkg/meta/model/table.rs`](../meta/model/table.rs)、[`pkg/table/constraint.rs`](../table/constraint.rs) 和 [`pkg/ddl/create_table.rs`](create_table.rs)。扩展本文件时不能把这些同名类型自动视为互通；需要显式转换或统一模型，并验证真实 DDL 调度接线。

## 错误处理与边界

所有可失败入口返回 `Result<_, ConstraintError>`，错误不带源错误链。主要边界如下：

- 名称上限按 Rust 字符串字节数而非 Unicode 字符数计算；`to_ascii_lowercase` 不执行完整 Unicode 大小写折叠。
- 匿名约束在 `build_constraint_info` 中允许空名，直到首次添加时才生成名称；直接调用 `check_constraint_name("")` 也会成功。
- 依赖检查只确认列存在且为 `Public`，不解析 `expression` 与 `columns` 是否一致，也不验证表达式是否受支持或确定性。
- `remaining_records_valid` 由调用者保证真实性；本文件无法防止伪造“校验通过”。
- 添加回滚无论目标是否存在都会加版本并报告完成；它只按名称删除目标，符合对应 Go 回滚的局部语义。
- ALTER 回滚通过 `!enforced` 推导旧值，只适用于布尔属性切换这一约定；调用者传错目标值会恢复到错误值。
- 删列和改名检查区分大小写，并检查所有状态的约束；是否只应考虑某些状态由上层保证。
- 本文件没有取消、重试、事务原子性、错误码映射或 job 状态；这些都不能由 `ConstraintOutcome.finished` 替代。

## 并发与资源生命周期

本模块没有锁、原子变量、channel、异步任务、session pool、事务或 I/O。所有修改均发生在调用者独占借用的 `&mut ConstraintTableState`、`&mut ConstraintInfo` 和 `&mut i64` 上，Rust 借用规则只保证单次调用期间没有并发可变访问，并不提供跨调用持久性或多节点同步。

一个完整操作通常跨多次调用存活：调用者应在每一步之后原子持久化约束状态与 schema 版本，并在适当的 schema 同步屏障后再调用下一步。当前 Rust 调用图没有证明存在这样的驱动器。相比之下，Go 实现在 worker/job 上下文中通过 `updateVersionAndTableInfo*` 写元数据并由 DDL 框架负责重试和完成通知；Go 的 `verifyRemainRecordsForCheckConstraint` 还从 worker session pool 借用 session，执行内部 SQL 后用 `defer` 归还，这些资源生命周期均未移植到本文件。

## 与 Go 版本的对应关系

直接对照文件是 [`constraint.go`](constraint.go)，回归行为见 [`constraint_test.go`](constraint_test.go)。主要映射如下：

| Rust | Go | 对齐情况与差异 |
| --- | --- | --- |
| `advance_add_check_constraint` | `(*worker).onAddCheckConstraint`、`checkAddCheckConstraint` | 核心状态顺序、非强制快速公开、重名/未知列检查及按名回滚意图相近；Rust 将元数据、扫描结果和版本抽象为内存参数，未包含 job 取消/回滚状态、持久化与 schema sync。 |
| `advance_drop_check_constraint` | `onDropCheckConstraint`、`checkDropCheckConstraint` | 都是 `Public -> WriteOnly -> 移除`；Go 更新 `model.TableInfo` 并完成 job，Rust 只修改本地向量。 |
| `advance_alter_check_constraint` | `(*worker).onAlterCheckConstraint`、`checkAlterCheckConstraint` | ENFORCED 的状态顺序和幂等路径对应；Rust 以布尔值代替真实存量扫描，并把回滚压缩进同一函数。 |
| `build_constraint_info` | `buildConstraintInfo` | 字段意图对应；Go 从 AST 恢复规范 SQL 表达式，Rust 接受现成字符串，且使用独立类型。 |
| `check_constraint_name` | `checkTooLongConstraint` | 都检查 64 字节下限语义；Go 返回 TiDB `ErrTooLongIdent`，Rust 返回领域枚举。 |
| `find_dependent_columns` | `findDependentColsInExpr` | 都去重并规范列名；Rust 额外用 `BTreeSet` 固定迭代顺序，并只支持简化 `ExpressionNode`。 |
| `set_names_for_constraints` | `setNameForConstraintInfo` | 都生成 `<table>_chk_<n>` 并跳过冲突。 |
| 两个 `ensure_column_*` | `IsColumnDroppableWithCheckConstraint`、`IsColumnRenameableWithCheckConstraint` | 单列/多列删除规则及重命名规则对应；Go 使用 `ast.CIStr.L` 做规范化比较并返回 TiDB 错误。 |

Go 中 `verifyRemainRecordsForCheckConstraint` 会执行 `select 1 ... where not <expr> limit 1`，用真实行决定是否违反约束；Rust 仅消费 `remaining_records_valid`。Go 测试使用 failpoint 观察 `WriteOnly`、`WriteReorganization`、`Public` 各阶段的写入行为和回滚，Rust [`constraint_test.rs`](constraint_test.rs) 多数测试采用独立的简化 `Table/CheckConstraint` 测试模型，只有最后一个测试直接覆盖生产状态机函数。因此两套测试的意图相近，但 Rust 当前对生产 API 的直接覆盖明显不完整。

## 扩展指南

若只扩展局部状态机，应在本文件修改最接近的 `advance_*` 或校验辅助函数，并把直接单元测试放在独立的 [`constraint_test.rs`](constraint_test.rs)，不要把测试内嵌到生产源文件。至少覆盖成功的每个状态边、版本是否恰好递增一次、非法状态不变、失败后的中间状态、大小写规则以及按名回滚不误删其他约束。

若目标是让 CHECK 约束在真实 Rust SQL 主链可用，不能只增加本文件分支。需要先决定是复用 `pkg/meta/model/table.rs::ConstraintInfo` 还是提供显式转换，再接入 job 参数解码、表元数据持久化、worker 重试/回滚、schema 版本同步、DML 可写约束缓存和真实存量行扫描。相应测试应同时验证独立状态机与 SQL 可见行为，并与 Go `constraint_test.go` 的状态窗口意图保持一致。

兼容性风险集中在状态顺序、错误类型/错误码、大小写与 Unicode 名称、自动命名冲突、旧元数据反序列化和回滚幂等性；性能风险集中在真实存量扫描及约束表达式求值，本文件当前的 `Vec` 线性查找只适合约束数量较小的表级元数据。任何并发接线都应把“状态和 schema 版本同一事务提交”作为不变量，而不是在本模块中添加进程内锁来模拟分布式一致性。

## 验证依据

- 源码全量阅读：[`constraint.rs`](constraint.rs)（443 行），并核对 [`column.rs`](column.rs) 的 `SchemaState`/`TableInfo`、[`generated_column.rs`](generated_column.rs) 的 `ExpressionNode`/`find_column_names_in_expr`、[`lib.rs`](lib.rs) 的模块导出。
- crate 边界：[`Cargo.toml`](Cargo.toml) 声明 package `astersql-ddl`、`[lib] path = "lib.rs"`；目标文件自身只使用标准库和同 crate 模块。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node` 查询确认三个 `advance_*`、两个列依赖函数、`build_constraint_info`、`find_dependent_columns`、`set_names_for_constraints` 与 `validate_dependencies` 的源码和内部调用边。`advance_add_check_constraint` 的已索引直接调用者只有 Rust 回滚测试；另外两个状态推进函数没有显示外部调用者。
- 全仓使用核对：`rg` 搜索三个 `advance_*` 和本文件自定义类型，确认生产 Rust 路径未调用它们；真实建表代码使用 `astersql_meta_model::ConstraintInfo`。
- Go 对照：全量阅读 [`constraint.go`](constraint.go)（435 行）和 [`constraint_test.go`](constraint_test.go)（247 行），核对 job 状态、真实数据验证、session pool、元数据版本更新以及各状态写入语义。
- Rust 测试：全量阅读 [`constraint_test.rs`](constraint_test.rs)（313 行）；其简化模型覆盖状态下的写入意图，直接生产 API 测试覆盖添加回滚只删除命名目标。
- 本任务只新增文档，按总计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文明确标示了“未接入真实 DDL 主链”的验证边界。
