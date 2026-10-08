# `pkg/session/runtime/source.rs`

## 文件定位

`source.rs` 是 `astersql-session` crate 的完整关系查询数据源执行层。`pkg/session/runtime.rs:116,174-175` 以私有 `mod source` 装配本文件，仅将 `is_scalar_count_non_null_constant` 提升为 `pub(crate)`，其余入口通过 `use source::*` 服务同一 `runtime` 模块。`pkg/session/Cargo.toml` 的 `[package] name = "astersql-session"`、`[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/session"` 确认其 crate 与 Go 包边界；目标包没有 `doc.go`。

本文件位于解析后的 `ast::SelectStmt`/`ResultSetNode` 与 `ConcreteSession` 的表扫描、表达式求值和结果集适配之间。普通单表快速路径主要在 `runtime/query.rs`，而派生表、多源 JOIN、集合运算、CTE、聚合/窗口组合等会进入本文件的 `execute_insert_select_node_with_outer`。它不是同路径 Go 文件的翻译：仓库不存在 `pkg/session/runtime/source.go`，可核对的 Go 行为主要分散在 `pkg/executor/cte.go`、JOIN/投影/排序执行器和相关测试中。

## 核心职责

1. 递归识别并执行表源：物理/临时表、事务 overlay、视图、虚拟系统表、派生表和 CTE 均被规范化为 `InsertSelectRows`。
2. 执行关系组合：支持 CROSS/INNER、LEFT、RIGHT、NATURAL/USING/ON、LATERAL，以及 UNION/INTERSECT/EXCEPT 的 DISTINCT/ALL 变体。
3. 编排完整 SELECT 阶段：WITH、FROM、WHERE、GROUP BY、HAVING、Projection、DISTINCT、ORDER BY、LIMIT，并为相关子查询合并外层行。
4. 管理 CTE 生命周期：维护语句级嵌套作用域，按 delta 迭代递归 CTE，执行 UNION 去重、递归深度检查、panic 恢复和内存超额后的临时文件溢写。
5. 提供查询结构辅助能力：收集物理表源、识别派生表和 stale-read 组合、识别可直接计数的 `COUNT(*)`/`COUNT(非 NULL 常量)`。

## 主要符号

- `trigger_recoverable_cte_failpoint`（第 20 行）：调用测试 failpoint，并用 `catch_unwind` 把字符串 panic 转为 `SessionError`；非字符串 payload 使用固定兜底消息。
- `RecursiveCteSpill`（第 41 行）：持有临时文件路径和已打开文件。`new` 最多尝试 16 个由进程 ID 与原子递增 ID 组成的独占文件名；`append` 写入自描述二进制行；`read_all` 解码；`Drop` 删除文件。
- `recursive_cte_values_size`（第 154 行）：以每个可空值 16 字节固定开销加字符串长度做饱和估算，作为是否触发 CTE spill 的阈值依据。
- `collect_physical_table_sources`、`result_set_node_has_derived_source`、`stale_source_profile`（第 161-240 行）：递归遍历 JOIN/派生查询，分别收集真实表源、检测派生表、汇总“存在 AS OF / 存在当前读”两个布尔量。
- `literal` 与 `is_scalar_count_non_null_constant`（第 243、248 行）：前者复用 `dml_runtime::Literal`；后者只接受单投影、非 DISTINCT、无聚合内排序的 COUNT，参数为空或唯一非 NULL 字面量。
- `execute_insert_select_table_source_with_outer`（第 358 行）：表源总入口。依次处理派生查询、从内到外搜索 CTE、系统表、临时/MDL 表、视图和普通扫描，并施加表名或别名限定。
- `execute_insert_select_join_with_outer`（第 614 行）：JOIN 主体；处理 LATERAL 右源、USING/NATURAL 合并列、歧义 marker、外连接补 NULL、等值 hash 快路和嵌套循环兜底。
- `visit_insert_select_source`（第 910 行）：对满足约束的 CROSS JOIN 逐左行访问，避免先物化完整笛卡尔积；右侧仍被物化。
- `execute_insert_select_set_list_with_outer`（第 992 行）：按 AST 分支顺序执行集合运算，并以行向量和计数表实现 DISTINCT/ALL 语义。
- `apply_insert_select_order_limit`（第 1070 行）：计算排序键，按 NULL、十进制数值或字符串排序，再解析 LIMIT/OFFSET 并调用统一 limit 执行逻辑。
- `execute_with_clause`、`execute_recursive_cte`（第 1143、1212 行）：安装/弹出 CTE scope；非递归 CTE 直接物化，递归 CTE 以 seed 和上一轮 delta 驱动迭代。
- `execute_insert_select_query_with_outer`、`execute_insert_select_query_body_with_outer`、`execute_insert_select_node_with_outer`（第 1419、1457、2065 行）：完整查询的校验入口、主体和 AST 动态分派入口。

