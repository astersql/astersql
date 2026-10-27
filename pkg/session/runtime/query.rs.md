# `pkg/session/runtime/query.rs`

## 文件定位

`execute_insert_select` 先执行查询子树，再构造 values 形式的 `ast::InsertStmt`，最终复用 `execute_relational_insert` 的默认值、冲突处理、约束与原子写入逻辑。

## 核心职责

1. 识别与求值关系表达式。`relational_query_expression_value` 处理列、变量、聚合、标量/比较/IN/EXISTS 子查询、`CASE`、类型转换、函数、行比较及三值逻辑，并把未特化的表达式交给父模块的 `relational_expression_value`。
2. 执行聚合和窗口。`relational_aggregate_value` 实现 `count`、`sum`、`avg`、`min/max`、`bit_xor`、`group_concat`、`any_value` 等；`relational_window_query_expression_value` 将窗口节点交给 `relational_window_value`，同时允许 `coalesce`/`ifnull` 和 `CASE` 嵌套窗口结果。
3. 形成查询结果及元数据。`relational_query_node_result_fields` 从物理表、虚拟系统表、派生表、CTE、通配符和表达式推导 `ConcreteResultField`；`execute_relational_query_record_set` 将内部 `InsertSelectRows` 转换为 `ConcreteRecordSet`，并保留空结果的列元数据。
4. 执行普通单/双表 `SELECT`。`execute_relational_select_rows` 完成权限检查、表解析、stale read、索引/主键/ANN 访问选择、扫描、过滤、锁、索引使用统计、排序、聚合、去重、LIMIT 和投影。
5. 为复杂查询提供通用入口。`relational_select_requires_full_query` 判断 CTE、派生表、多源、视图、分组、HAVING、聚合、子查询、校验和与 JSON/时间表达式是否必须进入 `source.rs` 的完整查询路径；`execute_full_relational_select` 临时安装语句快照并恢复会话状态。
6. 连接查询与写入。`execute_insert_select` 先执行查询子树，再构造 values 形式的 `ast::InsertStmt`，最终复用 `execute_relational_insert` 的默认值、冲突处理、约束与原子写入逻辑。

## 主要符号

- `RelationalRow = (i64, HashMap<String, Option<String>>)`（第 58 行）：内部行携带 handle 和“列名到可空字符串值”的映射。SQL `NULL` 用 `None` 表示，传给客户端时由 `execute_relational_query_record_set` 转为 `SHOW_NULL_CELL`。
- `InsertSelectRows`（第 62 行）：保存有序输出列和按名取值的行，供派生表、集合操作、子查询和 `INSERT ... SELECT` 共享。
- `RelationalLimitWindow { offset, count }`、`relational_limit_window`、`execute_relational_limit`（第 655、661、694 行）：解析字面量 LIMIT/OFFSET，并通过 `astersql_executor::select::ExecuteLimitValues` 使用规范 Limit 执行逻辑。
- 四类隐藏 marker（第 67、140-142 行）：以 NUL 前缀在行映射中标记字符串、歧义列、FLOAT32 和 VECTOR 类型。对应构造器及 `relational_expression_is_string` 让字符串比较、聚合和类型兼容不依赖额外行结构。
- AST 检测/收集器（第 265-653 行）：`relational_expression_has_aggregate`、`*_has_subquery`、`*_has_window`、`*_has_tidb_row_checksum`、`*_has_json_temporal` 递归识别表达式能力；`collect_join_using_column_names` 和 `collect_expression_column_names` 为元数据消歧、索引使用统计收集列名。
- `relational_query_expression_value`（第 910 行）：普通表达式的核心递归解释器；会读取或写入会话变量，并在子查询、聚合和警告场景访问 `ConcreteSession.state`。
- `filter_relational_query_rows`（第 1540 行）：无聚合/子查询时走表感知的快速表达式求值，否则走上述通用解释器；只有 SQL 真值严格为 `Some(true)` 的行被保留。
- `relational_select_requires_full_query`（第 1603 行）：紧凑单表路径与完整查询路径之间的能力门。
- `full_query_statement_read_ts`、`execute_full_relational_select`（第 1771、1821 行）：验证 AS OF 来源一致性、限制事务内 stale read，并临时设置/恢复 `snapshot_read_ts`。
- `relational_query_node_result_fields`（第 2004 行）：解析列名、别名、通配符、`USING` 消歧、`_tidb_rowid`、CTE/派生表兜底类型及表达式类型。
- `execute_relational_query_record_set`（第 2226 行）：完整查询节点到带字段元数据结果集的适配器。
- `execute_insert_select`（第 2271 行）：查询输出到普通 INSERT 执行器的适配器，并用 `binary_runtime_bytes`/`HexValue` 保持二进制字节。
- `execute_relational_select`、`execute_relational_select_rows`（第 2334、2345 行）：普通关系查询的公开入口与主体实现；前者还把 `sql_killer` 绑定到结果集的 store read 生命周期。

