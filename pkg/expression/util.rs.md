# `pkg/expression/util.rs`

## 文件定位

列依赖提取：`ExtractDependentColumns` 展开虚拟列定义；`ExtractColumns*` 按 `UniqueID` 去重或写入调用方提供的 `HashMap`、切片、`FastIntSet`；`ExtractCorColumns` 和 `extractColumnsAndCorColumns` 处理关联列。

## 核心职责

文件的职责可以分成九组：

1. 表达式集合操作：`Filter` 保序追加命中项，`FilterOutInPlace` 从后向前删除并返回逆序的被过滤项。
2. 列依赖提取：`ExtractDependentColumns` 展开虚拟列定义；`ExtractColumns*` 按 `UniqueID` 去重或写入调用方提供的 `HashMap`、切片、`FastIntSet`；`ExtractCorColumns` 和 `extractColumnsAndCorColumns` 处理关联列。
3. 等价与替换：`ExtractEquivalenceColumns`、`ExtractConstantEqColumnsOrScalar` 识别等值约束；`ColumnSubstituteImpl` 在 schema 映射、排序规则和失败回退约束下递归替换列。
4. 谓词规范化：`EliminateNoPrecisionLossCast` 消除安全 CAST；`PushDownNot` 按德摩根律和 SQL 三值逻辑下推 NOT；`ExtractFiltersFromDNFs` 提取 DNF 分支公共项；`DeriveRelaxedFiltersFromDNF` 生成仅引用给定 schema 的宽松下推条件。
5. AST、Row 与常量辅助：包括参数标记检查、位置表达式解析、Row 参数校验和常量取值。
6. 计划缓存与执行性质检查：识别运行时常量、非确定函数、可变副作用、参数或 deferred 常量，并可用 `RemoveMutableConst` 固化后者。
7. 下推收益和展示辅助：判断虚拟列、关联列、JSON 投影下推收益，格式化表达式、字节数和纳秒时长。
8. SQL digest 回填：`SQLDigestTextRetriever` 从本地或集群 statements summary 表查询规范化 SQL 文本。
9. MySQL 二进制协议参数：`ExecBinaryParam` 将 COM_STMT_EXECUTE 参数解码为带推断类型的 `Constant` 表达式。

## 主要符号

- `type Expr = Box<dyn Expression>`、`type ColumnFilter = fn(&Column) -> bool`：文件内部统一的拥有型表达式与列谓词别名。
- `cowExprRef<'a>`：`ColumnSubstituteImpl` 的写时复制参数容器。`Set` 只在首个实际变化时克隆整个参数切片，`Result` 返回最终克隆列表。
- `Filter` / `FilterOutInPlace`：表达式列表分选；后者的 `filtered_out` 顺序是反向扫描顺序，这一点由 `pkg/expression/util_test.rs` 固定。
- `ExtractDependentColumns` / `ExtractColumns` / `ExtractCorColumns` / `ExtractColumnsFromExpressions` / `ExtractColumnSet`：不同去重、排序、容器和关联列语义的依赖提取入口。`ExtractColumns` 以 `UniqueID` 去重并排序，保证结果稳定。
- `uniqueIDToColumnMapPool`、`GetUniqueIDToColumnMap`、`PutUniqueIDToColumnMap`：线程局部的 `HashMap<i64, Column>` 复用池；归还前必须清空。
- `FindUpperBound`：只接受 `column < int64` 或 `column <= int64`，严格小于用 `wrapping_sub(1)` 形成闭上界。
- `ColumnSubstitute` / `ColumnSubstituteAll` / `ColumnSubstituteImpl`：基于 `Schema::ColumnIndex` 的表达式替换。返回值区分“发生替换”和“替换失败”；强制模式在任一子项失败时保留原树。
- `logicalOps`、`oppositeOp`、`symmetricOp`、`CompareOpMap`：惰性初始化的逻辑/比较运算符集合和映射，是谓词改写的规则表。
- `EliminateNoPrecisionLossCast` / `PushDownNot` / `ContainOuterNot`：规范化布尔表达式；CAST 消除仅覆盖兼容字符串和符号属性一致且宽度不缩小的整数类型。
- `ExtractFiltersFromDNFs` / `DeriveRelaxedFiltersFromDNF`：分别提取 DNF 公因子、导出可安全下推到给定 schema 的弱条件。
- `ParamMarkerInPrepareChecker` / `ParamMarkerExpression` / `PosFromPositionExpr`：连接 parser AST 参数标记、prepare 状态与表达式常量。
- `IsRuntimeConstExpr`、`CheckNonDeterministic`、`IsMutableEffectsExpr`、`IsImmutableFunc`、`MaybeOverOptimized4PlanCache`、`RemoveMutableConst`：描述表达式的稳定性、副作用和计划缓存安全性。
- `SQLDigestTextRetriever`：持有 `SQLDigestsMap` 与 `fetchAllLimit`，通过 `unresolvedDigests`、`hasUnresolvedDigests` 和 `retrieveDigests` 控制查询；`RetrieveLocal` 和 `RetrieveGlobal` 负责分层回填。
- `ExecBinaryParam` 与 `binaryDate*` / `binaryDuration*`：二进制协议解码入口和时间文本组装辅助。
- `IsConstNull`、`IsColOpCol`、`ExtractColumnsFromColOpCol`：比较谓词的末端形态检查。

