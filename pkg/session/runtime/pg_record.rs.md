# `pkg/session/runtime/pg_record.rs`

匿名 PostgreSQL record 的局部类型与文本载体桥接。ExplicitRow/ImplicitRow 的字段顺序与嵌套布局由 `PostgreSQLRecordField` 保存；未知字面量和 NULL 保留 OID705，结果通过独立布局产生 OID2249 record。

`scalar_type` 接受已开放参数身份和带元素身份的数组显式转换，复用既有标量定型。`record_type_in_context` 对列及标量子查询使用真实来源元数据，优先 PostgreSQL 身份，避免把 UUID、json、jsonb 或数组降格为旧传输类型。`value_with` 复用共享表达式求值，按 record 文本规则转义字段，区分空字符串与 SQL NULL。

此文件为自行建立的 PostgreSQL 局部桥接，没有对应 Go record 子系统可复刻。不实现命名复合类型、record 入参、record 数组或行比较。边界、复现输入与当前验证见 `docs/postgresql-pg05-record-boundaries.md`；独立测试位于同目录 `pg_record_test.rs`。
