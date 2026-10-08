# `pkg/session/runtime/row_codec.rs`

## 文件定位

`row_codec.rs` 是 `astersql-session` crate 内部的关系行存储适配层，由 `pkg/session/runtime.rs` 以私有模块 `mod row_codec` 装配。它位于 SQL 运行时的字符串行表示与底层 TiDB 兼容 `Datum`/KV 编码之间：DML、DDL 回填、关系扫描、系统会话和 mlog 调用这里，把 `HashMap<String, Option<String>>` 形式的运行时行转换为记录 KV、索引 KV 或反向可显示的值。

该文件不是通用 rowcodec 的实现；实际字节格式由 `astersql-tablecodec`（例如 `EncodeRow`、`EncodeRowKeyWithHandle`、`GenIndexKey`、`GenIndexValuePortal`）提供。本文件负责结合 session 的表元数据、SQL 类型标志、DDL 中间状态和行写入配置正确调用这些编码器。`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`，并直接依赖 `astersql-kv`、`astersql-meta-model`、`astersql-parser-{ast,mysql}`、`astersql-sessionctx-stmtctx`、`astersql-tablecodec`、`astersql-types`、`serde_json` 与 `uuid`。

## 核心职责

1. 在 AST 字面量、运行时可选字符串和 `astersql_types::datum::Datum` 之间转换，并把 statement flags 传入有 SQL mode 含义的类型转换（`typed_literal_to_runtime_value_with_warning`、`datum_to_runtime_value`、`runtime_value_to_datum`）。
2. 为 binary 字符串和 BIT 提供可逆的文本桥接，避免非 UTF-8 字节在 session 的字符串行模型中丢失（`BINARY_RUNTIME_PREFIX`、`binary_runtime_bytes`）。
3. 按普通整数句柄、无符号整数句柄或 common handle 生成记录句柄，再编码记录 key/value（`relational_row_handle`、`encode_relational_row_with_format`）。
4. 根据索引元数据生成普通、唯一、部分、多值及 DDL 变更期索引条目（`relational_index_value_rows`、`encode_relational_index_value_row`、`encode_relational_index_entries`）。
5. 将 old/new row 合并为确定顺序的索引 mutation，并在 online DDL backfill/merge 阶段写临时索引键和值（`relational_index_mutations`）。
6. 提供旧行 origin default 与插入时表达式 default 的轻量运行时求值（`origin_default_runtime_value`、`insert_default_runtime_value`）。

## 主要符号