## 执行流程

列提取从 `ExtractColumns` 进入：递归函数 `extractColumns` 对 `Column` 读取 `UniqueID` 写入 map，对 `ScalarFunction` 遍历 `GetArgs()`，其他节点不继续；随后收集 map values 并按 `UniqueID` 排序。需要保留重复、复用容器或输出整数集合时，入口分别切换为 `ExtractAllColumnsFromExpressions`、`ExtractColumnsMapFromExpressionsWithReusedMap` 或 `ExtractColumnsSetFromExpressions`。`ExtractDependentColumns` 有意不同：遇到带 `VirtualExpr` 的列还会递归展开生成列表达式。

列替换从 `ColumnSubstituteImpl` 进入。列节点先在 schema 中定位并选取同索引 replacement；`InOperand` 列通过 `SetExprColumnInOperand` 将标志递归传入替换树。CAST/GROUPING 单独重建并保留类型 flag 与 coercibility。普通标量函数先推导旧 collation，再逐参数递归替换；启用新 collation 时试算替换后的 collation，只接受相同参数校对或不降低严格性的变化。`cowExprRef` 避免无变化时提前复制全部参数。若最终变化，等号左侧为常量时交换两侧，再调用 `NewFunction` 重建；错误或强制替换失败时返回原表达式并设置失败标志。

NOT 下推由 `PushDownNot` 调用 `pushNotAcrossExpr`。连续 NOT 翻转状态；比较运算使用 `oppositeOp`；AND/OR 使用德摩根律互换并递归处理参数；不能继续下推时才新建 `UnaryNot`。`wrapWithIsTrueForPushNot` 对非逻辑整型参数包裹 `IS TRUE WITH NULL`，避免二值化破坏 NULL 语义。`EliminateNoPrecisionLossCast` 会先展平 AND/OR，递归处理比较项，只有常量对侧且类型/校对兼容时才剥离列外层 CAST。

DNF 公因子提取时，`extractFiltersFromDNF` 展平 OR 分支，再拆分各分支的 CNF 项；用表达式 `HashCode()` 统计每个哈希出现的分支数，并用每分支 `seen` 防止同一条件重复计数。共有项被移出各分支；某一分支只剩公共项时整个 remainder 消失，否则重新组合 CNF 和 DNF。提取结果按哈希排序，避免 `HashMap` 遍历造成不稳定输出。

digest 回填由 `retrieveDigests` 每阶段重新收集并排序未解析 digest；空集合不发 SQL。未解析数量不超过阈值（默认 512，阈值为 0 时回退 512）时使用 `%?` 占位符的 `IN` 查询，否则只发一条无过滤查询，避免按批次重复扫描历史文件。`runFetchDigestQuery` 每次选择本地/集群的 current/history 单表，使用 `SELECT DISTINCT digest, digest_text` 保留旧 UNION DISTINCT 的成对去重，并把上下文标为 `kv::InternalTxnOthers`。`RetrieveLocal` 顺序查询本地当前、本地历史；`RetrieveGlobal` 顺序查询本地当前、集群当前、本地历史、集群历史。每阶段只回填请求中尚为空的文本，忽略额外结果和空文本；已经补齐则跳过所有后续扫描。

