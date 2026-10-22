# `pkg/planner/core/preprocess.rs`

## 文件定位

位于 SQL AST 与计划构建之间，提供一套简化的 Rust 预处理模型：把语句、DDL 和少量表达式状态表示成 `PreprocessNode`，在构建计划前检查名称、列、索引、表选项、别名、锁表目标、CTE 引用和部分语句组合，并通过 `PreprocessorReturn` 返回过期读及 InfoSchema 版本信息。

## 核心职责

- `Preprocess` 创建 `preprocessor`，应用 `PreprocessOpt`，按 `Enter`、`Leave`、`ensureInfoSchema` 的顺序执行一次节点预处理，并返回第一项错误或 `PreprocessorReturn`。
- `PreprocessNode`、`AlterTableOperation` 及相关轻量数据结构把计划构建器的 `Statement`、表名、列、索引、约束和 DDL 操作转换为本模块可校验的输入。
- `Enter` 负责进入节点时的状态记录：用户变量读写集合、DDL/repair/import/analyze/sequence 标志以及 SELECT 锁上下文；`Leave` 负责按节点类型分派具体校验并清理 SELECT 的锁上下文。
- 列与索引辅助函数实现大小写不敏感的名称去重、`AUTO_INCREMENT` 类型/唯一性/首键约束、生成列约束、索引部件形状、索引类型和表引擎白名单检查。
- `TryAddExtraLimit` 在非受限 SQL 且 `select_limit != u64::MAX` 时，为尚无 `Limit` 的 `Statement::Select` 包装 `PlanKind::Limit`，并递归处理 `Statement::Explain`。
- `EraseLastSemicolonInSQL`、`bindableStmtType`、`isTableAliasDuplicate` 和 `tryLockMDLAndUpdateSchemaIfNecessary` 提供预处理周边的独立工具行为。

## 主要符号

- 类型与入口：`Result<T>`、`PreprocessOpt`、`Preprocess`、`preprocessor`、`PreprocessorReturn`。`PreprocessOpt` 是修改预处理器的闭包；`InPrepare`、`InTxnRetry`、`InitTxnContextProvider` 和 `WithPreprocessorReturn` 是现有选项构造器。
- 语句分类：`TypeInvalid`、`TypeSelect`、`TypeSetOpr`、`TypeDelete`、`TypeUpdate`、`TypeInsert`，以及 `bindableStmtType`。当前 Rust 映射只识别 `Statement::Select` 和 `Statement::Insert`，其余返回 `TypeInvalid`。
- 状态位：`inPrepare`、`inTxnRetry`、`inCreateOrDropTable`、`parentIsJoin`、`inRepairTable`、`inSequenceFunction`、`initTxnContextProvider`、`inImportInto`、`inAnalyze`，统一存放在 `preprocessor.flag: u64`。
- 输入模型：`PreprocessNode` 覆盖 `Statement`、数据库 DDL、建表/视图/索引、删表/序列、重命名、repair、alter、select、binding、show、execute、sequence、cast 和用户变量；`ColumnDef`/`ColumnOption`、`IndexOption`/`IndexPart`、`Constraint`、`TableOption`、`TableName`、`AlterTableOperation` 描述其载荷。
- 嵌套状态：`preprocessWith::UpdateCTEConsumerCount` 从内到外寻找同名 CTE 并只增加最近定义的消费次数；`lockSelectCtx` 记录锁子句目标与 FROM 实际使用表；`tableAliasInJoin` 字段保留别名栈形态，但当前文件没有对其进行压栈/弹栈操作。
- 核心方法：`preprocessor::{Enter, Leave, tableByName, checkCreateTableGrammar, checkCreateIndexGrammar, checkAlterTableGrammar, checkSelectGrammar, updateStateFromStaleReadProcessor, ensureInfoSchema, skipLockMDL}`。
- 独立校验器：`checkAutoIncrementOp`、`checkColumnOptions`、`checkIndexOptions`、`checkIndexSpecs`、`checkIndexInfo`、`checkTableEngine`、`checkColumn`、`checkContainDotColumn`、`checkObjectName`。
- 周边工具：`TryAddExtraLimit`、`EraseLastSemicolon`、`EraseLastSemicolonInSQL`、`aliasChecker`、`getTableRefsAlias`、`tryLockMDLAndUpdateSchemaIfNecessary`。

## 执行流程

