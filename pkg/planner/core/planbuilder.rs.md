# `pkg/planner/core/planbuilder.rs`

## 文件定位

[`planbuilder.rs`](planbuilder.rs) 位于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 以 `lib.rs` 为 crate 根，`lib.rs` 通过 `pub mod planbuilder` 暴露本模块，并仅在测试配置下以 `#[path = "planbuilder_test.rs"]` 装入独立测试。该文件不负责 SQL 文本解析，也不是最终执行器；它接收已经类型化的 `Statement`、表/列元数据和表达式，构造 `BuiltPlan` 或辅助规划数据。

文件顶部已经标记 `// Copyright 2026 AsterSQL.`，且保留 PingCAP Apache License。实现使用 `crate::task` 的 `PlanNode`、`PlanKind`、`Expression`、`FieldType`、`TypeCode`，以及 `crate::find_best_task` 的 `AccessPath`、`IndexInfo`；因此它位于“语句/元数据输入”到“逻辑计划、管理命令及统计任务输出”的规划边界。

需要注意当前 Rust 边界与 Go 主实现并不完全相同：Go 的 `PlanBuilder.Build` 接收带 resolve context 的 AST 节点并依赖 session、infoschema、hint 与表达式重写器；本文件的 `Statement` 和 `BuiltPlan` 是较小的 typed 模型，`Statement::Select` 甚至直接携带已有 `PlanNode`。解析器驱动的完整 SELECT/CTE 路径由相邻的运行时规划模块承担，测试通过 `crate::main_test` 进入该路径。

## 核心职责

1. **语句分派**：`PlanBuilder::Build` 对 `Statement` 的全部变体进行穷尽匹配，将 SELECT、EXECUTE、SET、绑定、ADMIN、ANALYZE、SHOW、INSERT、数据导入、统计命令、DDL、TRACE/EXPLAIN 等路由到专用构建函数。
2. **构建 typed 计划**：以 `BuiltPlan::{Logical, Execute, Set, Binding, Prepare, Admin, Analyze, Show, Insert, Ddl, RenameTables, Explain, Trace, Command}` 表达规划结果；SHOW/ADMIN 辅助函数同时给出结果集 `Schema`。
3. **维护构建期状态**：在 `PlanBuilder` 中积累权限 `visitInfo`、优化标志、当前子句、查询块偏移、handle 列作用域、外层 CTE、锁定读标志、告警及全文/谓词匹配标记；`ResetForReuse` 清除一次构建留下的可变状态。
4. **落实边界规则**：校验 INSERT 列和值、生成列、索引/分区名、ANALYZE 选项、DDL job 参数、绑定 SQL、IMPORT INTO 列赋值与 NextGen SEM 存储 URI。
5. **生成访问与统计元数据**：构造/筛选 TiKV、TiFlash、索引访问路径；计算 handle 列、物理分区 ID、ANALYZE 列集合、持久化选项合并与 index/column tasks。
6. **收集权限需求**：LOAD/IMPORT、统计对象、RENAME TABLE、GRANT/REVOKE 辅助逻辑将所需静态或动态权限加入 `visitInfo`，实际授权判定留给后续阶段。

## 主要符号