二进制参数处理在 `ExecBinaryParam` 中逐项按 MySQL type 分派：整数和浮点按小端解码；日期、时间戳和时长先按允许长度构造文本，再交给 `types` 解析；decimal 接受截断但传播其他错误；字符串/blob 按 null 约定生成 Datum；未知类型产生 `ERR_UNKNOWN_FIELD_TYPE`。最后为每个 Datum 调用 `InferParamTypeFromDatum`，包装为 `Constant`。

## 数据与状态

大多数函数是对拥有型 `Box<dyn Expression>` 或借用表达式树的同步转换。递归识别依赖 `as_any().downcast_ref::<...>()`，因此当前明确遍历的组合节点主要是 `ScalarFunction`；新增表达式节点若包含子表达式，不会自动被这些工具看见。

列身份由 `Column::UniqueID` 决定。map 版本以该字段覆盖重复项，排序版本也按它稳定排序；slice 版本可保留重复，调用方必须选择与语义匹配的入口。`FastIntSet` 写入时把 `i64 UniqueID` 转为 `i32`，延续 Go 接口约束。

表达式重建必须维护派生状态：`SetExprColumnInOperand`、`removeMutableConstExpr` 和 GROUPING 重建会调用 `CleanHashCode`；`ColumnSubstituteImpl` 还保留 coercibility 和字段 flag。忽略这些缓存/类型元数据会造成相等判断、DNF 哈希或排序规则推导使用旧值。

`SQLDigestTextRetriever::SQLDigestsMap` 同时表示请求集合与回填结果：key 是 digest，空字符串表示未知。`fetchAllLimit` 默认为 512；独立测试通过 `SQLExecutor` 边界提供四类摘要数据并记录 SQL/参数，不在生产对象内保存 mock 数据。`updateDigestInfo` 只写空值，保证调用方预置结果不被覆盖。

静态规则表使用 `LazyLock<HashSet<_>>` / `LazyLock<HashMap<_, _>>`，初始化后只读。容量与时间单位常量均为 `f64`；`formatFloatLikeGo` 专门补齐 Go 对 NaN、正负 Inf 和带符号两位指数的拼写。

## 依赖与调用关系

crate 边界由 `pkg/expression/Cargo.toml` 确认，crate 名为 `astersql-expression`，`lib.rs` 是库入口且关闭 doctest。该文件通过 `use crate::*` 使用本 crate 的表达式节点、构造器、collation、fold、错误和上下文 API，并直接依赖 Cargo 中声明的 `intset`、`collate-dependency`、`types-dependency`、`types-decimal`、`types-time`、`exprctx-dependency`、`expropt`、`kv-dependency`、`param`、parser AST/mysql/opcode/driver 等 crate。

RustCodeGraph 的 `ExtractColumns` 节点显示其下游为本文件 `extractColumns`，上游包括 `pkg/expression/constant_propagation.rs::extractColumnsInternal`、planner integration test 以及多个 planner runtime/physical plan 路径。源码搜索进一步确认：

- `pkg/expression/constant_propagation.rs` 使用 `ExtractColumns` 和 `ColumnSubstitute` 做常量传播。
- `pkg/planner/core/operator/logicalop/logical_join.rs` 使用 `ExtractFiltersFromDNFs`、`PushDownNot` 和 `ExtractColumns` 规范化 join 谓词。
- `pkg/planner/core/operator/logicalop/logical_cte.rs` 使用 `ExtractFiltersFromDNFs`；projection、aggregation、window、datasource 等节点广泛使用 `ExtractColumns`。
- `pkg/planner/core/optimizer_runtime.rs` 使用 `ColumnSubstitute(All)` 做投影、schema 和谓词重写。
- `pkg/planner/core/expression_rewriter.rs` 使用计划缓存检查与 `RemoveMutableConst`；`pkg/util/ranger/detacher.rs` 也检查可变常量，防止 range 构建过度优化。
- `SQLDigestTextRetriever` 的运行时直接证据位于 `pkg/expression/util_runtime_parity_aster_unit_test.rs`；生产调用者若由泛型 `SQLExecutor` 间接接线，当前索引未给出可靠的 Rust 上游，本文不据此推测具体业务入口。

