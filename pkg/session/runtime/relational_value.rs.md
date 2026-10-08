# `pkg/session/runtime/relational_value.rs`

## 文件定位

`relational_value.rs` 是 `astersql-session` crate 内部“窄关系运行时”的值层实现。模块由 `pkg/session/runtime.rs:81` 私有声明，并由 `runtime.rs:172` 通过 `use relational_value::*` 提供给同一运行时的查询、DML、系统查询和解释计划代码；只有 `relational_compare` 与 `relational_window_value` 在 `runtime.rs:82` 被提升为 `pub(crate)`，其余 API 大多限制为 `pub(super)`。

它不等同于完整的 TiDB planner/executor 表达式引擎。`pkg/session/runtime.rs` 的模块注释把这一层定义为基于规范 parser、Domain 与 KV 接口的可执行会话运行时，用于 ConcreteSession/Go testutil 流程；本文件在该边界内直接处理以 `HashMap<String, Option<String>>` 表示的已解码关系行。

crate 归属由 `pkg/session/Cargo.toml` 的 `[package] name = "astersql-session"` 和 `[lib] path = "lib.rs"` 确认。与本文件直接相关的依赖包括 `astersql-parser-ast`、`astersql-parser-mysql`、`astersql-parser-types`、`astersql-meta-model`、`astersql-types`、`astersql-expression`、`astersql-expression-exprstatic`、`astersql-util-collate`、`chrono`、`serde_json` 与 `rust_decimal`。`nextgen` feature 没有在本文件形成条件编译分支；唯一条件编译项是测试模块 `#[cfg(test)] #[path = "relational_value_test.rs"] mod tests`。

## 核心职责

1. 对 parser AST 的标量表达式递归求值：字面量、列、逻辑/比较/算术运算、行比较、`IN`、`BETWEEN`、`LIKE`、`CASE`、`CAST` 和一组标量函数，入口是 `relational_expression_value`。
2. 保留 MySQL/TiDB 的关键值语义：三值逻辑、`NULL` 传播、字符串与数字混合比较、BIT/二进制显示、排序规则、时间、JSON、VECTOR FLOAT32 和除法精度。
3. 完成关系结果后处理：解析 `ORDER BY`/`GROUP BY` 的别名与位置引用、排序关系行、推导投影字段类型、计算窗口值并输出最终投影。
4. 为少数运行时专用路径提供桥接：`ON DUPLICATE KEY UPDATE` 的新旧行合并、`embed_text` AST 重写、向量距离的 EXPLAIN 文本以及物化视图构建期间的除法精度上下文。

该文件当前不是桩：`pkg/session/runtime/query.rs:3190-3191,3330` 将排序和投影接入普通关系 SELECT；`pkg/session/runtime/source.rs:1694,1705` 将排序/分组解析接入 INSERT SELECT；`pkg/session/runtime/system_session.rs:1119` 将定义时的除法精度接入物化视图构建。

## 主要符号

### 状态与基础值语义

- `BuildDivisionPrecision(Option<i32>)`：RAII guard。`enter(precision)` 只接受 `0..=30`，把新值写入线程局部 `BUILD_DIVISION_PRECISION` 并保存旧值；`Drop` 恢复旧值，因而支持嵌套和错误返回。
- `relational_truth` / `relational_boolean_value`：在 `Option<String>` 表示法与 SQL 三值布尔之间转换；`NULL` 保持 `None`，真/假输出为 `"1"`/`"0"`。
- `relational_compare`：公共比较内核。优先比较两个向量，再比较整数（含 `0x...` BIT 和控制字符字节表示），再走浮点/数字前缀，最后退化为字符串顺序。
- `relational_expression_compare`：结合 AST 类型线索，在字符串/非字符串混合时选择 DOUBLE 数值上下文；向量仍优先使用向量比较。
- `relational_numeric_prefix`、`relational_integer_value`、`relational_float_string`：实现宽松数值前缀、无损 BIT 整数解释以及 NaN/无穷输出约定。
- `parse_runtime_datetime`、`runtime_time_components`、`format_mysql_json`、`relational_hex_value`：集中处理时间、JSON 与二进制/十六进制的运行时文本格式。

### 表达式求值