- `Privilege`、`visitInfo`：描述静态/动态权限及库、表、列、错误文本和 grant 语义。`visitInfo::Equals` 使用结构相等。
- `clauseCode`、`clauseMsg`：保存表达式所处 SQL 子句及其报错文本；未知子句返回空字符串。
- `capFlagType`、`canExpandAST`、`renameView`，以及 `subQueryCtx*`：与 Go 命名对齐的能力/子查询上下文常量；本文件内未将所有常量接入 `PlanBuilder` 状态。
- `cteInfo`、`handleColHelper`：前者记录 CTE 构建标记；后者用 `Vec<HashMap<i64, Vec<usize>>>` 实现作用域栈，`popMap`/`popSelectOffset` 在栈不变量被破坏时 panic。
- `ColumnInfo`、`TableInfo`、`IndexMeta`、`PartitionInfo`、`SchemaColumn`、`Schema`：本模块使用的精简元数据和输出 schema。
- `Value`、`Statement`、`AdminStatement`、`ShowKind`、`InsertStatement`、`AnalyzeStatement`：构建入口的数据模型。`Value::UserVar` 和 `Value::Default` 保留规划期语义。
- `AnalyzeIndexTask`、`AnalyzeColumnsTask`：按物理表/分区生成的统计任务描述。
- `BuiltPlan`：本文件统一输出枚举。子计划通过 `Box<BuiltPlan>` 包装，逻辑关系计划通过 `PlanNode` 表达。
- `BuilderError`、`Result<T>`：字符串错误包装和模块内统一返回类型。
- `PlanBuilderOpt`、`PlanBuilderOptNoExecution`、`PlanBuilderOptAllowCastArray`、`NewPlanBuilder`：构造选项分别控制子查询预处理副作用和 CAST ARRAY 路径。
- `PlanBuilder`：核心有状态构建器；`Init`、`ResetForReuse`、`Build` 是主要生命周期/入口函数，`GetVisitInfo`、`GetOptFlag` 等暴露构建结果状态。
- 重要辅助族：访问路径 `getPossibleAccessPaths`/`getPathByIndexName`；ANALYZE `handleAnalyzeOptions`/`mergeAnalyzeOptionsWithResets`/`fillAnalyzeOptions`；schema `buildShowSchema`；导入 URI `checkNextGenS3PathWithSem`/`processNextGenS3PathWithSem`；DDL job `checkAlterDDLJobOptValue`。

## 执行流程

典型 typed 构建流程如下：

1. 调用方用 `NewPlanBuilder` 创建实例，构造选项通过 `PlanBuilderOpt::Apply` 写入开关；`Init` 会调用 `ResetForReuse`。
2. 调用 `PlanBuilder::Build(&Statement)`。匹配分支只在本层决定语句族，复杂规则委托给同名 `build*` 方法。
3. SELECT 分支设置 `isForUpdateRead` 并克隆输入 `PlanNode`；DO 创建单行 `Projection`；TRACE/EXPLAIN 递归调用 `Build` 后用 boxed target 包装；其他语句产生管理/命令型计划。
4. 构建函数先做输入校验，再生成计划，同时按需修改构建器状态。例如 `buildImportInto` 先检查重复列赋值，在 NextGen+SEM+S3/OSS 条件下处理 URI，再通过 `requireInsertAndSelectPriv` 记录权限。
5. 调用方读取 `BuiltPlan`，并可通过 `GetVisitInfo`、`GetIsForUpdateRead`、`GetHintState`、`GetOptFlag` 取得附带状态；复用前调用 `ResetForReuse`。

主要分支的内部流程：

- **INSERT/REPLACE**：`buildInsert` 依次调用 `getAffectCols`、`buildValuesListOfInsert`、`resolveGeneratedColumns`。显式列名大小写不敏感且不可重复；未给列名时排除隐藏列，并根据行宽判断是否保留生成列占位；非 `DEFAULT` 值不能直接写生成列；每个值由 `convertValue` 做受限类型转换。
- **ANALYZE**：`buildAnalyze` 用 `handleAnalyzeOptions` 仅验证显式选项，再以 `mergeAnalyzeOptionsWithResets` 合并未被 DEFAULT 重置的保存值，最后 `fillAnalyzeOptions` 按当前全局变量填默认项。无索引名时按物理 ID 生成列任务，有索引名时解析索引并生成索引任务。
- **访问路径**：`getPossibleAccessPaths` 从 table path 起步，过滤 invisible/ignore/use/force 条件后加入索引路径，可选加入 TiFlash；有 FORCE 时仅保留匹配索引，结果为空返回错误。
- **SHOW/ADMIN**：`buildShowSchema` 和一组 `build*Schema` 函数定义稳定的输出列；ADMIN 分支按命令填充 schema 与 payload，并校验索引或 DDL job 选项。
- **IMPORT INTO/SEM**：`checkImportIntoColAssignments` 以小写列名检测重复。NextGen SEM 下，`checkNextGenS3PathWithSem` 要求 role ARN 或完整 access/secret key；Premium 模式要求显式 external ID 等于全局 keyspace，并由 `processNextGenS3PathWithSem` 重写为当前 keyspace；Starter 模式要求调用者提供非空 external ID 且保留原 URI。