## 错误处理与边界

摘要检索保留 Go `3f2de015d91e4423164ca6e0beb07255869df7d5` 的错误顺序：本地当前失败立即返回；集群当前错误暂存，仍尝试本地历史。本地历史失败优先返回；若本地历史已补齐则忽略集群当前错误，否则传播该错误而不再扫描集群历史。共享上下文若已耗尽期限，本地历史也可能无法恢复，这是来源提交接受的权衡。`sqlDigestRetrieverSkipRetrieveGlobal` failpoint 通过已有 `fail` crate 启用，跳过集群查询但保留本地历史。

函数按用途采用三类失败策略。可恢复构造/求值路径返回 `Result<_, Error>`，例如 `SubstituteCorCol2Constant`、`CheckArgsNotMultiColumnRow`、`PopRowFirstArg`、常量取值、digest SQL 执行和 `ExecBinaryParam`。优化型 best-effort 重写通常返回原表达式并携带 `changed/failed`，避免优化失败改变查询语义；`ColumnSubstituteImpl` 的 collation 推导错误会记录日志或标记失败。

若调用者破坏已由协议或构造器保证的前置条件，部分辅助函数会 panic：`timeZone2int` 假定 `±HH:MM`；二进制日期辅助按固定长度切片；`GetFuncArg` 假定参数索引有效；多个 `NewFunctionInternal(...).unwrap()` 和组合条件 `expect` 假定既有表达式形态合法。对外协议入口 `ExecBinaryParam` 在进入固定长度辅助函数前验证时间/时长长度，但整数/浮点分支仍要求 `BinaryParam.Val` 与类型宽度一致。

`FindUpperBound` 的 `< i64::MIN` 通过 `wrapping_sub` 得到 `i64::MAX`，是代码当前事实，调用方不能把它误解成溢出错误。`getValidPrefix` 只接受 2..=36，合法前导 `+` 从结果删除，单独符号返回空串。`GetIntFromConstant` 将解析失败表现为 `(0, true)`，与 SQL NULL/无效位置的上层约定绑定。

`ExecBinaryParam` 的 decimal `IsNull` 分支生成默认 decimal，而字符串类 null 生成 NULL Datum、blob null 生成空字节；这些类型间差异来自当前 Go 对齐实现，扩展时不能统一化而不先核对协议兼容性。

## 并发与资源生命周期

文件自身不启动线程、异步任务或通道。`uniqueIDToColumnMapPool` 是 `thread_local!` 的 `RefCell<Vec<HashMap<...>>>`：map 只在当前线程借出和归还，避免 `Column` 内部非 `Send` 表达式跨线程移动；`PutUniqueIDToColumnMap` 清空内容后压回池。借出后未归还只损失复用收益，不影响正确性；跨线程归还不可由该 API 表达。

`CorrelatedColumn` 的 datum 在 `SubstituteCorCol2Constant` 中通过读锁访问，锁中毒会以 `expect("correlated datum lock poisoned")` panic；数据克隆后锁立即释放，后续常量折叠不持锁。

`SQLDigestTextRetriever` 由调用方独占可变借用更新，没有内部共享同步。一次 `RetrieveGlobal` 顺序执行至多四阶段查询；它不并行请求，也不拥有 executor。`kv::Context` 在各阶段间克隆，restricted SQL 返回的 rows 被立即转为 `HashMap`，资源生命周期由 `SQLExecutor` 实现管理。

惰性静态规则表由标准库保证线程安全初始化，之后只有共享只读访问。表达式树采用 clone/rebuild，工具函数不就地修改调用方共享的原节点；需要修改的哈希、类型 flag 或参数都发生在克隆节点上。

## 与 Go 版本的对应关系

digest 策略对应 Go `3f2de015d91e4423164ca6e0beb07255869df7d5`。Go 的 mockQuery 钩子以独立测试中的 SQLExecutor 外部边界替身实现；生产 SQL 构造、请求来源和回填使用真实 Rust 入口。运行时入口已挂载于 expression crate，当前可靠的直接调用证据仍为独立测试，不能据此声称新增了 SQL 内建函数接线。