- `relational_handle_key(table_id, logical_handle) -> kv::Key`：把逻辑 `i128` 句柄按低 64 位解释为 `IntHandle`，调用 `EncodeRowKeyWithHandle` 形成记录键。
- `typed_literal_to_runtime_value[_with_warning]`：识别 `Value`、括号及一元正负号；先构造相应 Datum，再按目标列 `FieldType` 转换。带 warning 版本返回 `(值, WarningCount > 0)`；非字面量退回 `dml_runtime::EvalExpr`。
- `is_typed_literal`：递归判断值、括号值和一元正负字面量，供 DML 选择精确类型转换路径。
- `BINARY_RUNTIME_PREFIX`、`is_binary_string_column`、`binary_runtime_bytes`、`display_runtime_value`：用 `__astersql_binary_hex__:` 加十六进制承载 binary charset/BINARY flag 列的原始字节；显示时仅在可解为 UTF-8 时还原文本。
- `datum_to_runtime_value`：覆盖 NULL、整数、浮点、DECIMAL、duration、enum、set、JSON、time、vector、BIT、string/bytes；未知 Datum kind 返回错误。
- `origin_default_runtime_value`、`insert_default_runtime_value`：读取列元数据默认值；后者按行求值 `CURRENT_DATE`、`CURRENT_TIMESTAMP`/`NOW`、`VEC_FROM_TEXT` 和 UUID，并兼容早期 Rust catalog 中带 `.000000` 的 timestamp 默认表达式。
- `runtime_value_to_datum`：按目标列类型解析运行时字符串。它显式处理 binary marker、BIT 十六进制和数值语义、整数范围、JSON、DECIMAL 精度/scale、空 enum、日期时间及通用 `ConvertTo`。
- `relational_row_handle`：common handle 对主键列执行 `EncodeKey`；否则从 PK/auto-increment 列或 `_tidb_rowid` 解析 `IntHandle`，NULL 或解析失败即报错。
- `encode_relational_row[_with_format]` 与 `ConcreteSession::encode_relational_row_for_write`：收集 Public/WriteOnly/WriteReorganization 列，处理 changing-column 转换和 `PreventNullInsertFlag`，调用 `EncodeRow`，再用物理分区 ID 与 handle 生成记录键。实例方法读取 `state.row_encoder_enabled` 选择行值格式。
- `relational_index_column`、`multi_valued_index_datum`、`relational_index_value_rows`：解析索引列；对 MVIndex 的 JSON array 做类型化、长度截断、编码去重和笛卡尔展开；普通索引列兼顾 changing type/dependency column 与 binary 原始字节。
- `encode_relational_index_value_row`：应用 changing type 后，调用 `GenIndexKey` 与 `GenIndexValuePortal`，传入 restored-data、distinct、handle 和物理表 ID 信息。
- `encode_relational_index_entries`：筛选当前应维护的本地索引；排除 clustered primary 与 global index，按 `ConditionExprString` 过滤部分索引，再展开并编码条目。
- `encode_relational_unique_index_entries`：只生成 Public、本地、非 clustered 的 unique index；跳过含 NULL 的非 distinct 键，返回 `(key, indexed datums, index name)` 供冲突检查。
- `relational_index_mutations`：旧行产生删除、新行产生写入；以 `BTreeMap<Vec<u8>, _>` 合并同键操作；根据索引状态/backfill 状态编码临时索引 mutation，并在 merging 阶段同时维护正式键。

全部生产符号可见性均为 `pub(super)`（或 `ConcreteSession` 的 `pub(super)` 方法），没有对 crate 外公开的 API、trait、结构体或条件编译生产分支。测试模块由 `runtime.rs` 的 `#[cfg(test)] mod row_codec_test` 独立装配。

## 执行流程

记录写入主流程如下：

1. `dml.rs`、`ddl.rs` 等准备表元数据与字符串行，并向 `encode_relational_row_for_write` 传入当前 statement flags。
2. `relational_row_handle` 决定 common handle、显式/自增整数主键或隐藏 `_tidb_rowid`；分区表的最终物理 ID 由 `ConcreteSession::row_physical_id` 决定。
3. `encode_relational_row_with_format` 只编码可写 schema state 的列。changing column 从 dependency column 读取旧值并转为新类型；禁止 NULL 的过渡列会产生 `[ddl:1138]` 错误。
4. 每个字符串值经 `runtime_value_to_datum` 恢复为强类型 Datum，随后 `astersql_tablecodec::EncodeRow` 生成值、`EncodeRowKeyWithHandle` 生成键。
5. 调用方再用 `relational_index_mutations` 为旧/新行生成删除或写入操作；普通索引直接维护正式键，online DDL 中间状态还可能写 temp index key/value。

索引值流程从 `relational_index_value_rows` 开始。普通列得到一个 Datum；多值索引列把 JSON 数组转换成元素 Datum，并以单元素 `EncodeKey` 的结果去重，再与此前列组合。空数组意味着该行没有索引项。`encode_relational_index_value_row` 随后生成 TiDB 兼容 index key/value；部分索引先由 `ParseGeneratedExpr` 与 `row_matches_simple_where` 判定是否参与。

读取侧不在本文件解码整行；`relational_scan.rs::decode_relational_row_value` 使用底层 row decoder 后调用 `datum_to_runtime_value`，缺失旧列时调用 `origin_default_runtime_value`。因此本文件同时承担写入转换和读取后规范化，而字节级解码属于相邻模块/`astersql-tablecodec`。