## 执行流程

### 语句分派

`ConcreteSession::execute_statement` 在 `pkg/session/runtime/dispatch.rs:3173-3230` 先处理集合操作、系统表和专门查询。普通 `SELECT` 经过 MDL、分组合法性和 SELECT INTO 检查后：

1. `relational_select_requires_full_query` 为真时，调用 `execute_full_relational_select`，随后经 `execute_relational_query_record_set -> execute_insert_select_node -> source.rs::execute_insert_select_node_with_outer` 递归执行复杂查询树。
2. 否则调用 `execute_relational_select`；如果本文件不能处理（例如没有 FROM），返回 `Ok(None)`，分派器继续尝试常量查询或 KV 兼容路径。

RustCodeGraph 的节点证据显示，`execute_relational_select` 定义于本文件第 2334 行，调用者位于 `dispatch.rs:3190`；`execute_relational_query_record_set` 定义于第 2226 行，调用者位于 `dispatch.rs:3176` 和本文件派生表分支第 2381 行。

### 普通 SELECT

`execute_relational_select_rows` 的主流程如下：

1. 收集物理表源并拒绝同一语句混用当前读与 AS OF 读；检查每个表的 `SelectPriv`。
2. 消费 `pending_stale_read_ts` 或计算 AS OF 时间戳，按历史 catalog version/当前 MDL snapshot 解析表；本地临时表优先。
3. 对 point equality 更新 chunk allocator 策略，验证显式向量索引与 ORDER BY 距离度量匹配；解析右表和 LIMIT。
4. 收集谓词列并判定 READ COMMITTED、锁类型、主键范围、二级索引和 index merge；用 `record_select_request` 记录选择的 `TableReader`、`IndexLookup` 或 `IndexMerge` 路径。
5. 仅在无窗口、无分区/连接/锁/分组/HAVING/DISTINCT 等条件下推 LIMIT；标量非空常量 `COUNT` 可直接走计数；满足 TiFlash、向量索引、L2 升序和 LIMIT 等条件时可走 ANN TopK。
6. 按隔离级别和访问路径扫描；再应用显式分区过滤、最多一个右表的行连接及 WHERE。普通过滤通过 `filter_relational_query_rows` 保持 SQL 三值逻辑。
7. 对锁定读重新读取最新值并叠加事务本地变更，构造行键与唯一键，执行 NOWAIT/WAIT 锁；加锁后再次扫描。乐观事务则把读取行写入事务缓冲，并记录冲突键和内存统计。
8. 更新 `RUNTIME_INDEX_USAGE`；必要时排序；处理全聚合和 COUNT 快路径；最后按正确阶段执行 LIMIT、投影、窗口和 DISTINCT，返回带 `ConcreteResultField` 的结果集。

### 完整查询、子查询与 INSERT SELECT