## 数据与状态

`PlanBuilder` 是单次或串行复用的可变状态对象：

- `visitInfo` 是构建期间追加的权限需求，不在 `Build` 入口自动清空；跨语句复用必须显式 `ResetForReuse`。
- `qbOffset` 与 `handleHelper.scopes` 都是栈。`pushSelectOffset`/`popSelectOffset`、`pushMap`/`popMap` 必须成对；空栈弹出会触发 `expect`。
- `outerCTEs`、`curClause`、`optFlag`、`warnings`、`unusedViewHints` 是语句解析/改写的上下文或附带结果。`HandleUnusedViewHints` 会 drain 未使用提示并转为 warning。
- `disableSubQueryPreprocessing`、`allowBuildCastArray` 来自构造选项；`enableSemiJoinRewrite`、`noDecorrelate` 等开关保留为构建状态。当前 `ResetForReuse` 不重置全部开关，说明构造级选项意在跨复用保留；新增字段时必须明确属于“每次构建状态”还是“实例配置”。
- `nonViableFTSMatch` 与 `predicateMatchSeen` 通过 Mark/Has 方法写读，并在复用时清零。
- 大部分输入和输出使用拥有所有权的 `String`/`Vec`，构建时通过 clone 断开调用方借用；`BuiltPlan::Explain/Trace` 用 `Box` 拥有嵌套计划。
- `calcOnceMap` 缓存一次谓词列计算结果；第一次 `get_or_calculate` 执行闭包，之后返回缓存集合的克隆。

## 依赖与调用关系

RustCodeGraph 已索引 `planbuilder.rs`，查询 `buildImportInto` 能定位本文件 `PlanBuilder::buildImportInto`（第 1133 行）及 Go 对应实现（`planbuilder.go` 第 4820 行）；图的 `callers`/`callees` 对该 Rust 符号未返回边，因此以下直接边以源码分派为准：

- 上游：`PlanBuilder::Build` 是 typed 总入口，直接调用 `buildExecute`、`buildAnalyze`、`buildInsert`、`buildImportInto`、`buildDDL`、`buildExplain` 等。`pkg/planner/core/planbuilder_test.rs` 直接构造 `Statement` 并调用该入口。
- 完整 SQL 上游：独立测试通过 `crate::main_test::{exercise_statement_for_test, logical_optimize_default_for_test}` 验证 parser-backed SELECT、聚合、子查询和 CTE；这说明完整 SQL 主链不只经过本文件的精简 `Statement::Select`。
- 下游计划结构：`crate::task::{PlanNode, PlanKind, Expression, FieldType, TypeCode}`；访问路径使用 `crate::find_best_task::{AccessPath, IndexInfo}`。
- 下游环境边界：ANALYZE 默认值读取 `vardef_dependency` 原子/全局配置；SEM URI 读取 `config_kerneltype_dependency`、`config_deploymode_dependency`、`config_dependency`，并调用 `sem_dependency`；URI 解析依赖外部 crate `url = "2"`。
- crate 边界：`Cargo.toml` 声明默认 feature 为空，`nextgen` feature 传递到 deploymode/kerneltype 依赖；同一 manifest 的 `package.metadata.porting.go-package = "pkg/planner/core"` 明确 Go 移植来源。

该文件没有直接向执行器提交 I/O，也没有数据库连接或事务句柄；`BuiltPlan` 是供后续优化/执行层消费的值对象。

## 错误处理与边界

可恢复校验统一返回 `BuilderError(String)`，多数路径使用 `?` 原样传播。重要失败条件包括：空 prepared statement/SET 名称；绑定 SQL 为空或结构不匹配；索引、列、分区不存在；INSERT 列重复、行宽不符、生成列被显式赋非 DEFAULT；无可用访问路径；ANALYZE 选项越界或 sample num/rate 同时给出；IMPORT 重复赋值或 SEM 凭证/external ID 不合规；DDL job 并发、batch size、最大写速率越界。