## 数据与状态

- session 行的核心形态是 `HashMap<String, Option<String>>`：key 使用列名小写形式，`None` 表示 SQL NULL；它不是最终持久化格式。
- `Datum` 是类型边界。`Flags` 随调用链传入 DECIMAL、temporal、changing-column 等 `ConvertTo`，承载截断、SQL mode 等转换策略。
- binary 字符串使用带固定前缀的 ASCII 十六进制中间表示。该 marker 会原样存入 rowcodec，真正原始字节只在索引编码或协议/显示边界恢复；这维持行 offset 的确定性。
- BIT 的中间表示是 `0x` 十六进制。数值输入优先解析为 `i64/u64` Datum，避免把 `-1` 的 UTF-8 字节误当 binary literal。
- 表/列/索引的 `SchemaState`、`BackfillState`、`ChangeStateInfo`、`ChangingFieldType`、`UseChangingType` 决定 online DDL 期间编码哪些列、使用哪种类型、维护正式索引还是临时索引。
- `row_encoder_enabled` 位于 `ConcreteSession.state` 的内部可变状态中，写路径通过 `borrow()` 读取并构造 `rowcodec::Encoder`；纯函数版本默认启用新 row encoder。
- mutation 使用 `BTreeMap`，既消除同 key 的旧/新操作冲突，也产生按 key 排序的稳定输出。多值索引用 `BTreeSet` 按编码字节去重。

## 依赖与调用关系

RustCodeGraph 对该文件识别出 31 个符号。主要上游调用边包括：

- `dml.rs` 在 INSERT/UPDATE/DELETE、唯一键检查、外键级联、索引回填路径调用 `typed_literal_to_runtime_value_with_warning`、`encode_relational_row[_for_write]`、`relational_index_value_rows` 和 `relational_index_mutations`。
- `ddl.rs` 在表交换、列变更和索引回填中调用记录/索引编码；`normal_ddl_test.rs` 直接复核索引编码。
- `relational_scan.rs` 调用 `datum_to_runtime_value`、`origin_default_runtime_value` 和 `runtime_value_to_datum`，并用 `encode_relational_row` 计算 overlay/候选行键。
- `system_session.rs` 在索引 backfill record 生成中调用 Datum 转换与索引值展开；`mlog.rs` 组合记录写入与索引 mutation。
- `session.rs` 的测试辅助入口调用记录编码和索引 mutation；`admin.rs` 调用单条索引编码；`ttl_runtime.rs`、`dxf_session.rs`、`import_query.rs` 使用 binary marker 还原函数。

主要下游关系是：AST 求值进入 `crate::dml_runtime::{EvalExpr, ParseGeneratedExpr}`；类型转换进入 `astersql-types` 与 `astersql-sessionctx-stmtctx`；表/列/索引状态来自 `astersql-meta-model` 和 `astersql-parser-mysql`；KV 编码进入 `astersql-tablecodec`；多值索引 JSON 解析进入 `serde_json`；UUID 默认值进入 `uuid`。

RustCodeGraph 的精确 `query` 为 `encode_relational_row`、`encode_relational_row_with_format`、`relational_index_mutations`、`runtime_value_to_datum` 返回了本文件符号 ID；其 `explore` 调用半径还显示 `encode_relational_row` 被 session/DDL/scan 路径调用，`relational_index_mutations` 被 session 与 mlog 调用，`datum_to_runtime_value` 被 scan/system-session 调用。图命令的单独 `callers/callees` 在本地未打印额外明细，因此调用位置又以 `rg` 对直接源码引用进行了核验。

## 错误处理与边界

所有可能失败的编码与转换使用 `SessionResult`，外部错误经 `session_error("上下文", error)` 包装；针对用户可理解边界则直接构造 `SessionError`。关键边界包括：