需要注意的实现层差异：

- Go `uniqueIDToColumnMapPool` 是 `sync.Pool`，Rust 因 `Column` 非 `Send` 改为线程局部池；复用范围变窄，但避免跨线程传递节点。
- Go map 迭代无序；Rust 的 `ExtractColumns`、多表达式列提取和 DNF 公因子输出显式排序，以维持稳定结果。
- Go 接口值和切片替换对应 Rust trait object、所有权 clone 与 `cowExprRef`；语义重点是“未变化保留原树、变化后清缓存并重建”，而不是逐语句机械相同。
- Go `strconv.FormatFloat` 的 Inf 和指数拼写由 Rust `formatFloatLikeGo` 显式模拟。
- Rust `ParamMarkerInPrepareChecker` 的通用 `enter`/`leave` 基本放行，核心状态只在 `enter_param_marker` 更新；扩展 AST visitor 时应继续以 Go 的 prepare/execute 语义为准。

Go 测试 `pkg/expression/util_test.go` 是完整参考；Rust 独立测试 `pkg/expression/util_test.rs` 覆盖列表顺序、数值前缀、二进制时间、格式化、时区和校对边界，`pkg/expression/util_runtime_parity_aster_unit_test.rs` 补充 digest 查询及更多运行时对等场景。`util_test.rs` 中的 9 个 `port_3f2de015d9_` 用例进一步覆盖分层摘要命中、部分命中、fetch-all 单次扫描、未请求结果保护、错误恢复与 failpoint；原有两个 `test_digest_retriever_` 用例继续保留旧输入和结果契约。未被现有 Rust 测试点名的其他分支不能仅凭移植关系宣称已充分验证。

## 扩展指南

新增表达式节点或容器节点时，先检查所有递归 walker：`extractDependentColumns`、`extractColumns*`、`ExtractCorColumns`、`extractColumnsAndCorColumns`、`extractColumnSet`、`ColumnSubstituteImpl`、NOT/DNF 辅助、`containMutableConst`、`removeMutableConstExpr`、`HasColumnWithCondition` 和副作用检查。目前它们大多只递归 `ScalarFunction`，新节点若拥有子表达式，需要明确决定是否参与每一种语义。

扩展列替换或函数重建时，应同时维护 `InOperand`、返回类型 flag、coercibility、collation 推导和 hash/canonical hash 清理；必须覆盖普通模式与 `ColumnSubstituteAll` fail-fast 回退。建议在独立测试文件中新增用例，不把测试内嵌到 `util.rs`；优先扩展 `pkg/expression/util_test.rs`，涉及 SQL executor/digest 的场景扩展 `pkg/expression/util_runtime_parity_aster_unit_test.rs`。

新增 NOT 可翻转运算符时，要同步 `logicalOps`、`oppositeOp`（需要交换参数时还要考虑 `symmetricOp`），并验证 NULL、非整型真值和嵌套 AND/OR。新增 DNF 提取逻辑时要保持每分支去重、稳定排序，以及“某分支只剩公因子时 remainder 为 None”的不变量。

扩展 MySQL 参数类型时，在 `ExecBinaryParam` 增加分支并核对 `pkg/expression/util.go`、协议长度、null 表现、符号扩展和最终 `InferParamTypeFromDatum`。固定长度 helper 不做完整边界检查，因此所有外部入口必须先验证 buffer 长度；回归测试应放在独立 Rust 测试文件，并包含 malformed packet。

改变 digest 策略时需保持内部请求来源、current 优先与未解析 digest 的 history 回退、错误恢复顺序、已有文本不覆盖和大集合单次 fetch-all 行为；`fetchAllLimit` 改动必须同步 Go 语义与 `test_digest_retriever_matches_go_partial_and_fetch_all_contract`。性能风险主要来自表达式深递归、无必要 clone、HashCode 重算和全量 statements summary 查询；兼容风险主要来自 collation、SQL 三值逻辑、计划缓存可变常量和二进制协议边界。