1. 调用方构造一个 `PreprocessNode` 和已知表映射 `HashMap<String, TableInfo>`，可附带若干 `PreprocessOpt`。
2. `Preprocess` 用 `known_tables` 初始化其余字段为默认值，按顺序调用选项闭包。选项可以设置模式位，或用克隆值整体替换 `PreprocessorReturn`。
3. `Enter` 首先检查是否已有错误；若有则返回 `true` 表示应中止。否则它记录用户变量状态，立即检查数据库对象名，或为 DDL、repair、import、analyze、sequence、SELECT 设置标志和锁上下文。
4. `Preprocess` 在 `Enter` 后立即取走 `p.err`；若出现错误则返回，不运行 `Leave`。
5. `Leave` 根据节点种类分派校验。建表路径依次验证表名、非空列集、每列定义、大小写不敏感的重复列、自增约束、表约束、不支持的表选项和引擎；SELECT 路径验证别名、GROUP BY 位置、无效函数警告及锁目标是否出现在 FROM；其他 DDL 各走对应检查器。
6. SELECT 离开时无论 `checkSelectGrammar` 返回成功或失败，都会弹出一层 `lockSelectCtxStack` 并清除 `parentIsJoin`。其他进入阶段设置的 DDL/repair/import/analyze/sequence 标志在当前简化实现中没有统一的离开清理逻辑，因当前入口只处理单个抽象节点而非递归 AST。
7. `Leave` 用 `fail` 保留第一项 `BuilderError`；`Preprocess` 再次取走错误，成功时调用 `ensureInfoSchema`。若返回结构的版本仍为零，版本取 `known_tables` 中最大表 ID 的绝对值，空表集则为 `1`。
8. 独立的 `TryAddExtraLimit` 不经过 `Preprocess`：它克隆输入语句，给无现有 Limit 的 SELECT 增加一层 Limit；EXPLAIN 递归处理其内部语句；受限 SQL、无限制配置及其他语句保持原样。

## 数据与状态

`preprocessor` 是一次预处理调用内的可变状态容器，不使用全局状态。`err: Option<BuilderError>` 具有“首错优先”语义；`fail` 只在尚无错误时记录失败。`warnings` 当前只由 `checkSelectNoopFuncs` 写入，对 `get_lock`、`release_lock`、`sleep` 产生字符串警告，但 `PreprocessorReturn` 不携带这些警告，`Preprocess` 结束后也不会对外返回它们。

表查找由 `known_tables` 完成。`TableName::key` 生成小写 `schema.name`，`tableByName` 先按完整键、再按小写裸表名查找；不存在时产生 `table ... does not exist`。这不是 Go 版的权限感知 InfoSchema 查找：它不读取会话当前库、临时表覆盖或用户权限。

用户变量采用大小写不敏感状态机。赋值会把名称放入 `varsMutable` 并从 `varsReadonly` 移除；在 SELECT/UPDATE/INSERT/DELETE 中，未在本语句赋值的读取会进入 `varsReadonly`。独立测试证明“先读、后写、再读”最终仍保持 mutable 且不再 fold 为只读变量。

`PreprocessorReturn` 保存 `IsStaleness`、`LastSnapshotTS`、`InfoSchemaVersion` 和内部初始化位。`updateStateFromStaleReadProcessor(Some(ts))` 拒绝零时间戳并同时设置前三项过期读状态；但当前 `Preprocess` 分派没有调用该方法，调用方必须经其他接线显式使用它。

CTE 使用三组向量表达可见名、进入偏移和分层定义；消费计数只修改最内层同名定义。锁状态使用栈表达嵌套 SELECT，不过当前 `Preprocess` 只对一个 `PreprocessNode` 调用一次 `Enter`/`Leave`，没有递归遍历子节点。

## 依赖与调用关系

直接 Rust 依赖很小：`crate::planbuilder::{BuilderError, Statement, TableInfo}` 提供错误、语句和表元数据，`crate::task::{PlanKind, PlanNode}` 用于 Limit 包装，标准库 `HashMap`/`HashSet` 用于查找、去重和状态集合。虽然 `astersql-planner-core` 的 Cargo manifest 还声明 parser AST、InfoSchema、session variables 等大量 crate 依赖，本文件并未直接使用这些外部 crate，而是操作本 crate 的轻量模型。

内部调用主干为：

`Preprocess -> preprocessor::Enter -> recordUserVariable / check* / pushLockSelectCtx`

`Preprocess -> preprocessor::Leave -> checkCreateTableGrammar | checkCreateIndexGrammar | checkAlterTableGrammar | checkSelectGrammar | ...`

`checkCreateTableGrammar -> checkColumn -> checkColumnOptions / isInvalidDefaultValue`，并继续到 `checkAutoIncrement`、`checkConstraintGrammar`、`checkUnsupportedTableOptions`、`checkTableEngine`。索引路径由 `checkIndexInfo`、`checkIndexOptions`、`checkIndexSpecs` 共同完成。

RustCodeGraph 的文件关系列出 `pkg/planner/core/lib.rs`、`pkg/planner/core/preprocess_test.rs`、`pkg/planner/core/planbuilder_runtime.rs`、若干其他测试和 `pkg/util/stmtsummary/reader.rs`；逐符号搜索进一步确认，后几者没有调用本文件的 `Preprocess` 或 `TryAddExtraLimit`。`pkg/util/stmtsummary/reader.rs` 的 `stmtType` 是语句摘要字段函数，与 `preprocessor::stmtType` 同名但无调用关系。

与应用主链的关系必须从当前接线理解：`pkg/executor/compiler.rs::Compiler::compileInner` 确实在优化前调用 `CompilerDependencies::Preprocess`，但该 trait 方法签名接收真实 parser `ast::NodeRef` 并返回 `CompilerPreprocessResult`，并非本文件接收 `PreprocessNode` 的函数。

## 错误处理与边界