复杂查询在 `source.rs:2065` 递归执行，但表达式和结果封装仍回调本文件。标量子查询由 `execute_relational_subquery` 执行，超过一行报错；比较子查询、IN 和 EXISTS 明确传播空集与 NULL 的三值语义。相关子查询可由 `execute_insert_select_node_with_outer(..., Some(outer))` 取得外层行。

`execute_insert_select` 验证目标和 SELECT 子树，核对显式目标列数，将每行按输出列顺序转成值表达式；二进制运行时编码转换成 hex literal，其余值使用 `utf8mb4_bin`。随后 `PlanInsert` 重新规划为普通 values INSERT，并调用 `execute_relational_insert(..., true, Some(&select_columns))`，因此不会在查询模块重复实现冲突与约束逻辑。

## 数据与状态

- 行值采用 `Option<String>`，类型语义通过表元数据和隐藏 marker 补足。marker 与普通列共存于 `HashMap`，投影时不会暴露给客户端。
- `ConcreteSession.state` 是 `RefCell` 风格的会话可变状态。本文件会读取/修改用户变量、字符串变量集合、当前 warnings、事务、隔离级别、pending/snapshot/transaction stale read 时间戳、锁相关表、乐观锁冲突键及事务内存统计。
- `full_query_statement_read_ts` 消费一次性的 `pending_stale_read_ts`；`execute_full_relational_select` 保存旧 `snapshot_read_ts`、临时安装语句时间戳，并在执行后恢复。恢复发生在返回结果前，错误结果也会经过恢复语句。
- 结果字段由 `ConcreteResultField` 持有列、输出别名、原表/别名和数据库；表达式列通过源表字段推导类型，无法证明的 CTE/派生表列保守使用 `utf8mb4_bin` 的 `TypeVarString`。
- `RUNTIME_INDEX_USAGE` 是跨会话共享的互斥映射。键包含 domain、table 和 index ID；每次命中更新最近访问时间、查询/KV 请求/访问行数及访问比例桶。
- `projection_original_text` 清理投影原文中的版本注释，确保无显式别名时的结果列名接近 MySQL/TiDB 展示语义。

## 依赖与调用关系

上游直接调用关系：

- `pkg/session/runtime/dispatch.rs:3176`：带表的集合操作调用 `execute_relational_query_record_set`。
- `pkg/session/runtime/dispatch.rs:3224-3228`：SELECT 先依据 `relational_select_requires_full_query` 选完整或普通路径。
- `pkg/session/runtime/control.rs:2105-2107`：INSERT 分派对带 SELECT 的语句调用 `execute_insert_select`。RustCodeGraph 同样给出 `execute_insert` 是该入口的调用者。
- `pkg/session/runtime/source.rs:365,1001-1332,2065`：派生表、集合操作、CTE/递归 CTE 和连接通过 `execute_insert_select_node_with_outer` 进入完整查询执行；本文件的子查询也反向调用它。
- `pkg/session/runtime/system_query.rs:20` 使用 `format_unix_timestamp`；`pkg/session/runtime/source.rs:738-739` 使用字符串类型识别。

主要下游依赖：

- parser/AST：`astersql-parser-ast` 的 `SelectStmt`、`ExprNode`、`ResultSetNode`、`SetOprStmt`、`InsertStmt` 和锁类型决定控制流。
- executor：`astersql_executor::select::ExecuteLimitValues` 执行 LIMIT；`ConcreteRecordSet` 承载结果。
- planner：`astersql_planner_core::ShouldSkipReuseChunkForPointGet` 控制 PointGet chunk 复用；访问范围、二级索引与 index merge 的具体辅助函数来自同一 runtime 父模块。
- metadata/types：`astersql-meta-model`、`astersql-parser-types` 和 `astersql-types::metadata` 提供表、列、索引、向量与字段类型。
- 存储/事务：扫描、overlay、编码、行锁和 TSO/catalog 查询通过 `ConcreteSession` 在 `runtime.rs`、`source.rs`、`dml.rs` 等相邻实现完成。
- 外部 crate：`chrono` 格式化时间，`serde_json` 区分 JSON 字符串/时间哨兵，`rust_decimal` 保持 SUM/AVG 十进制定点计算。

