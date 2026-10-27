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
