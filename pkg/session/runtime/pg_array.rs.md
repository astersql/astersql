# `pkg/session/runtime/pg_array.rs`

本文件连接共享 SQL ARRAY 分析、求值和规范数组文本载体。`analyze` 对直接 SELECT 构造及显式 int4[] 构造上下文记录元素身份和参数约束；无上下文的裸空构造继续42P18。`field_type` 使用 PostgreSQLArrayElement，独立于 MySQL JSON/多值索引数组标记。

`value` 保留既有构造族逻辑；int4 构造经 `int4_text` 验证后输出规范 PgArray 文本。`value_as` 为明确 int4[] 上下文提供空数组/全NULL构造求值，维度和下界由规范容器验证。`validate_cast` 仅开放 int4 构造、同族数组、文本和NULL等本任务输入；未开放转换以0A000拒绝。`cast_value` 供常量及行求值消费者共用，不自行建立新执行内核。

`int4_text` 委派 PgArray::from_text_with 和既有 pg_cast_int4，保留容器/元素 SQLSTATE，输出 shape/lower/NULL 齐全的文本；字段元数据携带独立1007身份。它不把 SQL NULL 存为 PgArray，NULL保持外层 Option。测试位于独立的 pg_array_test.rs。