- `relational_expression_value(expression, row)`：约 1,500 行的主分派器，返回 `SessionResult<Option<String>>`。`None` 是 SQL `NULL`，错误是不能继续执行的解析、范围或未支持语义。
- `relational_table_expression_value(expression, row, table)`：在主分派器之外加入 `TableInfo`，为比较、`IN`、`LIKE` 使用列类型和 collation；不能或不需要利用列元数据时回退到通用求值器。
- `relational_row_items`：穿过括号和 `COLLATE` 提取行构造器，供行比较使用。
- `relational_numeric_value`：把表达式结果收窄到 `i128`；完全不含数字的字符串按零处理，含数字但不是合法整数则报错。
- `relational_like` / `relational_like_with_escape`：大小写不敏感的轻量 wildcard 状态机；`%` 匹配任意序列，`_` 按 Unicode 标量匹配一个字符，转义符保护通配符。
- `relational_on_duplicate_value`：先用当前目标行建立环境，再以 `entry.or_insert` 补充 incoming/source-only 列；向量 `+/-/*` 单独处理，其余委托本文件或 `crate::dml_runtime::EvalExpr`。
- `relational_expression_value_with_embed`：先探测 `embed_text`，只有存在时才克隆并重写 AST；`IF` 只递归访问选中分支，避免执行未选中的嵌入调用。

### 关系行后处理

- `sort_relational_rows`：预先计算每行 ORDER key，再稳定地逐 key 比较；升序时 `NULL` 在前，降序反转比较结果。
- `resolve_select_order_by`：将 ORDER BY 别名、无歧义投影列名和一基位置编号解析为投影表达式；越界/零位置返回 planner 1054 风格错误。
- `resolve_select_group_by`：解析 GROUP BY 一基位置；禁止位置引用落到聚合表达式并返回 planner 1056 风格错误。
- `relational_expression_type` / `relational_expression_result_field`：从 AST 和 `TableInfo` 推导输出 `FieldType`，并构造 `ConcreteResultField` 元数据。
- `relational_window_value`：按命名 spec 解析分区、ROWS frame 和函数；支持 `row_number`、`rank`、`dense_rank`、`sum`、`count`、`min/max(_count)`、`first/last/nth_value`、`lag/lead`、`var_pop`。
- `project_relational_rows`：展开 wildcard、检查隐藏列和 `_tidb_rowid`、建立标题/投影/字段元数据，并逐行执行普通表达式、窗口表达式或直接列投影。
- `relational_count_extrema`：用统一比较契约统计最小值或最大值的同值个数，计数使用 wrapping add。

### VECTOR 与辅助检查

- `explain_vector_number`、`explain_vector_literal`、`explain_vector_distance_projection`：生成受控长度的向量 EXPLAIN 表达。
- `relational_expression_is_vector`：识别向量构造、向量返回函数和 VECTOR cast，排除维数/距离/norm 这类标量结果。
- `expression_contains_function`、`expression_contains_cast`：递归检查部分 AST 形态，供上游决定特殊执行路径；当前并不遍历所有 ExprKind。

## 执行流程

### 普通关系 SELECT

1. `pkg/session/runtime/query.rs` 扫描并解码出 `Vec<RelationalRow>`；其中每个元素包含行标识与字符串化列映射。
2. 若 ORDER BY 未由索引扫描满足，`resolve_select_order_by` 把别名/位置变成真实表达式，`sort_relational_rows` 计算 key 并排序（`query.rs:3190-3191`）。
3. 聚合专用快路径可直接调用 `relational_count_extrema`；普通结果进入 `project_relational_rows`（`query.rs:3330`）。
4. `project_relational_rows` 先建立输出列名和 `ConcreteResultField`，再按投影种类调用直接列读取、`relational_expression_value_with_embed` 或 `relational_window_value`。
5. 表达式求值递归下降：子表达式先返回 `Option<String>`，当前节点按 SQL `NULL` 和错误规则组合；最终 `None` 转为 `SHOW_NULL_CELL`，二进制/ BIT 值按显示契约转换。

### 标量求值分派

`relational_expression_value` 的主要分支顺序具有语义意义：

- 值、列、括号/显式 COLLATE 先处理；未知列立即产生 `Unknown column`。
- 一元和布尔运算实现三值逻辑；`AND` 遇 false、`OR` 遇 true 可确定结果，但当前实现仍先求值左右两侧。
- `<=>` 单独提供 NULL-safe equality；行比较要求同列数并逐项短路，任何未决 NULL 返回 NULL。
- 普通比较通过 `relational_expression_compare` 选择向量、整数、浮点、数字前缀或字符串比较。
- 算术对整数溢出、除零、向量维数/运算错误分别处理；`/` 在物化视图 guard 生效且输入非科学计数法时使用 `MyDecimal` 与 half-up rounding，否则走 `f64`。
- `IN`、`BETWEEN`、`LIKE`、`CASE`、`CAST` 保留各自 NULL 传播；表扫描若调用 `relational_table_expression_value`，字符串列比较改用列 collation。
- 函数分派覆盖条件/字符串/数值/日期时间/JSON/VECTOR 等子集；少数函数（如 `elt`、`lpad`、`timestampadd`）委托 `dml_runtime::EvalExpr`，DATE_ADD/SUB 则构造 canonical expression 函数求值；未知函数明确报 `unsupported scalar function`。