`pkg/session/Cargo.toml` 将上述 crate 作为 `astersql-session` 的常规依赖；`nextgen` feature 只转发配置 feature，本文件没有 `#[cfg]` 条件编译项，因此其核心路径不受本地 feature 分叉。

## 错误处理与边界

- 所有公开执行入口返回 `SessionResult`，底层错误用 `session_error("上下文", error)` 增加操作语义；SQL 规则错误直接构造 `SessionError`。
- 列解析区分未知列、字段列表中的歧义列及带限定符列；`JOIN ... USING` 的同名列通过 `collect_join_using_column_names` 避免误报歧义。
- 标量子查询超过一行报 `Subquery returns more than 1 row`；空行或空列返回 NULL。比较、IN、BETWEEN 等在任一必要操作数为 NULL 时保留三值逻辑，而 WHERE 只接受真。
- 聚合忽略 NULL；无非空输入的 SUM/AVG/MIN/MAX 返回 NULL。VECTOR 禁止 SUM/AVG；不支持的聚合或复合 aggregate/subquery 表达式显式报错。
- LIMIT/OFFSET 必须是可转为 `usize` 的字面量；执行错误标注为 `execute relational LIMIT`。OFFSET+COUNT 使用饱和加法，ANN TopK 还检查 `u32` 范围。
- AS OF 不能与当前读混合、不能为同一查询指定不同时间、不能在事务内使用，也不能和待处理的 transaction-as-of 状态叠加。
- 锁定 stale transaction 被拒绝；FOR SHARE 受 noop/shared-lock-promotion 开关控制；NOWAIT 和 WAIT N 被传给锁管理器。缺失唯一键是否加锁由 `constraint_check_in_place_pessimistic` 控制。
- 普通快速路径仅直接处理第一、第二个物理表；更复杂来源必须由 `relational_select_requires_full_query` 转入 `source.rs`。新增语法若未更新能力门，可能误入能力不足的路径。
- `format_unix_timestamp` 对无效时间戳回退 UNIX epoch，微秒精度最多六位；这是当前明确行为，扩展时不能无意改成 panic。

## 并发与资源生命周期

本文件没有创建线程、异步任务或通道；执行是同步的会话调用。并发控制来自外部存储事务和共享状态：

- 行锁键带 `domain_id`，悲观锁通过 `acquire_row_locks` 获取；锁生命周期由会话事务管理，本文只负责选择锁键、模式和等待策略。加锁后重新扫描，避免返回加锁前的过期版本。
- 乐观锁定读不阻塞其他写者，而是把读行写入事务缓冲并登记 `optimistic_for_update_keys`，使提交时能够检测重叠写冲突。
- 语句快照是临时会话状态。完整查询保存并恢复 `snapshot_read_ts`；普通查询消费 pending stale timestamp。修改这些路径时应保证错误返回也不会泄漏快照状态。
- `RUNTIME_INDEX_USAGE.lock()` 对 poison 使用 `into_inner` 继续服务，计数使用 `saturating_add` 防溢出。
- 结果集由 `execute_relational_select` 附加 `Arc::clone(&self.sql_killer)`，让后续 store read 可观察取消信号；数据行本身当前主要在内存中物化，连接、排序、DISTINCT、窗口和部分聚合可能持有整批行。
- `INSERT ... SELECT` 先物化查询结果，再一次性交给普通 INSERT 路径；独立测试验证失败时不留下前缀行，语句级原子性由下游写入实现负责。

## 与 Go 版本的对应关系