本文件没有模块级常量、枚举、trait 或条件编译项；唯一静态量是 `RecursiveCteSpill::new` 内的 `AtomicU64 NEXT_SPILL_ID`。

## 执行流程

### 入口与表源

`runtime/query.rs` 的完整查询、子查询入口调用 `execute_insert_select_node_with_outer`；`runtime/explain_analyze.rs:433,456` 也直接调用 `execute_insert_select_query` 获取真实结果。节点分派只接受 `SelectStmt`、`SetOprStmt` 或 `SetOprSelectList`，其他节点明确报错。

对单个 `TableSource`，执行顺序是：若有 `QuerySource`，递归执行派生查询并加别名；否则按 `cte_scopes` 逆序查找同名 CTE；再尝试 information/performance/metrics/sys 虚拟表；最后解析本地临时表或 MDL 表。视图会重新 parse 已存 `CREATE VIEW` SQL、执行其 SELECT、校验并重命名输出列；普通表根据事务是否存在选择 `scan_latest_with_transaction_overlay` 或 `scan_registered_table`。

### JOIN 与集合运算

JOIN 先执行左源。LATERAL 只允许作为 CROSS/INNER 的右源，并为每个左行把该行传作 `outer`；其他 JOIN 先执行右源。`insert_select_join_columns` 从 NATURAL 公共列或 USING 列得到合并键，`merge_insert_select_rows` 对重复非限定列放置歧义 marker，随后移除 USING/NATURAL 合并列的歧义。

简单且左右键类型一致的等值 ON 可构造右侧 `HashMap<String, Vec<usize>>`；否则对左右行做嵌套循环。ON 与 USING/NATURAL 条件全部通过 `insert_select_join_matches` 复核。LEFT/RIGHT 未匹配行由 `null_insert_select_row` 补 NULL；RIGHT JOIN 按右侧顺序缓存匹配结果。无 ORDER/LIMIT/DISTINCT/聚合/相关外层行等条件的 CROSS JOIN 可由 `visit_insert_select_source` 边访问边投影，以缩小峰值中间结果。

集合运算先取得第一分支列布局，后续分支必须同列数。UNION DISTINCT 用 `HashSet` 保留首个出现值；INTERSECT/EXCEPT ALL 用右侧行计数逐次消费；DISTINCT 变体另用集合去重。集合自身和外层 `SetOprStmt` 的 ORDER/LIMIT 在相应 AST 层分别执行。

### CTE

`execute_with_clause` 推入空 scope，按声明顺序物化 CTE，因此后续 CTE 可引用前序定义；列名列表会校验宽度并重命名普通列及字符串 marker。主体完成或失败后都会执行 cleanup failpoint 边界并弹出 scope。

递归 CTE 要求查询为集合运算且至少包含 seed 与一个递归成员。seed 执行后按显式列名重命名；UNION 而非 UNION ALL 时维护全局 `seen`。每轮只把上一轮 `delta` 安装为 CTE 当前值，执行所有递归分支，检查列数和仅允许 UNION/UNION ALL；空 delta 表示收敛。达到 `cte_max_recursion_depth` 则报错。

当全局 `EnableTmpStorageOnOOM` 开启，且 failpoint 强制或估算内存超过 `tidb_mem_quota_query` 时，历史结果被追加到真实临时文件，只保留本轮 delta 在内存中；收敛时重新读取历史行。磁盘峰值写入 `last_statement_disk_max`，`Drop` 最终删除文件。

### SELECT 主体

`execute_insert_select_query_with_outer` 先执行 `validate_only_full_group_by`，然后收集谓词统计并处理 WITH。主体先识别 TIDB_ROW_CHECKSUM 的严格 point/batch point-get 约束，再判定聚合、子查询和可流式 CROSS JOIN。