所有显式失败都封装为无类型码的 `BuilderError(String)`，调用者只能依赖成功/失败或消息文本；这与 Go 版大量使用可识别的 `plannererrors`、`dbterror`、`types` 错误不同。`Preprocess` 在进入和离开阶段之间都检查错误，确保首错直接返回。

主要边界包括：对象名不能为空、不能超过 64 个字符且不能含 NUL；CREATE TABLE 至少一列且列名大小写不敏感唯一；最多一个自增列，自增列必须为允许的数值类型且作为主键/唯一键第一部件；虚拟生成列不能作为主键；索引最多 16 个部件且部件必须在列名和表达式之间二选一；前缀长度不能为零；向量索引必须恰有一个表达式部件；锁子句目标必须在 FROM 集合中。

`checkIndexOptions` 对 columnar 模式要求明确的 `vector`/`inverted`/`fulltext` 类型并拒绝 invisible；非 columnar 模式拒绝这些类型。`IndexOption.global` 当前除结构保存外没有在该函数中检查；`checkReferInfoForTemporaryTable` 仅在被显式调用时拒绝临时表上的 global index。

`TryAddExtraLimit` 的第三个参数名是 `restricted_sql`：传 `true` 时不加 Limit。现有测试注释把第二次调用描述为 “for_update 场景”，但实际传入的是 `restricted_sql = true`；函数本身没有读取 `Statement::Select.for_update`。此外，Rust 版不处理 Go 版支持的 SHOW 与集合操作额外 Limit，也没有检查 SELECT INTO。

`EraseLastSemicolonInSQL` 只在分号是最后一个 Unicode 字符时移除一个分号，不裁剪空白；`"select 1;  "` 原样返回。`checkObjectName` 以 Rust 字符串字节长度执行 64 长度上限，需要扩展时应先确认与 MySQL 标识符字符计数语义是否一致。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁、事务句柄或 I/O。`preprocessor`、`lockSelectCtx`、CTE 栈、告警和集合都由调用栈独占；`WithPreprocessorReturn` 捕获并克隆输入值，也不共享可变引用。因此该实现本身没有跨线程同步要求。

生命周期的重点是栈和标志的成对变化。SELECT 的 `Enter` 压入锁上下文，`Leave` 在完成检查后弹出；CTE 消费层从向量尾部向前搜索。当前入口没有真实 AST 递归，若未来把它接到递归 visitor，必须保证每个压栈点在错误路径上也有对应清理，否则嵌套 SELECT/CTE 会污染外层状态。

`tryLockMDLAndUpdateSchemaIfNecessary` 的名称沿用 Go 意图，但当前没有实际加锁：它只验证表 ID 为正，在 `skip == false` 时提高传入的 schema version，然后克隆 `TableInfo`。因此它不持有或释放 MDL 资源，不能作为并发 DDL 隔离已经实现的证据。

## 与 Go 版本的对应关系

Rust 使用自定义 `PreprocessNode`，没有 session/权限/真实 resolve context，也不递归遍历；`PreprocessorReturn` 用数字版本代替 InfoSchema 对象；`TryAddExtraLimit` 仅覆盖 SELECT/EXPLAIN；`bindableStmtType` 未覆盖 Go 的 set operation、delete、update；错误身份也未移植。

当前 Rust 同名函数没有这些生产调用边，所以新增行为时不能只修改本文件并假定整条 Rust SQL 主链已经获得该行为。

## 扩展指南

- 新增节点类型时，先扩展 `PreprocessNode`，再明确逻辑属于进入态、离开态还是独立递归子节点；同步更新 `Enter`/`Leave`，并在 `pkg/planner/core/preprocess_test.rs` 增加独立回归测试，不要把测试内嵌到生产文件。
- 扩展列/索引/DDL 规则时，优先修改最窄的 `check*` 函数，再由复合检查器调用；同时与 `pkg/planner/core/preprocess.go` 的同名函数及 `pkg/planner/core/preprocess_test.go` 对照错误顺序、大小写、NULL 和 AST 重写语义。
- 若要把本模块接入生产编译路径，需要设计 `ast::NodeRef`/resolve context/InfoSchema 与 `PreprocessNode` 的转换或改为直接 visitor，并与 `CompilerDependencies::Preprocess` 的所有实现统一；这是跨文件接线工作，不能用增加一个调用点替代。
- 扩展 `TryAddExtraLimit` 时需补齐 Go 的 SHOW、set operation、SELECT INTO 和已有 Limit 分支，并验证克隆是否保持 AST/计划身份约束。当前参数 `restricted_sql` 与测试注释容易混淆，应以函数分支为准。
- 引入真实 MDL、事务或过期读状态时，要把获取、释放、错误回滚和 schema 刷新作为一个生命周期设计；当前 `tryLockMDLAndUpdateSchemaIfNecessary` 只是纯数据辅助，不能直接扩写成隐式全局锁操作。
- 兼容风险主要来自 Go 错误类型/优先级、标识符规则、临时表和权限隐藏；性能风险主要来自未来真实 AST 多次遍历、表查找和不必要克隆。任何生产接线应先建立针对调用主链的集成测试，再评估这些风险。
