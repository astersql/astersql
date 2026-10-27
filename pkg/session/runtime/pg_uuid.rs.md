# `pkg/session/runtime/pg_uuid.rs`

负责 PostgreSQL 显式 text↔uuid 的共享转换和资格检查。通过独立 `TypePgUuid` 保持身份，不把文本外观、CHAR(36)或BINARY(16)当作UUID。值转换调用 `astersql_types::pg_uuid::PgUuid` 的真实词法和规范输出，既有行容器保存规范文本，类型由 FieldType/NativeType 独立携带。

`validate_cast` 在SELECT行求值前检查来源和目标，typed NULL及零行不能隐藏bytea/数值等不支持转换；只开放text→uuid、uuid→text及uuid身份转换。`cast_value` 对非NULL严格解析并规范化，SQL NULL继续由外层Option承载；不修剪输入或修改version/variant位。

`PgUuidCastError` 将非法词法映射到22P02，将未开放资格映射到0A000，错误不回显用户输入。session/lib.rs保留错误身份，server/runtime.rs、conn.rs和pg_conn.rs传递SQLSTATE。query.rs及relational_value.rs共同调用转换；planning.rs仅从明确cast约束推导OID0，control.rs沿现有参数绑定保持声明身份。pg_result.rs从类型码描述2950/16/-1并规范输出简单查询文本，不依赖首行。

独立测试 `pg_uuid_test.rs` 使用CreateAnalyzeSession验证真实SQL、显式参数身份、NULL、零行、非法输入、资格拒绝及恢复；parser和server相邻测试验证AST/Restore及真实TCP描述。运行 `cargo test -p astersql-session --lib pg05_uuid_sql -- --nocapture --test-threads=1`，须先按任务规则动态领取共享Cargo槽位。

本文件不实现UUID生成器、隐式转换矩阵、声明列存储子系统、二进制参数/结果codec或PG18.6真实差分；后二者由后续任务分别验收。