通用路径依次执行 FROM、合并 outer 行、WHERE、非聚合预排序/限制、GROUP BY 分组、窗口上下文、Projection、HAVING、聚合 ORDER/LIMIT、DISTINCT，最后在 DISTINCT 之后应用最终 LIMIT。`source_test.rs::distinct_is_applied_before_order_by_and_limit` 固定了普通与聚合查询中 DISTINCT 先于最终 LIMIT 的行为。

## 数据与状态

- 统一中间结果 `InsertSelectRows` 定义于 `runtime/query.rs`：`columns` 保持输出顺序，`rows` 是 `HashMap<String, Option<String>>`；SQL NULL 为 `None`，类型/歧义信息通过父模块的隐藏 marker 保存。
- `outer: Option<&HashMap<...>>` 表示相关子查询或 LATERAL 的外层行。本层列先写入，外层键只在缺失时补入，因此内层作用域会遮蔽外层同名列；歧义 marker 也有显式遮蔽检查。
- `ConcreteSessionInner::cte_scopes`（`runtime/session.rs:616`）是 `RefCell<Vec<HashMap<String, InsertSelectRows>>>`。嵌套查询逆序查找，符合内层优先的名称解析；scope 是语句级物化状态。
- `last_statement_disk_max: Cell<i64>` 记录递归 CTE 临时文件峰值，供 `DiskTrackerMaxConsumedForTest` 观察；`CTEStorageMapIsEmptyForTest` 检查语句结束后 scope 不残留条目。
- DISTINCT、集合运算和递归 UNION 的行身份均基于按 `columns` 顺序生成的 `Vec<Option<String>>`，避免 `HashMap` 迭代顺序影响结果。
- JOIN、排序、分组、DISTINCT 和一般集合运算通常在内存中全量物化；只有受限 CROSS JOIN visitor 和递归 CTE 历史行 spill 降低部分峰值内存。

## 依赖与调用关系

上游直接证据：

- `runtime/query.rs:772,1600`：相关子查询与完整查询通过 `execute_insert_select_node_with_outer` 进入本文件；`query.rs:1634,2716` 使用标量 COUNT 判定。
- `runtime/explain_analyze.rs:433,456`：EXPLAIN ANALYZE 的复杂路径调用 `execute_insert_select_query`。
- `runtime/dispatch.rs:264`、`control.rs`、`dml.rs`、`explain_select.rs`、`query.rs`、`system_query.rs` 和 `select_into.rs` 多处调用 `collect_physical_table_sources`；`query.rs` 还使用派生表和 stale-source 检测器。
- 本文件内部由节点分派递归进入派生表、集合分支、CTE seed/递归成员和嵌套 SELECT。

主要下游依赖：

- parser/AST：`astersql-parser-ast` 的 SELECT、JOIN、集合运算、CTE、表达式、ORDER/LIMIT 节点决定所有控制流；`parse` 用于系统表查询和视图定义重解析。
- 相邻 runtime：`query.rs` 提供 `InsertSelectRows`、表达式/窗口/聚合判断与求值、marker、limit 和 predicate；`relational_scan.rs`/`session.rs` 提供表扫描、事务 overlay 和会话状态；`dml_runtime::Literal` 解析字面量。
- metadata/variables：表与列来自 MDL/本地临时表；`astersql-sessionctx-vardef` 提供内存配额、临时存储开关和最大递归深度。
- 标准库资源：`HashMap`/`HashSet` 管理行与去重，`RefCell` 管理 scope，`AtomicU64` 生成 spill 文件名，`std::fs::File` 与 `Read/Write/Seek` 持久化历史行。
- `pkg/session/Cargo.toml` 明确依赖 parser、sessionctx、types、infoschema、planner/executor 相邻 crate、`rust_decimal`、`crc32fast` 和 test failpoint；`nextgen` feature 仅转发配置 feature，本文件无 `#[cfg]` 分支。

## 错误处理与边界

