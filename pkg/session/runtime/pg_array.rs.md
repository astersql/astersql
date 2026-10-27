# `pkg/session/runtime/pg_array.rs`

本文件连接共享 SQL ARRAY 分析、求值和规范数组文本载体。`analyze` 对直接 SELECT 构造及显式 int4[]/int8[]/text[]/bool[] 构造上下文记录元素身份和参数约束；无上下文的裸空构造继续42P18。`field_type` 使用 PostgreSQLArrayElement，独立于 MySQL JSON/多值索引数组标记。

`value` 保留既有构造族逻辑；已开放族构造经 `typed_array` 验证后输出规范 PgArray 文本。`value_as` 为明确整数数组上下文提供空数组/全NULL构造求值，维度和下界由规范容器验证。`validate_cast` 仅开放整数构造、int4[]/int8[]、文本和NULL等本任务输入；未开放转换以0A000拒绝。`cast_value` 供常量及行求值消费者共用，不自行建立新执行内核。

`int4_text` 委派 PgArray::from_text_with 和既有 pg_cast_int4，保留容器/元素 SQLSTATE，输出 shape/lower/NULL 齐全的文本；字段元数据携带独立1007身份。它不把 SQL NULL 存为 PgArray，NULL保持外层 Option。测试位于独立的 pg_array_test.rs。

## 2026-10-27 数组显式转换（任务23）

显式int4[]↔int8[]通过规范PgArray逐元素转换，collect完全成功后才构造结果；保留维度、下界、元素NULL及外层SQL NULL。反向i64→i32 checked转换报22003。text→目标数组委派真实标量codec；构造器沿目标类型处理空数组/全NULL及元素溢出。不开放隐式cast矩阵或其他族。


## text 数组逐族接入（任务25）

analyze/value_as/validate_cast注册Text上下文（OID25/数组1009），text叶值调用既有pg_text_cast，再由PgArray验证shape/lower/NULL与NUL；显式text[]输入和空/全NULL构造保留独立身份。跨text数组与整数数组cast矩阵仍范围外0A000。独立pg05_array_text测试核对prepared与直接共享执行。


## bool 数组逐族接入（任务26）

Bool数组沿既有analyze/value_as/typed_array/validate_cast/cast_value注册，元素调用pg_boolean_cast(TextToBool)，保留两层NULL与原子失败。跨bool数组与整数/text数组cast矩阵未开放，显式0A000；不以MySQL真值替代PG标量契约。

## 2026-10-27 int2数组局部接线

Int2定型、共同整数类型选择、构造及文本转换复用pg_cast_int2；跨数组族转换仍0A000。

行为与验证见 `docs/postgresql-pg05-array-int2-evidence.md`；不开放完整跨族cast、存储或数组运算。


## 2026-10-27 int8数组边界构造

一元带符号整数元素复用query::pg_integer_literal_type规则：i64::MIN的正数字面token为Uint64，须对完整负号表达式定型为Int8，避免只分析正token误拒合法边界。实际值仍沿既有i128精确一元运算和pg_cast_int8，不经f64。其他族、numeric数组或隐式转换矩阵未扩展。证据见 `docs/postgresql-pg05-array-int8-evidence.md`。


## 任务29 bytea数组局部接线

Bytea数组沿共享analyze/cast/value消费者，typed_array调用types::pg_array::bytea_text_input，保留任意字节、形状/下界和两层NULL。构造器先做标量codec再单独数组转义；不复制server bytea语法。跨族数组转换保持0A000。

OID17参数构造ARRAY时仅识别既有binder生成的binary literal cast及typed NULL，复用binary_cast_literal_value/binary_runtime_bytes还原原字节；不按普通text的前缀外观识别字节，不开放任意binary表达式或跨族转换。


## numeric数组开放（任务30）

numeric元素分析与显式数组转换接入共享PgNumeric；逐元素apply_modifier后整体发布，保留下界、NULL和precision/scale，其他跨族转换继续0A000。

当前精确18.6差分与验证记录见 `docs/postgresql-pg05-array-numeric-evidence.md`；只承诺记录范围。


## float4数组与原始binary32位型（任务31）

Float4身份/分析/定型接入既有ARRAY消费者。构造器逐叶调用已有pg_cast_float4或读取已经定型的私有标量字节，先收集全部元素再发布规范PgArray；保留shape/lower/元素NULL/外层NULL。只有私有Bytes AST或已带Float4[]身份的结果能读取内部字节载体，公开文本cast仍严格解析正常数组文本。跨族cast保持既有范围门禁。

验证与兼容性范围见 `docs/postgresql-pg05-array-float4-evidence.md`；测试位于对应独立测试文件，生产文件不嵌入单元测试。


## float8数组与binary64位型（任务32）

Float8分析、定型、构造与同族cast，和Float4共用矩形shape收集；元素按各族标量codec解析，原子发布；私有常量应用器扩展64位，跨族数组cast仍0A000。

行为证据与本机验证限制见 `docs/postgresql-pg05-array-float8-evidence.md`；测试保持在同目录独立测试文件。

## 2026-10-28 varchar数组局部接线

数组分析、构造及cast注册Varchar；逐元素调用已有pg_text_cast/pg_varchar_length，显式字符截断和typmod校验在数组shape/lower之外处理。非法元素整值失败，跨族合法未支持转换仍0A000。独立测试见同目录既有测试文件及server的pg05_array_varchar前缀；真实18.6证据见`docs/postgresql-pg05-array-varchar-evidence.md`。

## 2026-10-28 bpchar数组局部接线

本文件在既有数组消费者中注册Bpchar元素1042/数组1014身份，保留字符长度typmod、shape/lower及两层NULL。标量转换复用pg_text_cast，按字符截断并补U+0020，不复制标量实现。独立测试见对应测试文件的pg05_array_bpchar前缀；18.6差分及限制见`docs/postgresql-pg05-array-bpchar-evidence.md`。

Bpchar构造器共同长度由constructor_field_type逐叶检查：同长度标量及typed NULL保留长度，未定型NULL、无长度或不同长度使typmod为-1；普通查询和prepared描述使用同一规则。


## date数组逐族接入（任务35）

Date分析、构造和同族显式转换调用PgDate；cast_value_with_context与canonical_date_text接收不可变标量日期上下文。构造器避免将BC/扩展年份经MySQL日期归一化，跨族cast仍0A000。独立测试使用pg05_array_date前缀，真实18.6差分见docs/postgresql-pg05-array-date-evidence.md。
