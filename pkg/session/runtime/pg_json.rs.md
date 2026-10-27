# `pkg/session/runtime/pg_json.rs`

实现显式 text↔json 的共享转换和资格检查。值由批次6交付的 `astersql_types::pg_json::PgJson` 校验，保留重复键、空白、数值词法和Unicode转义原文；SQL NULL由外层Option表示，与JSON null及字符串"null"分离。

`TypePgJson` 是独立于MySQL TypeJSON(245)、text、bytea的内部身份。parser按方言选择类型，FieldType/NativeType通过查询定型、typed NULL、参数声明和结果传递。query.rs与relational_value.rs调用同一cast_value；validate_cast在行求值前拒绝无证据转换，所以typed NULL和空集不能绕过资格检查。仅开放text、未定型字符串及NULL→json，json→text和身份转换。

`PgJsonCastError`保持输入22P02/22021及未开放转换0A000的结构化错误，session/lib.rs、server/runtime.rs、conn.rs、pg_conn.rs传递SQLSTATE。错误不回显用户输入。planning.rs仅从明确cast推导OID0，control.rs保留已声明text/bytea/json参数身份并沿既有绑定器执行。pg_result.rs只增加114/-1的结果描述，复用原有简单查询文本出口。

独立测试pg_json_test.rs通过CreateAnalyzeSession覆盖原文、标量、嵌套cast、NULL/零行、声明参数、拒绝和恢复；parser独立测试覆盖双方言及Restore/serde，server/pg_types_test.rs使用ConcreteSessionDriver及真实TCP验证114/-1/-1、行字节、SQLSTATE及恢复。各crate使用pg05_json_sql测试前缀，执行Cargo前须领取仓库共享槽位。

jsonb规范转换由相邻pg_jsonb模块实现；本模块不提供JSON运算、完整隐式转换矩阵、声明列存储，或完整协议入参/二进制结果codec；后两者由任务8/9负责，真实PG18.6四方向差分由任务10负责。

当前共享SQL lexer消费反斜杠，直接SQL测试对SQL字面量单独转义；包含Unicode转义的原始JSON另外经真实typed text参数绑定验证，不修改JSON原文断言。共享parser未注册bytea拼写；转换资格通过既有BINARY字段和声明OID17参数验证。标准字符串词法和bytea拼写属于其他族的既有缺口，不在本模块扩建。


## 2026-10-27 JSONB显式转换接线

JSONB相关资格委托pg_jsonb::validate_cast，允许json↔jsonb；PgJsonCastError::Jsonb保持规范解析器SQLSTATE。