Go 同路径实现不存在；本文件顶部模块说明和 `pkg/session/Cargo.toml` 仅声明它属于 Go `pkg/session` 的迁移范围。

- LIMIT 下推注释指向 Go `pkg/planner/core/task.go:611-627` 的 `attach2Task4PhysicalLimit`；Rust 仅在无窗口、连接、锁、分组、HAVING、DISTINCT 等安全条件下把窗口嵌入扫描。
- Rust 的 `Option<String>` 行模型、marker、内存物化连接/窗口及若干紧凑快路径是当前 Rust runtime 的实现手段，不等价于 Go 的 chunk/executor pipeline。文档只能声称已由源码和测试覆盖的 SQL 行为一致，不能声称执行架构完全一致。

独立 Rust 回归测试进一步固定迁移语义：`pkg/session/runtime_test/query.rs` 覆盖字符串/数值比较、information_schema 元数据、派生 UNION/CROSS JOIN、投影函数、INSERT SELECT 冲突与原子性、外连接降级及窗口嵌套子查询；`pkg/session/runtime/query_binary_test.rs` 固定 BLOB 经 INSERT SELECT 后的逐字节保持。

## 扩展指南

- 新增表达式种类或函数：先更新 `relational_query_expression_value`；若结果类型或路由判断受影响，同时更新 `relational_expression_is_string`、aggregate/subquery/window/JSON-temporal 检测器和 `relational_query_expression_type_from_sources`。测试应放在独立的 `pkg/session/runtime_test/query.rs` 或新增同目录 `*_test.rs`，不要内嵌到生产文件。
- 新增聚合/窗口：更新 `relational_aggregate_value` 或 `relational_window_query_expression_value`，并检查普通 SELECT 的 `all_aggregates` 快路径是否也需支持，防止完整路径和快速路径结果分叉。
- 新增查询结构：首先评估 `relational_select_requires_full_query`，再在 `pkg/session/runtime/source.rs::execute_insert_select_node_with_outer` 扩展递归数据源执行；不要把多源逻辑硬塞进只支持有限表源的 `execute_relational_select_rows`。
- 新增访问路径或下推：在选择路径、扫描调用、残余 WHERE、ORDER BY 和 LIMIT 阶段保持“只消费一次”的不变量，并同步 `record_select_request` 与 `RUNTIME_INDEX_USAGE`。重点验证事务 overlay、READ COMMITTED、分区、锁、窗口和 DISTINCT 禁止条件。
- 修改 stale read/锁：同步审查 `full_query_statement_read_ts`、普通路径的 `statement_read_ts`、`execute_full_relational_select` 的恢复逻辑以及 `source.rs` 的递归来源；验证错误返回、空锁定读、唯一键缺失、悲观/乐观事务。
- 修改结果元数据：同步更新 `append_relational_source_fields` 和 `relational_query_node_result_fields`，验证通配符、别名、JOIN USING、空结果、CTE/派生表、虚拟系统表及 `_tidb_rowid`。
- 修改 INSERT SELECT：保留二进制转 hex literal、源/目标列数检查及复用 `execute_relational_insert` 的结构，并同步 `runtime_test/query.rs` 的冲突/原子性/外连接测试与 `runtime/query_binary_test.rs`。

兼容风险主要是 MySQL/TiDB 的 NULL、类型强制、列名/字段类型和错误文本；正确性风险集中在 stale read、锁后重扫、谓词下推和复杂查询路由；性能风险集中在全量物化、笛卡尔连接、排序/DISTINCT/窗口、无效下推以及共享索引统计锁。

## PostgreSQL int4 显式转换（2026-10-24）

PG int4 Cast 消费者复用 pg_integer_relational_value。关系 SELECT 在行求值前用实际来源字段检查 int4 的输入类型，拒绝 float/numeric，零行也不能绕过；包含 CAST 谓词的 PG SELECT 路由到既有完整查询路径，避免无表快速投影忽略转换错误。MySQL 选择保持不变。