### 窗口与投影

`relational_window_value` 先解析 named/ref spec，再为当前行计算 partition key 并扫描全部行形成分区。ROWS frame 将前后界限转换为半开区间；无 frame 时，无 ORDER BY 使用整个分区，有 ORDER BY 使用分区开头到当前行。函数随后只在该 frame 或分区位置上计算。调用方必须已经按窗口 ORDER 要求提供合适的行序；本函数本身不重排 partition。

`relational_expression_value_with_embed` 的两遍 visitor 是投影阶段的特殊流程：第一遍确认是否含 `embed_text`，第二遍执行选中 IF 分支中的嵌入函数并把返回值改写为 AST literal/NULL，最后交回普通求值器。

## 数据与状态

- 关系值统一是 `Option<String>`：`None` 表示 SQL NULL；布尔是 `"1"`/`"0"`；JSON、时间、DECIMAL、VECTOR 和二进制也先以文本/私有 marker 承载。该设计使本文件必须显式恢复类型语义。
- `RelationalRow` 的值映射以规范化列名为 key。`relational_expression_column` 优先匹配 `table.column`，再匹配裸列名；`project_relational_rows` 同时维护 `TableInfo.Columns` 来检查隐藏列、BIT 和输出元数据。
- BIT 在内部可用固定宽度 `0x...` 保存字节。比较时 `relational_integer_value` 按无符号大端数解释，`HEX()` 去掉列值的私有前导零，最终直接列投影将偶数长度 hex 恢复为字节显示。
- `BUILD_DIVISION_PRECISION` 是 `thread_local Cell<Option<i32>>`，不是进程全局锁。guard 保存上一层值，析构时恢复；只有 `/` 分支读取它。
- JSON 求值在字符串、`serde_json::Value` 与 AsterSQL BinaryJSON 之间转换，最终由 `format_mysql_json` 输出 MySQL 风格的逗号空格格式。
- 窗口求值每次为当前行重新扫描行集合并构造 partition/frame，没有跨行缓存；排序同样装饰并克隆整批行。这些是扩展时需要关注的主要时间/内存成本。

## 依赖与调用关系

### 上游调用者

- `pkg/session/runtime/query.rs`：普通 SELECT 的 ORDER BY、聚合 extrema、窗口/投影主调用点。
- `pkg/session/runtime/source.rs`：INSERT SELECT 的 ORDER BY/GROUP BY 解析及窗口相关表达式计算。
- `pkg/session/runtime/dml.rs`：DML 表达式、候选行排序和嵌入文本参数求值。
- `pkg/session/runtime/system_query.rs`：系统表/系统命令的常量和行表达式求值。
- `pkg/session/runtime/explain_select.rs`：常量折叠与向量距离展示。
- `pkg/session/runtime/control.rs`：控制语句中的表达式值。
- `pkg/session/runtime/system_session.rs`：物化视图构建用 `BuildDivisionPrecision::enter`。
- `pkg/session/runtime.rs:1495`：简单 WHERE 的 LIKE 分支复用 `relational_expression_value`。

RustCodeGraph 的 `query` 能定位上述核心符号与 `runtime.rs:82` 的再导出，但本次 `callers/callees` 对这些递归/泛型函数未返回有效业务边，只误报 parser AST 的 `pub` property；因此业务调用边以上述真实源码引用为准，不能把空图结果解释为“无调用者”。

### 下游依赖

- parser：`ast::ExprNode`、`ExprKind`、window/frame/field AST 与 MySQL type/flag。
- metadata：`astersql_meta_model::TableInfo`/`ColumnInfo` 提供列类型、collation、隐藏列与表身份。
- types/collation：VECTOR 运算、MyDecimal、日期时间、BinaryJSON、datum 转换与 collator wildcard。
- expression：JSON builtin、DATE_ADD/SUB 的 canonical function factory 和 evaluation context。
- session 内部：`crate::dml_runtime::EvalExpr`、`runtime_value_to_datum`、`display_runtime_value`、`SessionError`、`ConcreteResultField` 等由 `use super::*` 引入。

