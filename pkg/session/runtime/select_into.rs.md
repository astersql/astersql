# `pkg/session/runtime/select_into.rs`

## 文件定位

本文件是 `astersql-session` crate 的 canonical session 运行时组件，模块由 `pkg/session/runtime.rs` 中的私有声明 `mod select_into;` 接入。它不是通用 executor 层的 `SelectIntoExec`，而是 `ConcreteSession` 已经把 `SELECT` 物化为 `ConcreteRecordSet` 后的终端处理器：普通查询原样返回结果集；带 `INTO OUTFILE` 的查询把结果同步写入服务端本地文件，并返回空结果集。入口是 `ConcreteSession::finish_select_into`（`select_into.rs:237`），直接上游是 `ConcreteSession::execute_statement` 的 SELECT 分派（`runtime/dispatch.rs:3186-3256`）。

crate 边界由 `pkg/session/Cargo.toml` 确认：库入口是 `lib.rs`，本模块通过父模块共享 `astersql_parser_ast`、`astersql_parser_mysql`、session 状态和结果集类型；没有本文件专属 feature gate。安全增强模式（SEM）对 `SELECT INTO` 的拒绝发生在进入本文件之前，见 `runtime/dispatch.rs:3202-3207` 和依赖 `astersql-util-sem-compat`。

## 核心职责