2026-10-24 int8：两个现有 Cast 求值路径按 PostgreSQLInt8 复用严格 i64 转换；行扫描前的输入族检查也覆盖 int8，嵌套逐层求值并执行中间 int4 范围检查。

PG CAST 投影也走已有完整关系查询路径，确保显式 OID 参数的执行结果具有引擎来源字段类型；避免无表快速投影缺失 native_types 而无法编码，不以 SQL 子串或值猜测 OID。

### PostgreSQL限定布尔显式转换（任务3）

PG SELECT在行求值前检查布尔显式转换资格（含空扫描）与整数WHERE拒绝；关系查询作用域使用PgCastTable传递catalog字段类型，并将显式布尔转换接到共享内核。

## PostgreSQL signed16（任务5）

PostgreSQLInt2加入现有整数转换的结果类型、返回投影、输入验证及关系求值分发，保持TypeShort和NULL身份；不扩展算术/比较重载。

## PostgreSQL 有限 numeric 承载

会话查询 CAST 消费者使用 pg_numeric_relational_value，保持 NULL 和精确 scale。

## PostgreSQL numeric 声明与舍入

PG numeric求值传入声明FieldType；SELECT准备阶段验证precision/scale，NULL与空扫描也拒绝非法声明(22023)。

## 精确数值到整数转换（任务11）

整数输入验证接收显式 numeric 表达式；不开放 float 或完整隐式转换矩阵。

## 默认数值常量（任务12）

任务12：pg_integer_literal_type 对超 i64 的无符号整数字面量返回无约束 numeric；关系查询在替换一元操作数前通过 pg_numeric_literal_value 精确求值 PG 小数/指数常量，维持 Prepare、零行和执行身份一致。


## PostgreSQL float4 文本输入（任务16）

正式关系求值中的PostgreSQLFloat4调用共享binary32转换，保持错误来源及NULL。

### 2026-10-26 浮点显式整数转换

PG float4/float8 转整数复用最近偶数舍入后检查目标有符号域；正式表达式和关系消费者共同覆盖，MySQL路径保持原逻辑。

### 2026-10-26 浮点真实客户端差分（任务20）

PostgreSQLFloat8关系查询Cast分支复用pg_float8_relational_value，与写入表达式保持相同严格解析及结构化错误。

### 2026-10-26 varchar 字符长度（任务22）

共享 `pg_varchar` 按 Unicode 标量计数；显式 CAST 裁剪，赋值仅允许额外 U+0020，类型长度域为 1..10485760。沿已有 FieldType 文本身份及错误源传递，保留 MySQL 路径。回归见 `pg_extended_test::pg_scalar_22`、`builtin_cast_test::pg_scalar_22_formal_varchar` 和 `relational_value_test::pg_scalar_22_varchar_relational`。

### 2026-10-26 定长文本（任务23）

文本 CAST 与零行/NULL 长度校验覆盖 bpchar；通过现有 PgCastTable 绑定类型判断源身份。INSERT SELECT 在构造值 AST 时保留来源 bpchar 身份，供目标赋值转换使用。

派生表 bpchar 输出以已绑定投影别名作为后续源列名，保证 `SELECT x::text FROM (SELECT v AS x ...)` 的转换使用实际 bpchar 类型。

## bytea hex 输入接线

PostgreSQL既有参数绑定使用binary CAST承载OID17。对该CAST内的HexLiteral/Bytes直接保留原始字节marker，不能使用ValueExpr.text()的0x展示字符串作为实际字节。与relational_value消费者共用binary_cast_literal_value。

## UUID显式转换局部接线

UUID cast在行求值前按来源/目标检查资格（含NULL和零行），共享求值调用pg_uuid规范转换；不依字符串外观推导身份。

## JSON显式转换与类型交接

PostgreSQL JSON显式cast委托pg_json共享转换；行求值前校验转换资格，NULL/零行同样保持约束。