- 全部执行入口返回 `SessionResult`。IO、UTF-8、数值解析等错误被包装成含操作上下文的 `SessionError`；标量子查询多行错误由表达式层原样保留。
- 视图 SQL 必须能 parse 为 `CreateViewStmt`，且运行时输出宽度必须等于声明列数；CTE 显式列名和递归成员也必须与 seed 等宽。
- JOIN 缺左源、LATERAL 用于非 CROSS/INNER、流式 visitor 遇到非 CROSS/INNER、集合分支列数不等都会显式失败。USING/NATURAL 的 NULL 键不匹配，ON 使用 SQL predicate 语义。
- 递归 CTE 只接受 UNION/UNION ALL；缺 seed/递归项、最大深度为零或迭代未收敛均报错。seed/recursive panic 通过 recoverable failpoint 转成错误；cleanup panic 被捕获以避免锁/作用域清理路径中断。
- spill 格式以列数、NULL marker、字节长度和 UTF-8 payload 编码。截断的行头 EOF 被视为文件结束，但行内部截断、非法 UTF-8或超出地址空间均报错；文件创建冲突最多重试 16 次。
- TIDB_ROW_CHECKSUM 仅允许直接投影且必须是带主键点查/批点查；嵌套表达式、WHERE 中使用、无主键或非点查会被拒绝。
- ORDER BY 的非 NULL 值在双方都能解析为 `rust_decimal::Decimal` 时数值比较，否则字符串比较；该紧凑模型不等价于完整 MySQL 类型/排序规则系统，扩展时需防止扩大“已兼容”声明。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道，查询解释执行在当前会话线程同步完成。`cte_scopes` 与 `last_statement_disk_max` 分别使用 `RefCell`/`Cell`，明确是会话内可变状态而非跨线程共享状态。

`execute_with_clause` 在调用主体前 push scope，并在闭包返回后 pop；正常结果和 `SessionResult::Err` 都经过 pop。递归每轮用上一轮 delta 替换同名 CTE，收敛后再安装完整结果，避免递归成员读取尚未完成的累计结果。

spill 文件以 `create_new` 防止覆盖既有路径，文件句柄随 `RecursiveCteSpill` 生命周期持有；离开函数（包括错误展开）时 `Drop` 尝试删除路径。删除错误被忽略，因此进程/权限异常下仍可能残留文件，这是当前明确的清理边界。原子 ID 采用 `Relaxed`，只用于同进程唯一命名，不承载数据同步语义。

JOIN/集合/排序/分组与结果向量由当前函数拥有，无共享锁。Go CTE 的 `resTbl`/`iterInTbl` 以锁协调多个 executor；Rust 紧凑 runtime 以单会话 scope 和同步物化取得行为对齐，并不复制其并行 executor 所有权模型。

## 与 Go 版本的对应关系

仓库没有 `pkg/session/runtime/source.go`，因此只能按可验证行为对照，不能声称结构一一对应。

- Rust `execute_recursive_cte` 的 `accumulated`/`delta` 对应 `pkg/executor/cte.go` 注释和 `cteProducer` 中的 `resTbl`/`iterInTbl`/`iterOutTbl`：每轮递归只读取上一轮输入，输出并入完整结果，再成为下一轮输入。
- Rust 使用与 Go 相同路径名的 `testCTESeedPanic`、`testCTERecursivePanic`、`testCTEStorageSpill`、`assertIterTableSpillToDisk` 和 `mock_cte_exec_panic_avoid_deadlock`；Go `computeSeedPart`/`computeRecursivePart` 也恢复 panic，`Close` 在存储锁边界测试 panic 后仍释放锁。
- Go `setupCTEStorageTracker` 让 chunk row container 在配额下 spill，并记录 memory/disk tracker；Rust 用 `recursive_cte_values_size`、全局临时存储开关和 `RecursiveCteSpill` 实现本运行时可验证的磁盘物化，记录 `last_statement_disk_max`。
- `pkg/executor/test/cte/cte_test.go` 的 `TestSpillToDisk`、`TestCTEPanic`、`TestCTEDelSpillFile` 和 `TestCTEIssue49096` 分别固定 spill 结果、panic 错误传播、存储清理及 cleanup 不死锁意图；Rust 源码中的同名 failpoint 与 scope/file cleanup 对齐这些边界。
- Rust JOIN、集合、聚合和窗口是基于 `Option<String>` 行映射的同步解释执行，不是 Go 的 chunk/volcano executor pipeline。hash 等值快路和流式 CROSS JOIN只是局部性能接线，不能据此推断与 Go 优化器或并发执行器完全等价。

