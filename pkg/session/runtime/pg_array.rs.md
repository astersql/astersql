# `pkg/session/runtime/pg_array.rs`

本文件连接共享 SQL ARRAY 分析、求值和规范数组文本载体。`analyze` 对直接 SELECT 构造及显式 int4[]/int8[] 构造上下文记录元素身份和参数约束；无上下文的裸空构造继续42P18。`field_type` 使用 PostgreSQLArrayElement，独立于 MySQL JSON/多值索引数组标记。

`value` 保留既有构造族逻辑；整数构造经 `integer_array` 验证后输出规范 PgArray 文本。`value_as` 为明确整数数组上下文提供空数组/全NULL构造求值，维度和下界由规范容器验证。`validate_cast` 仅开放整数构造、int4[]/int8[]、文本和NULL等本任务输入；未开放转换以0A000拒绝。`cast_value` 供常量及行求值消费者共用，不自行建立新执行内核。

`int4_text` 委派 PgArray::from_text_with 和既有 pg_cast_int4，保留容器/元素 SQLSTATE，输出 shape/lower/NULL 齐全的文本；字段元数据携带独立1007身份。它不把 SQL NULL 存为 PgArray，NULL保持外层 Option。测试位于独立的 pg_array_test.rs。

## 2026-10-27 数组显式转换（任务23）

显式int4[]↔int8[]通过规范PgArray逐元素转换，collect完全成功后才构造结果；保留维度、下界、元素NULL及外层SQL NULL。反向i64→i32 checked转换报22003。text→目标数组委派真实标量codec；构造器沿目标类型处理空数组/全NULL及元素溢出。不开放隐式cast矩阵或其他族。