- binary `CHAR(n)` 字面量超长返回 `Data too long for column`，不足长度补零；非法 marker 不解码，显示函数保留原字符串。
- 不支持的 Datum kind 明确失败；NULL runtime 值转换为 NULL Datum。
- unsigned/signed 整数执行显式宽度范围检查；JSON、DECIMAL、temporal 和 BIT 解析错误携带操作上下文。DECIMAL 还通过带 flags 的目标 `FieldType` 转换以保留声明 precision/scale。
- common handle 编码或构建失败、整数 handle 缺失/NULL/不可解析、changing-column dependency offset 无效均中止写入。
- `PreventNullInsertFlag` 在 DDL 过渡列上阻止 NULL；changing type 的试转换即使结果未替换原 Datum，也会提前验证兼容性。
- 未知索引列、非法 MVIndex JSON/元素类型会失败；空 JSON 数组合法但生成零索引项；重复数组元素按编码值去重。
- clustered primary 不生成独立索引 KV，global index 在此本地编码路径被排除；unique 索引只对不含 NULL 的条目做 distinct 冲突探测。
- temp index mutation 依据 `DeleteOnly`、`ReadyToMerge`、`Merging` 等状态选择 delete/backfill/merge key version；修改此逻辑会影响 online DDL 可见性与恢复兼容性。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或长期资源。绝大多数函数只借用元数据和当前行，并返回拥有的 key/value/Datum。`ConcreteSession::encode_relational_row_for_write` 对 session state 做一次短生命周期不可变 `borrow()`；借用仅用于读取 `row_encoder_enabled`，在调用返回前释放。

时间与随机性是仅有的非纯输入：`insert_default_runtime_value` 每次调用用 `SystemTime::now()` 求 `CURRENT_DATE/TIMESTAMP/NOW`，用 `Uuid::new_v4()` 求 UUID，因此默认表达式是逐行求值而非缓存。索引和记录 mutation 只负责构造数据，不提交事务；事务顺序、原子性与重试由 `dml.rs`、`ddl.rs`、`mlog.rs` 等调用方管理。

内存开销随列数、索引数及多值索引展开量增长。多值索引对多个 array 列做组合扩展，理论大小为各元素数乘积；当前实现没有在本文件设置数量上限，调用者和 SQL/JSON 限制是外部边界。

## 与 Go 版本的对应关系

仓库没有同路径 `pkg/session/runtime/row_codec.go`；Rust 文件把 Go 中分散在 table、tablecodec、tables/index 与 DML 层的职责收束到 session runtime 适配层。因此应按语义而非文件名对齐：

- `origin_default_runtime_value` / `insert_default_runtime_value` 对应 `pkg/table/column.go` 的 `GetColOriginDefaultValue` / `GetColDefaultValue`。Go 版本通过 expression context 完整处理默认值；Rust 当前显式覆盖持久化默认值及常见表达式，并保留旧 Rust catalog 的 timestamp 拼写兼容。
- 记录键和值分别对齐 `pkg/tablecodec/tablecodec.go::EncodeRowKeyWithHandle` 与 `EncodeRow`；Rust 直接调用其已移植 crate API，避免自定义字节格式。
- 索引 key/value 对齐 `pkg/tablecodec/tablecodec.go::GenIndexKey` 与 `GenIndexValuePortal`；索引维护行为对应 `pkg/table/tables/index.go::index.Create` 及相关 DML mutation 路径。
- SQL 类型转换与 SQL mode 语义在 Go 中由 statement/expression context 驱动；Rust 通过 `NewStmtCtx().TypeCtx().WithFlags(flags)` 传递当前支持的转换标志。warning 仅由 `typed_literal_to_runtime_value_with_warning` 显式返回布尔量，不能假设等同于 Go 完整 warning 列表。
- online DDL changing column、temp index、部分索引、多值索引和 restored data 都通过共享元数据与 tablecodec API对齐；任何状态分支调整都应同时查验 Go 的 table/index mutation 与 DDL backfill 路径，而不能只比较本文件。