`convertValue` 只实现有限的 typed 转换：NULL/DEFAULT 透传，同类型透传，字符串可解析为整数，整数可转 float，其他值可格式化为字符串；不应把它描述为完整 SQL 类型转换器。`splitWhere` 按字面量 `" and "` 拆分表达式名称，同样是 typed 辅助而非真正 AST 布尔分解。

两处栈 API 使用 panic 保卫内部不变量：`handleColHelper::popMap` 和 `PlanBuilder::popSelectOffset`。这类 panic 不是用户输入错误接口；调用者必须保持作用域 push/pop 配对。`parse_size` 会拒绝无法解析、负数、非有限或超过 `i64` 的值，但最终从 `f64` 截断为整数，因此扩展单位/精度时需保持 Go 兼容性。

当前实现中部分字段和函数主要保留 Go 对齐表面，例如 `capFlagType` 常量、若干 CTE/优化开关；文档不能据其存在推断完整 Go 子系统已经接线。

## 并发与资源生命周期

生产代码中没有 `async`、线程生成、锁、channel、文件句柄或显式事务。`PlanBuilder` 的核心入口要求 `&mut self`，设计上应由一个构建流程独占，不提供跨线程共享同步；即使类型可被移动，也不应并发修改同一实例。

资源生命周期主要是内存所有权与复用：输入通过借用进入，结果克隆所需元数据；递归 Explain/Trace 计划由 `Box` 管理；作用域栈必须严格配对；`ResetForReuse` 释放逻辑状态但复用向量/映射容量。`HandleUnusedViewHints::drain` 会消费提示，重复调用不会再次产生相同 warning。

`planbuilder_test.rs` 在 `nextgen` feature 下用全局 `Mutex` 串行化部署模式修改，并用 `DeployModeGuard::drop` 恢复原模式。这是测试对全局配置生命周期的保护，不是本文件生产路径内部的同步机制。ANALYZE 默认值和部署/keyspace 模式来自全局状态，因此并行调用虽不修改 builder 本身，仍可能观察外部配置变更；测试或新调用方修改这些全局值时必须提供隔离/恢复。

## 与 Go 版本的对应关系

直接对照为 `pkg/planner/core/planbuilder.go`，manifest 也通过 `package.metadata.porting` 指向 `pkg/planner/core`。名称与职责对齐项包括 `PlanBuilder`、构造选项、`ResetForReuse`、`Build` 分派、权限收集、访问路径、ANALYZE 选项、INSERT/IMPORT、SHOW/ADMIN schema 和 DDL job 参数。

但两者并非逐类型等价：

- Go `Build(ctx, *resolve.NodeW)` 接收真实 parser AST/resolution context，并先执行 SEM 检查、设置 resolve context 和列裁剪 flag；Rust `Build(&Statement)` 接收精简枚举，没有 context 参数。
- Go `PlanBuilder` 持有 session context、infoschema、外层 schema/name、hint processor/state、表达式重写器池和更多 DML/CTE 状态；Rust 本文件只保留可表达当前 typed 路径的子集。
- Go SELECT 从 AST 自底向上构造计划；Rust 本文件的 `Statement::Select` 直接克隆已有 `PlanNode`。Rust parser-backed 主流程由其他相邻模块覆盖，不能仅凭本文件断言 SELECT 已与 Go 完整等价。
- Rust 使用拥有所有权的枚举代替 Go 的接口/指针层次，并将部分 command payload 简化为字符串向量；错误是 `BuilderError(String)`，不保留 Go 错误类型层级。
- 对齐测试集中在可验证边界：`pkg/planner/core/planbuilder_test.rs` 对应 Go `planbuilder_test.go` 的 SHOW、索引路径、ANALYZE、权限、IMPORT、DDL job 和 S3 SEM 用例；Go 文件仍覆盖更多 expression rewriter、clone 和真实 planner 语义。