- `OutfileFormat::from_option`（`select_into.rs:34`）把 AST 中的 `FIELDS`/`LINES` 选项归一化为字节级写出配置；默认值为字段分隔 `\t`、转义符 `\`、行结束 `\n`，默认没有包围符。
- `OutfileFormat::escape_field`（`select_into.rs:61`）按首字节规则转义 NUL、转义符、包围符、行终止符，以及未被包围时的字段终止符。
- `OutfileFormat::null_value`（`select_into.rs:86`）输出 `<escape>N`；显式禁用转义时输出 `NULL`。
- `optionally_enclose`（`select_into.rs:93`）依据 `ConcreteResultField.column.GetType()` 判断 `OPTIONALLY ENCLOSED` 是否适用。
- `ConcreteSession::record_explain_for_plan`（`select_into.rs:116`）在写出前为 `EXPLAIN FOR CONNECTION` 记录一个简化的、观测用途的物理计划快照。
- `ConcreteSession::finish_select_into`（`select_into.rs:237`）负责类型校验、独占创建文件、逐行序列化、落盘同步、更新受影响行数报告，并吞掉原结果集以避免再向客户端发送行。

本文件不负责解析 SQL、生成关系结果、验证 SEM 权限、流式拉取 executor chunk，也不承担完整 TiDB executor 的所有类型格式化；这些边界分别位于 parser、`runtime/dispatch.rs`、查询执行路径和 `pkg/executor/select_into.rs`。

## 主要符号

- `OutfileFormat`（私有结构体，`select_into.rs:25`）：持有 `field_terminator: Vec<u8>`、单字节 `enclosure`、单字节可选 `escape`、`optionally_enclosed` 和 `line_terminator: Vec<u8>`。终止符完整写出，但转义判断只比较其首字节。
- `OutfileFormat::from_option(&ast::SelectIntoOption) -> Self`：读取 `FieldsInfo`、`LinesInfo`。`Enclosed` 和 `Escaped` 只取 UTF-8 字节串首字节；空 `Escaped` 变成 `None`。`FieldsClause::DefinedNullBy`、`NullValueOptEnclosed` 与 `LinesClause::Starting` 没有在本文件使用（AST 定义见 `pkg/parser/ast/lib.rs:3814-3850`）。
- `OutfileFormat::escape_field(&self, value: &str, enclosed: bool) -> Vec<u8>`：遍历 UTF-8 字节；NUL 改写为字符 `0` 并加转义前缀，其他命中字节保留原值并加前缀。
- `OutfileFormat::null_value(&self) -> Vec<u8>`：构造 NULL 的文件表示，不读取列类型。
- `optionally_enclose(Option<&ConcreteResultField>) -> bool`：字符串、各类 blob、日期时间、duration、JSON 返回 `true`；缺失结果字段元数据和数值类型返回 `false`。
- `ConcreteSession::record_explain_for_plan(&self, &ast::SelectStmt, &ConcreteRecordSet)`：私有辅助函数。它提取第一个物理表、简单的 `列 = 常量` 谓词和结果行数；若元数据中存在首列匹配的非唯一二级索引，记录 `IndexLookUp` 三行，否则记录一行 `Point_Get`。
- `ConcreteSession::finish_select_into(&self, &ast::SelectStmt, ConcreteRecordSet) -> SessionResult<ConcreteRecordSet>`：模块内唯一对父模块可见的 API；所有 I/O 错误都转成 `SessionError`。

本文件没有模块级常量、trait、条件编译 API 或测试模块；唯一条件编译项是 Unix 下导入 `OpenOptionsExt` 并设置文件 mode `0640`（`select_into.rs:22-23,253-254`）。

## 执行流程

1. `runtime/dispatch.rs:3186-3256` 对不同 SELECT 数据源分别构造 `ConcreteRecordSet`，但每条成功分支最终都调用 `finish_select_into`；因此 information schema、死锁历史、运行时 DDL/统计系统表、完整关系查询、简化关系查询、常量查询和 session KV 回退共享同一个 OUTFILE 收尾路径。
2. `finish_select_into` 先调用 `record_explain_for_plan`。没有可识别物理表时清空 `last_explain_for_rows`；有表时根据简单等值条件和 catalog 索引生成观测快照。`try_borrow_mut` 失败会静默跳过记录，不能把成功 SELECT 变成 `RefCell` panic（`select_into.rs:229-234`）。
3. 若 `SelectIntoOpt` 为空，函数立即返回原 `ConcreteRecordSet`，不触碰文件系统，也不改写 DML 报告（`select_into.rs:243-245`）。
4. 若类型不是 `ast::SelectIntoType::Outfile`（例如 `Dumpfile` 或变量），返回 `unsupported SelectInto type`（`select_into.rs:246-248`）。
5. 从 AST 构造 `OutfileFormat`；以读写、`create_new(true)` 打开 `FileName`。目标已存在时绝不截断覆盖；Unix 创建权限为 `0640`（`select_into.rs:250-257`）。
6. 用 `BufWriter` 包装文件，先保存原始行数。随后从 `result.rows` 头部逐行 `pop_front`，每列前（首列除外）写字段分隔符（`select_into.rs:258-265`）。
7. 值等于内部 NULL 哨兵 `SHOW_NULL_CELL`（`<nil>`）或 `CONCRETE_NULL_VALUE`（`__astersql_internal_null__`）时写 NULL 表示并直接进入下一列（`runtime.rs:283-285`、`select_into.rs:267-272`）。
8. 非 NULL 值根据包围配置和列元数据决定是否包围；写开始包围符、转义后的值、结束包围符。每行最后写完整行终止符（`select_into.rs:273-295`）。
9. 所有行写完后依次 `flush`、从 `BufWriter` 取回文件、`sync_all`，确保缓冲和文件数据同步请求完成（`select_into.rs:296-303`）。
10. 成功后将 `last_dml_report` 设置为 `Operator = "SelectInto"`、`AffectedRows =` 原始行数，其余字段默认；最后返回无列无行的 `ConcreteRecordSet`（`select_into.rs:305-310`）。`protocol_state` 会把该报告的行数暴露为协议 affected rows（`runtime/dispatch.rs:1006-1023`）。

## 数据与状态

输入 AST 是 `ast::SelectStmt`，其中 `SelectIntoOpt: Option<SelectIntoOption>` 决定是否写文件；`SelectIntoOption` 提供类型、路径、字段和行选项（`pkg/parser/ast/lib.rs:3837-3850`）。输入结果是 `ConcreteRecordSet`，其 `rows` 为 `VecDeque<Vec<String>>`、`result_fields` 为与列对齐的可选元数据（`runtime/session.rs:1652-1685`）。本函数取得结果集所有权并消费 `rows`，所以写出是一次性终端操作。

NULL 不是 `Option`，而是由两个字符串哨兵表示。由此存在一个重要不变量：上游必须保证真实字符串值不会与哨兵混淆；本文件仅按字符串相等判断。可选包围依赖 `ConcreteResultField` 的 `ColumnInfo` 类型；派生列或缺少 catalog 字段时元数据为 `None`，因此 `OPTIONALLY ENCLOSED` 不包围该值。

会话侧有两处可见状态变化：`last_explain_for_rows` 是 `EXPLAIN FOR CONNECTION` 的计划快照（消费点在 `runtime/dispatch.rs:2956-2977`）；`last_dml_report` 为协议 affected rows 提供依据。文件写出失败时函数在设置 `last_dml_report` 之前返回，因此不会报告成功行数；但独占创建后发生的中途错误可能留下部分文件。

## 依赖与调用关系

RustCodeGraph 将本文件识别为 13 个符号；精确查询定位到 `OutfileFormat`（第 25 行）、`optionally_enclose`（第 93 行）、`record_explain_for_plan`（第 116 行）和 `finish_select_into`（第 237 行）。其调用图确认 `finish_select_into -> record_explain_for_plan / escape_field / null_value / optionally_enclose`。图索引没有解析出 method callers，因此上游边由源码搜索核验为 `runtime/dispatch.rs:3209,3212,3217,3222,3226,3229,3232,3256 -> finish_select_into`。

直接下游依赖包括：

- 标准库 `OpenOptions`、`BufWriter`、`Write`：独占文件创建、缓冲写和持久化。
- `astersql_parser_ast`（父模块别名 `ast`）：读取 SELECT/INTO AST、表达式和值节点。
- `astersql_parser_mysql::type`：判断可选包围的 MySQL 列类型；对应依赖在 `pkg/session/Cargo.toml` 中声明为 `astersql-parser-mysql`。
- `ConcreteRecordSet`、`ConcreteResultField`、`ConcreteSession`、`SessionResult`、`SessionError` 与 NULL 哨兵：经 `use super::*` 从 `runtime.rs` 父模块作用域取得。
- `collect_physical_table_sources`、`current_database`、`metadata_catalog`：为观测计划提取表、数据库和二级索引信息。
- `crate::dml_runtime::DmlExecutionReport`：成功后的受影响行报告。

`pkg/executor/select_into.rs` 是另一条、以 child executor/chunk/sink 为中心的完整执行器实现；本文件没有调用它。两者共享 Go 语义来源，但不能把 executor 的流式生命周期、类型格式化或独立单测自动视为 canonical session 路径已覆盖。

## 错误处理与边界

- 仅支持 `OUTFILE`；其他 `SelectIntoType` 在创建文件前失败。
- `create_new(true)` 保证已有文件报错且不覆盖，行为与 Go 的 `os.O_EXCL` 对齐。错误文本来自操作系统并经 `SessionError::new(error.to_string())` 包装。
- 每一次字段/行写入、`flush`、`into_inner` 和 `sync_all` 都传播错误；没有重试、临时文件、原子 rename 或失败清理。因此调用者必须把“报错但路径已创建/含部分内容”视为可能状态。
- `from_option` 只取包围符和转义串的首个字节。parser 应在更早阶段约束合法长度；直接构造 AST 的调用者不能依赖本函数再次校验。多字节 Unicode 字符会被截成首字节，不应作为扩展时的隐式支持结论。
- 转义匹配只看字段/行终止串的第一个字节，与 Go `escapeField` 一致；完整终止串仍会被写出。
- `escape_field` 对 `ConcreteRecordSet` 中的所有非 NULL 字符串执行字节转义，而完整 Go executor 只对 string/JSON eval type 调用转义。canonical 路径的值已经字符串化，这是一项实现边界，扩展二进制或类型精确输出时必须重新评估。
- `record_explain_for_plan` 只理解第一个物理表和形如 `列 = 常量` 的谓词，其他情况用占位值 `?`；生成的行是简化观测模型，不是 planner 真实物理计划。状态已被其他不可变借用占用时，它会跳过更新。
- SEM 禁止行为由调用方先行检查；绕过 `execute_statement` 直接调用本方法不会自行执行该安全检查。

## 并发与资源生命周期

`ConcreteSession` 内部状态使用 `RefCell`，说明该 canonical 路径是会话内同步执行模型，而不是 `Send + Sync` 的共享写出器。`record_explain_for_plan` 特意使用 `try_borrow_mut` 避免观测写入和现存借用冲突；成功收尾更新 `last_dml_report` 则使用 `borrow_mut`，要求届时没有冲突借用。

文件生命周期完全位于一次 `finish_select_into` 调用中：独占创建 `File` → 包装 `BufWriter` → 顺序消费全部行 → `flush` → `into_inner` → `sync_all` → 作用域结束关闭文件。没有后台任务、锁、通道或跨调用 writer。内存方面，输入结果已全量物化；每个字段的 `escape_field` 再分配一个至多略大于原字符串的 `Vec<u8>`。因此空间上至少保留完整结果集，写出时还产生字段级临时缓冲；与 `pkg/executor/select_into.rs` 的 chunk 流式写出不同。

同一路径并发写同名文件时只有一个 `create_new` 能成功；本文件没有额外协调。`sync_all` 提升成功返回前的数据持久化保证，但会给请求尾延迟带来同步 I/O 成本。

## 与 Go 版本的对应关系

直接 Go 对照位于 `pkg/executor/select_into.go`：`SelectIntoExec.Open` 同样只接受 OUTFILE，以 `O_RDWR|O_CREATE|O_EXCL` 和 `0640` 创建文件；`escapeField` 对 NUL、escape、enclosure、字段/行终止首字节采用相同转义规则；`dumpToOutfile` 使用 `\\N`/`NULL`、字段与行分隔符以及 optional enclosure；`Close` 负责 flush 和 close。`pkg/executor/select_into_test.go` 覆盖已有文件、point get、类型格式、affected rows、分隔/包围/转义、NULL 和常量等 Go 语义。

本文件保持的核心语义包括：不覆盖已有文件、默认 `\t`/`\n`/`\`、禁用 escape 后 NULL 为 `NULL`、NUL 写为 `<escape>0`、可选包围面向字符串/时间/JSON 类、成功后 affected rows 等于写出行数。`pkg/session/load_data_runtime_test.rs:257-288` 是 canonical Rust 路径的直接回归测试：它验证默认写出为 `1\tone\n2\ttwo\n\\N\t\\N\n`，并能被 `LOAD DATA LOCAL INFILE` 重新读入。

差异必须保留为当前事实：Go executor 从 child executor 分 chunk 拉取并按原始 typed datum 格式化，canonical 文件接收已字符串化、全量物化的 `ConcreteRecordSet`；Go 的 `Close` 尝试 flush、关闭目标和关闭 child，而本文件在函数内部 `flush + sync_all` 后由 RAII 关闭文件；Go executor 的 `LinesStartingBy` 也未在其写出片段中使用，本文件同样忽略 AST 的 `LinesInfo.Starting`。完整 Rust executor 的浮点格式测试在 `pkg/executor/select_into_test.rs`，不直接覆盖本文件，因为 canonical 路径没有调用 `DumpRealOutfile`。

## 扩展指南

- 增加 `DUMPFILE` 或变量目标时，从 `finish_select_into` 的类型分支拆出独立 sink，不要让 OUTFILE 的独占创建和分隔规则泄漏到其他目标；同步增加 `pkg/session` 下独立 `*_test.rs`，不要把测试嵌入本源文件。
- 扩展 `FIELDS`/`LINES` 语义时，首先修改 `OutfileFormat::from_option` 及 `escape_field`，明确处理 `DefinedNullBy`、`NullValueOptEnclosed`、`Lines.Starting` 和多字节字符；同时与 parser 的长度校验、Go `LineFieldsInfo`/`SelectIntoExec` 对齐。
- 改进类型保真时，需要从 `ConcreteRecordSet` 的字符串单元格边界入手，或复用 typed executor，而不能只在 `optionally_enclose` 添加类型。尤其应覆盖 BIT/二进制、浮点极值、decimal、enum/set、JSON、时间和真实值碰撞 NULL 哨兵的情况。
- 改成流式或异步写出时，要同时调整结果生产接口、affected rows 累计、取消/错误清理和资源关闭顺序；不得仅把 `BufWriter` 移到后台，因为当前 `ConcreteSession` 状态基于 `RefCell` 且输入已全量物化。
- 改动 `record_explain_for_plan` 时，应保持其“观测失败不影响查询成功”的不变量，并在独立 session runtime 测试中覆盖无表、多表、非等值谓词、唯一/非唯一索引和借用冲突。
- 文件 I/O 行为变更应新增已有文件不覆盖、中途错误后的残留策略、Unix 权限和成功后协议 affected rows 测试。现有最接近的 Rust 测试是 `pkg/session/load_data_runtime_test.rs::select_into_outfile_uses_load_data_compatible_defaults`；Go 兼容矩阵位于 `pkg/executor/select_into_test.go`，完整 Rust executor 的单测位于 `pkg/executor/select_into_test.rs`。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/select_into.rs` 确认目标已索引且含 13 个符号。
- RustCodeGraph 精确查询：`query OutfileFormat`、`query finish_select_into`、`query record_explain_for_plan`、`query optionally_enclose`；`callees finish_select_into` 确认四条本文件内部调用边。method callers 未由图输出，故以 `rg` 和 `runtime/dispatch.rs:3209-3256` 补证八个直接调用点。
- 已阅读生产源码：`pkg/session/runtime/select_into.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/session.rs`、`pkg/session/dml_runtime.rs`、`pkg/parser/ast/lib.rs`、`pkg/session/Cargo.toml`。
- 已阅读对照实现：`pkg/executor/select_into.go` 与 `pkg/executor/select_into.rs`。前者是 Go 行为基线；后者证明仓库还存在独立的 typed/chunk executor 路径，不能与 canonical session 收尾器混同。
- 已阅读测试：直接 canonical 回归 `pkg/session/load_data_runtime_test.rs:257-288`；Go 兼容覆盖 `pkg/executor/select_into_test.go`；独立 Rust executor 的浮点测试 `pkg/executor/select_into_test.rs`。未发现 `select_into.rs` 同目录同名测试文件。
- 人工复核结论：该文件存在的原因是让 canonical session 的所有 SELECT 结果共享一个 OUTFILE 收尾点；其安全扩展边界集中在 AST 选项归一化、字节转义、结果类型/NULL 表示、文件生命周期以及两个 session 观测状态。