独立 Rust 测试 `pkg/session/runtime/row_codec_test.rs` 验证 timestamp 默认表达式（含带引号 FSP 和旧 `.000000` 后缀）、逐行 UUID v4，以及 DECIMAL 索引游标编码对 precision/scale 的保持。Go 默认值的邻近测试在 `pkg/table/column_test.go`；底层行/索引编码测试位于 `pkg/tablecodec/tablecodec_test.go` 和 table/index 相关测试。

## 扩展指南

- 新增 SQL 类型或改变字符串/Datum 映射时，应成对修改 `datum_to_runtime_value` 与 `runtime_value_to_datum`，检查 typed literal 路径，并在独立的 `row_codec_test.rs` 增加正向、反向、NULL、溢出、SQL mode/warning 和非 UTF-8 回归；不要把测试写进生产文件。
- 修改 binary marker 必须保持已落盘值兼容，并同步检查 `dxf_session.rs`、`import_query.rs`、`system_session.rs`、`ttl_runtime.rs` 等消费者；前缀碰撞和双重编码是主要兼容风险。
- 扩展默认表达式应优先对齐 Go `GetColDefaultValue` 的 statement-time 语义，明确时区、FSP、随机函数逐行求值和错误/warning；不要把元数据表达式文本误当最终值。
- 新增 handle 类型或分区规则应从 `relational_row_handle`、`ConcreteSession::row_physical_id` 和 `relational_handle_key` 接入，同时验证 record key、common handle、unsigned handle 与隐藏 rowid。
- 新增索引形态应检查 `relational_index_value_rows`（值展开）、`encode_relational_index_entries`（是否应维护）、`encode_relational_unique_index_entries`（冲突语义）和 `relational_index_mutations`（DDL 状态机）四层，并同步底层 tablecodec 与 Go 行为。
- 性能修改需关注每列 Datum clone、整表/索引元数据 clone、MVIndex 笛卡尔积、重复 `relational_index_value_rows` 计算及 BTree 容器排序成本；优化不能改变 key/value 字节或 mutation 顺序语义。
- 本文件所有 API 都是 runtime 私有边界。若要复用，优先在 session runtime 内新增窄接口，不应直接扩大为跨 crate 公共 API，除非同时定义清晰的字符串行不变量。

## 验证依据

- 源与装配：`pkg/session/runtime/row_codec.rs`（985 行、31 个图索引符号）、`pkg/session/runtime.rs`（`mod row_codec` 与独立 `#[cfg(test)] mod row_codec_test`）、`pkg/session/Cargo.toml`（crate/feature/依赖边界）。
- Rust 测试：`pkg/session/runtime/row_codec_test.rs`；调用证据：`pkg/session/runtime/{dml.rs,ddl.rs,relational_scan.rs,system_session.rs,mlog.rs,admin.rs,session.rs,ttl_runtime.rs,dxf_session.rs,import_query.rs}` 和 `normal_ddl_test.rs`。
- Go 对照：`pkg/table/column.go::{GetColOriginDefaultValue,GetColDefaultValue}`、`pkg/tablecodec/tablecodec.go::{EncodeRowKeyWithHandle,EncodeRow,GenIndexKey,GenIndexValuePortal}`、`pkg/table/tables/index.go::index.Create`；邻近测试为 `pkg/table/column_test.go` 与 `pkg/tablecodec/tablecodec_test.go`。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件、目标文件 31 个符号；`files --filter pkg/session/runtime/row_codec.rs` 命中目标；`explore` 与精确 `query` 核验主要符号及调用半径。精确 `callers/callees` 命令本次退出成功但未产生文本，因此以 `explore` 结果和直接引用搜索交叉验证。
- 人工复核范围：本文能说明该文件存在原因、记录/索引编码流程、类型与 DDL 状态边界、错误和安全扩展点；没有把底层 rowcodec 字节实现或事务提交职责误归于本文件，也未声称 Cargo/运行时测试已执行。
