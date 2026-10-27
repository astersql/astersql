# `pkg/types/pg_uuid.rs`

## 文件定位

实现 PostgreSQL UUID 的独立规范值和文本词法。`lib.rs` 导出模块，独立行为测试位于 `pg_uuid_test.rs`。

## 值与接口

`PgUuid` 私有保存 `[u8; 16]`，接受全部128位组合，不校验 version 或 variant。`from_bytes` 和 `as_bytes` 使用文本从左到右的字节顺序，不交换 GUID 字节序。SQL NULL 由外层容器承载，不能以全零 UUID 代替。

`FromStr` 接受32个ASCII十六进制字符、大小写、一对可选外层花括号，以及每四位之后的可选单连字符（末尾除外）。不修剪空白，不接受URN、嵌套或单侧花括号、错位/重复/尾连字符、多余字符和非法十六进制。解析按字节检查，非ASCII输入不会触发UTF-8切片panic。`PgUuidError::sqlstate()` 为 `22P02`，诊断不携带原始用户输入。

`Display` 始终输出36个ASCII字符的小写 `8-4-4-4-12` 格式；例如 `00112233445566778899AABBCCDDEEFF` 输出 `00112233-4455-6677-8899-aabbccddeeff`。解析使用固定16字节缓冲，输出直接写入formatter。

## 边界与验证

本模块不负责SQL定型、typed NULL元数据、协议参数/结果或二进制消息长度诊断；文本解码层应先处理非法UTF-8。这里的`from_bytes`只接收精确16字节数组，不是协议载荷解码入口。后续消费者须持续保存UUID身份。

测试命令：`cargo test -p astersql-types --lib pg_uuid_test -- --nocapture --test-threads=1`（遵守任务约定先领取Cargo共享槽位）。覆盖全部128种分隔组合、花括号和大小写、非法输入及恢复、全零/全ff、全部单字节值和首尾位、规范字节顺序及外层NULL区分。该单元测试不代表PostgreSQL18.6真实客户端差分通过。