## 错误处理与边界

- 所有公开求值入口返回 `SessionResult`；底层解析错误通过 `session_error(context, error)` 增加动作上下文，协议式错误则直接构造 `SessionError`。
- SQL NULL 与错误严格分离：除零、JSON path 无结果、无效日期的部分函数和空窗口聚合可能返回 `Ok(None)`；未知列、整数/DECIMAL 溢出、VECTOR 维数/转换错误、非法 frame offset 和未支持函数返回 `Err`。
- 三值逻辑保留 UNKNOWN：比较任一普通操作数为 NULL 通常返回 NULL；`<=>` 永不返回 NULL；`IN` 未命中但候选含 NULL 时返回 NULL。
- 行比较和 row IN 强制左右列数一致，否则返回 `Operand should contain the same number of columns`。
- `BuildDivisionPrecision::enter` 拒绝小于 0 或大于 30；guard 的 Drop 保证错误路径也恢复旧值。
- `relational_like_with_escape` 的轻量 fallback 使用 Unicode scalar 而非 grapheme cluster；大小写转换也不是完整列排序规则。表列 LIKE 通过 `relational_table_expression_value` 的 collator path 获得更准确的列语义。
- 时间解析只覆盖源码列出的格式；`EXTRACT(WEEK ...)` 当前固定返回 0，这是当前实现事实，不应描述为完整 MySQL WEEK 语义。
- `nth_value` 对参数求值错误使用 `.ok()` 后退为 0，`lag/lead` 只实现默认偏移一行，`RANGE` frame 不走显式 frame 分支；这些都是当前窗口子集的边界。
- `SLEEP` 会真实阻塞当前 OS 线程；负数或非有限值报错。
- 未识别的 AST/函数不会静默成功，分别报 `unsupported relational scalar expression` 或 `unsupported scalar/window function`。

## 并发与资源生命周期

- `BuildDivisionPrecision` 的状态按线程隔离，`Cell` 不跨线程共享；其注释假定池化 session 在各自线程同步执行。RAII 析构恢复嵌套上下文，无需 mutex，但 guard 不能替代异步 task-local 状态。
- 本文件没有启动异步任务、线程池、通道或事务，也不持有 KV 资源；借用的 AST、行映射、`TableInfo` 与 window specs 都只在同步调用期间有效。
- `SLEEP` 是唯一主动等待资源的路径，调用 `std::thread::sleep`，会占用会话线程直至结束。
- 排序先克隆全部 `RelationalRow` 并保存所有 key；投影创建完整二维结果；窗口函数按输出行重复构造 partition/key。大结果集可能产生 O(n) 额外内存，窗口路径按函数和 key 求值成本可接近 O(n²)。
- `embed_text` 以同步闭包借用传入，不被保存；visitor 将第一个错误保存到 `Option<SessionError>`，遍历结束后传播。

## 与 Go 版本的对应关系

仓库中没有 `pkg/session/runtime/relational_value.go`，因此不存在逐函数的一对一 Go 文件。Rust 文件把 Go 主线中分散的能力聚合到了 ConcreteSession 的窄运行时：

- 通用表达式与 builtin 语义对应 `pkg/expression` 的 `Expression.Eval*`、`EvalBool`、各 `builtin_*.go` 以及类型化 eval context；例如 Go 的 DECIMAL 除法在 `pkg/expression/builtin_arithmetic.go` 使用 `ctx.GetDivPrecisionIncrement()`。
- `BuildDivisionPrecision` 对齐 Go eval context 暴露的 `GetDivPrecisionIncrement`/`WithDivPrecisionIncrement`（`pkg/expression/exprstatic/evalctx.go`），但这里用线程局部 guard 把物化视图定义精度送入窄求值器；接线点是 `system_session.rs:1119`。
- 投影职责在 Go 主线由 `pkg/executor/projection.go` 的 `ProjectionExec` 和 `expression.EvaluatorSuite` 承担；窗口由 `pkg/executor/windows` 与 aggfuncs 承担。Rust 本文件是字符串行上的同步局部实现，不具备 Go ProjectionExec 的 chunk/vectorized/parallel pipeline。
- 排序规则比较对应 Go 中 datum/type 比较时传入 `collate.GetCollator(...)` 的模式；Rust 的表感知入口显式从 `ColumnInfo.GetCollate()` 取 collator。
- Rust 使用相同的 parser AST、MySQL type、decimal、JSON、time 和 vector 移植 crate，以复用语义实现；但本文件仅支持源码枚举的 AST 与函数子集。不能由“名称相同”推断完整 Go 功能等价。