因此修改时应以“保持当前 Rust 行为并对照 Go 对应规则”为原则，而不是机械复制 Go 全文件或把 Go 未移植子系统递归拉入本任务。

## 扩展指南

- 新增语句类型时，同时扩展 `Statement`、`BuiltPlan`（若无合适输出）、`PlanBuilder::Build` 的穷尽分派和独立 `planbuilder_test.rs`；如属于完整 parser 主链，还需检查相邻 runtime planner 的 AST 转换入口。
- 新增构建器字段时，明确初始化、`ResetForReuse`、clone/所有权和跨语句残留策略。每次构建状态必须清零；构造选项应像现有开关一样有意保留。
- 调整 INSERT、ANALYZE、访问路径、权限或 SEM 规则时，先对照 `planbuilder.go` 的同名函数与 `planbuilder_test.go` 的对应测试；不要用更宽松的 Rust 校验替代 Go 边界。
- 新增 SHOW/ADMIN 输出时，集中修改相应 `build*Schema` 和 `buildShowSchema`，保持列名、次序、类型宽度和 flags 稳定；在独立 Rust 测试中断言 schema，而不要把测试内嵌到生产文件。
- 调整 ANALYZE 时保持三阶段顺序：验证显式值、合并保存值/DEFAULT reset、最后填动态默认值。`SampleRate` 以 `f64::to_bits` 存在 `u64` map 中，不能当普通整数比较。
- 调整 IMPORT URI 时同时考虑 `s3`/`oss`、下划线/连字符 key 别名、Starter 与非 Starter、认证二选一、重复 query key 的 first-value 语义，并使用 nextgen 条件测试隔离全局 deploy mode。
- 访问路径扩展应维护 invisible、multi-valued、vector、global、temporary、prefix length、TiFlash 和 FORCE/USE/IGNORE 的组合含义；性能风险主要来自候选路径集合膨胀及不必要 clone。
- 保持错误文本兼容时参考 Rust 的 `go_error_messages_and_table_plan` 及 Go 原测试。所有测试继续放在 `pkg/planner/core/planbuilder_test.rs`，遵守生产源码与测试分离要求。

## 验证依据

- 源码全貌：`pkg/planner/core/planbuilder.rs`（3150 行）；检查了类型、常量、`PlanBuilder` 全部入口、访问路径、ANALYZE、schema、权限、导入 URI 和 DDL job 辅助逻辑。
- crate 与模块：`pkg/planner/core/Cargo.toml`（crate 名、`nextgen` feature、依赖和 Go porting metadata）；`pkg/planner/core/lib.rs`（`pub mod planbuilder`、独立 `planbuilder_test.rs` 装配）。
- Rust 测试：`pkg/planner/core/planbuilder_test.rs`，覆盖 SHOW/Slow schema、大小写精确索引选择、ANALYZE 默认与边界、SHOW/ANALYZE/ADMIN 构建、权限与 IMPORT、RENAME 权限、DDL 参数、Premium/Starter SEM、parser-backed SELECT/聚合/子查询/CTE，以及 DEFAULT reset。
- Go 对照：`pkg/planner/core/planbuilder.go` 的 `PlanBuilder`、`NewPlanBuilder`、`ResetForReuse`、`Build`、`getPossibleAccessPaths`、`handleAnalyzeOptions`、`buildAnalyze*`、`buildInsert`、`buildImportInto`、`checkNextGenS3PathWithSem`；`pkg/planner/core/planbuilder_test.go` 的相应边界测试。
- RustCodeGraph：`status` 显示索引包含 11,467 files / 307,296 nodes / 1,848,419 edges；`query buildImportInto --json` 同时命中 Rust `planbuilder.rs:1133` 和 Go `planbuilder.go:4820`。对该 Rust 符号执行 `callers`/`callees` 未返回边，因此调用关系以 `PlanBuilder::Build` 源码和测试直接调用复核，未虚构图边。
- 本任务是纯文档分析，按计划不运行 Cargo。验收使用任务指定的 11 个固定二级标题结构命令，并人工检查本地链接、符号名称、已实现/未接线边界及独立测试位置。