## 扩展指南

- 新增表源种类：在 `execute_insert_select_table_source_with_outer` 接线，并同步检查限定名、视图/CTE 优先级、事务 overlay、结果字段推导和 `collect_physical_table_sources`。测试放在独立 `pkg/session/runtime/source_test.rs` 或 `pkg/session/runtime_test/*.rs`。
- 新增 JOIN 能力：保持 `insert_select_join_columns`、`merge_insert_select_rows`、NULL 扩展及歧义 marker 一致；hash 快路必须始终由完整 ON 复核。重点覆盖 NULL 键、重复列、LEFT/RIGHT、LATERAL 与 outer shadowing。
- 新增集合算子语义：以列序行向量定义相等性，并分别验证 DISTINCT 与 ALL 的重复计数；同步检查 AST 运算符索引和外层 ORDER/LIMIT。
- 修改 CTE：保持 scope 在错误路径也 pop、递归成员只读 delta、UNION DISTINCT 跨轮去重、列 marker 随重命名、spill 后完整历史可重建。应同步 Rust 独立测试和 `pkg/executor/test/cte/cte_test.go` 所表达的 Go 边界。
- 修改 SELECT 阶段：必须保留 SQL 次序，尤其 WHERE 在分组前、HAVING 在投影/聚合上下文、DISTINCT 在最终 LIMIT 前。扩展类型比较时同步审查 `runtime/query.rs` 的表达式类型和 marker。
- 性能风险集中在全量物化、非等值嵌套循环 JOIN、排序/分组/DISTINCT 和 spill 全量回读；正确性风险集中在列歧义、NULL/三值逻辑、相关外层遮蔽和 CTE 迭代；兼容风险集中在字符串化行模型、排序规则、错误文本和 Go pipeline 的顺序差异。

## 验证依据

- RustCodeGraph：`status` 报告索引含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/session/runtime/source.rs` 确认目标文件已索引且有 57 个符号；`query execute_insert_select_query_with_outer`、`query execute_recursive_cte`、`query execute_insert_select_table_source_with_outer` 定位到本文对应实现。`explore` 与 impl 内函数的 `callers/callees` 本次未返回内容，因此调用边以精确仓库搜索补充，没有据此虚构图关系。
- 阅读的生产证据：`pkg/session/runtime/source.rs` 全部 2,089 行；`pkg/session/runtime.rs` 的模块装配和共享 import；`pkg/session/runtime/session.rs:616-644` 的 CTE/disk 状态；`pkg/session/runtime/query.rs` 的调用点与中间行模型；`pkg/session/Cargo.toml` 的 crate、feature 和依赖。目标包未发现 `doc.go`。
- 精确调用边搜索：对 `collect_physical_table_sources`、`result_set_node_has_derived_source`、`stale_source_profile`、`is_scalar_count_non_null_constant`、`execute_insert_select_query(_with_outer)`、`execute_insert_select_node_with_outer`、`execute_recursive_cte`、`execute_with_clause` 和 `visit_insert_select_source` 执行 `rg`，结果覆盖 `query.rs`、`dispatch.rs`、`control.rs`、`dml.rs`、`explain_*`、`system_query.rs` 与 `select_into.rs`。
- 阅读的 Rust 测试：`pkg/session/runtime/source_test.rs`；`pkg/session/lib.rs:178-179` 确认其作为独立 `cfg(test)` 模块编译。该测试验证派生集合源上普通及聚合 DISTINCT 在最终 LIMIT 之前生效。
- 阅读的 Go 对照：`pkg/executor/cte.go` 的 `CTEExec`、`cteProducer::genCTEResult`、seed/recursive 迭代与 spill/failpoint；`pkg/executor/test/cte/cte_test.go` 的 cleanup、spill、panic 和磁盘文件回归。未找到同路径 Go 文件，因此未作逐函数映射。
- 按任务约束未运行 Cargo。结构验证使用任务指定命令；交付前另检查目标文档恰有 11 个固定二级标题、仅目标文档与编号任务删除属于本会话、`plan.md` 未修改，并人工复核所有行为结论均可回到上述符号、调用点或测试。
