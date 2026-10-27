# `pkg/session/runtime/pg_jsonb.rs`

实现显式text/json→jsonb严格重校验和规范化。PgJsonb真实解析器保留精确numeric、重复键最后值及PG键序；Option外层保留SQL NULL。jsonb→json/text沿规范文本输出，无法恢复原始空白或重复键。

validate_cast在行求值前限制转换资格，只接受未定型字符串、text、json、jsonb及NULL输入；反向只开放json/text和身份转换。MySQL BinaryJSON、bytea及其他不能保真桥接均返回0A000，包括typed NULL和空集。

cast_value将PgJsonbError装入既有PgJsonCastError::Jsonb，沿session/server原错误通道保留22P02、22P05、22003、22021，不回显输入。query和relational_value共用此实现，FieldType独立TypePgJsonb贯通定型及OID3802参数绑定。独立pg_jsonb_test验证SQL转换、NULL/空集、参数身份、重绑定、失败恢复和共享消费者；server相邻TCP测试验证简单查询描述、规范字节和SQLSTATE。

本模块不提供JSON运算、完整隐式转换矩阵、MySQL opaque/time桥接或binary参数/结果codec；后续任务负责wire四方向及PostgreSQL18.6对照，不能以本批测试替代。