独立 Rust 测试 `pkg/session/runtime/relational_value_test.rs` 目前只直接验证 LIKE：下划线按一个 Unicode 字符匹配，以及反斜杠转义 `_`/`%`。更广泛的行为证据来自上游 runtime 测试和 Go 的 expression/executor 测试，而不是本文件测试中的完整逐函数覆盖。

## 扩展指南

1. 新增 AST 运算符或标量函数时，优先在 `relational_expression_value` 增加窄而明确的分支；若已有 canonical expression 或 `dml_runtime::EvalExpr` 实现，应复用它，避免再次近似 Go 语义。
2. 涉及字符串列比较、IN 或 LIKE 时，同时审查 `relational_table_expression_value`，确保 collation 不会因通用字符串 fallback 丢失；类型返回值还要同步 `relational_expression_type`。
3. 新增投影表达式返回类型时同步 `relational_expression_result_field` 的上游消费者；新增 ORDER/GROUP 引用规则时同步 `resolve_select_order_by`、`resolve_select_group_by` 和 source/query 两条调用链。
4. 新增窗口函数时明确它使用 partition 还是 frame、NULL 是否跳过、空 frame 返回值及 ORDER 前置条件；如要支持 offset/default、RANGE/GROUPS frame，应扩充 AST 参数和 frame 计算，不能套用现有默认一行实现。
5. 修改二进制/BIT/JSON/VECTOR 文本表示时，必须成对检查编码、比较、HEX/CAST、投影显示和结果字段类型，避免私有 marker 泄漏到 SQL 输出。
6. 修改 `/` 或物化视图定义求值时，保持 `BuildDivisionPrecision` 的 `0..=30` 验证与 Drop 恢复，并对照 Go `builtin_arithmetic.go`、eval context 及 `pkg/executor/test/executor/executor_test.go:2486` 的精度意图。
7. 测试必须保留在独立文件 `pkg/session/runtime/relational_value_test.rs`，不要内嵌到生产文件。当前直接测试覆盖很窄；至少为新增分支补充 NULL、错误、边界数值、Unicode/collation 与 Go 对照案例。若行为通过 query/source 接线触发，还应在相邻独立 runtime 测试中加入端到端断言。
8. 性能敏感扩展要避免在逐行窗口或投影内重复解析同一 AST/JSON/VECTOR；如引入缓存，应明确其 statement/session 生命周期，不能放入无边界全局状态。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/session/runtime/relational_value.rs` 已索引，共 3,319 行、355 个符号。
- RustCodeGraph 源码读取：按 `node --file pkg/session/runtime/relational_value.rs` 分段读取 1-3319 行，核对模块常量、类型、函数、impl 和测试条件编译项。
- RustCodeGraph 符号查询：查询了 `relational_expression_value`、`relational_table_expression_value`、`sort_relational_rows`、`resolve_select_order_by`、`resolve_select_group_by`、`relational_window_value`、`project_relational_rows`、`BuildDivisionPrecision`；图的 caller/callee 结果对这些符号不完整，调用关系改由下列源码引用验证。
- 模块与调用路径：`pkg/session/runtime.rs:81-82,172,1495`，`pkg/session/runtime/query.rs:3190-3191,3330`，`pkg/session/runtime/source.rs:1694,1705`，`pkg/session/runtime/system_session.rs:1119`，以及 `runtime/{control,dml,explain_select,system_query}.rs` 中的直接调用点。
- crate 边界：`pkg/session/Cargo.toml` 的 package/lib/feature/依赖声明；目标文件没有 feature 分支。
- 独立 Rust 测试：`pkg/session/runtime/relational_value_test.rs` 的 `like_underscore_matches_one_unicode_character` 与 `like_backslash_escapes_wildcards`。
- Go 对照：不存在同路径 Go 文件；读取/搜索了 `pkg/expression/exprstatic/evalctx.go`、`pkg/expression/builtin_arithmetic.go`、`pkg/executor/projection.go`、`pkg/executor/windows/` 与 `pkg/executor/test/executor/executor_test.go:2486` 的对应职责和精度契约。
- 人工复核结论：该文件存在于 ConcreteSession 的窄关系执行链中；主运行路径是“解码关系行 → 解析排序/分组引用 → 标量/窗口求值 → 投影和字段元数据”；安全扩展必须同步值语义、表 collation、类型推导、独立测试和上游接线。
