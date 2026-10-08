# `pkg/server/handler/util.rs`

## 文件定位

本文件是 `astersql-server-handler` crate 的公共 HTTP handler 工具模块，由 [`pkg/server/handler/lib.rs`](lib.rs) 以 `pub mod util` 暴露。crate 的 [`Cargo.toml`](Cargo.toml) 将其对应到 Go 包 `pkg/server/handler`；文件自身不直接使用该清单中的领域、存储等依赖，而是为同 crate 的升级 handler、下游 TiKV handler crate 以及 `pkg/server` 的 HTTP 适配层提供协议常量、内存响应表示、JSON 写出和表/分区名切分。

它位于 status HTTP 请求链的辅助层而非路由入口：例如 [`upgrade_handler.rs`](upgrade_handler.rs) 调用 `WriteError`/`WriteData` 生成内存响应，随后 [`pkg/server/http_status.rs`](../http_status.rs) 的 `upgrade_response` 把 `ResponseWriter` 的状态码和 body 转为 server `Response`；[`tikvhandler/tikv_handler.rs`](tikvhandler/tikv_handler.rs) 则直接调用 `ExtractTableAndPartitionName` 规范化路由中的表名。

## 核心职责

- 定义与 Go `handler` 包一致的路径变量和查询参数键，如 `DB_NAME`、`TABLE_NAME`、`TABLE_ID_QUERY`、`OPERATION`，并提供 `DBName`、`TableName`、`TableIDQuery`、`Operation` 等 Go 风格兼容别名。
- 通过 `WriteError`、`WriteErrorWithCode` 和 `WriteData` 把状态码、JSON content type 与 body 写入 `ResponseWriter`。
- 用 `ExtractTableAndPartitionName` 实现 Go 同名函数的简单 `table(partition)` 切分语义。
- 为当前升级接口提供最小 JSON 扩展点 `JsonValue` 与字符串转义函数 `json_string`。

这里的响应与 JSON 能力是当前 Rust status server 的轻量内部实现，不是通用 HTTP/JSON 框架：`ResponseWriter` 只保存状态、content type 和字节数组，`JsonValue` 只要求返回一个 `String`，而 `terror_log` 当前不执行日志记录。

## 主要符号

- 路径/查询常量：`DB_NAME` 至 `SECONDS` 保存路由与查询字段的实际字符串值；`HEADER_CONTENT_TYPE`、`CONTENT_TYPE_JSON`、`STATUS_BAD_REQUEST`、`STATUS_OK` 保存响应协议值。Go 风格常量是前述 snake-case 常量的直接别名，并以 `#[allow(non_upper_case_globals)]` 保留旧调用拼写。
- `WriteError(w: &mut ResponseWriter, err: Error)`：固定以 400 调用 `WriteErrorWithCode`。
- `WriteErrorWithCode(w, status_code, err)`：先记录状态码，再追加 `err.message` 的原始字节，最后把写入错误和原错误交给 `terror_log`。
- `WriteData<T: JsonValue>(w, data)`：调用 `json_marshal_indent`；成功时设置 `Content-Type: application/json`、状态 200 并追加 JSON body，序列化失败时转入 `WriteError`。
- `ExtractTableAndPartitionName(input)`：返回拥有所有权的 `(table, partition)`；无左括号或无右括号时返回原字符串和空分区，否则使用第一个 `(` 与第一个 `)` 之间的内容。
- `JsonValue`：公开 trait，唯一方法 `to_json(&self) -> String`。本文件为 `String` 和 `&str` 实现；[`upgrade_handler.rs`](upgrade_handler.rs) 还为 `ClusterUpgradeInfo`、`SimpleServerInfo` 实现。
- `ResponseWriter`：公开但字段仅 crate 可见的内存响应对象；公开读取接口为 `status_code`、`body_bytes`，写入方法只在本模块内部可见。
- `Error`：只携带 `message: String` 的最小错误类型；`Error::new` 为 crate 内构造入口。
- `json_marshal_indent`、`json_string`、`terror_log`：分别承担当前无失败的 trait 调用、JSON 字符串转义和日志占位。

## 执行流程

错误响应流程为：handler 构造 `Error` → `WriteError` 选择 400（或调用方直接使用 `WriteErrorWithCode`）→ `ResponseWriter::write_header` 保存状态 → `write` 追加错误文本 → `terror_log` 接收潜在写错误及原错误。当前 `write` 恒为 `Ok(())`，所以潜在写错误分支在现实现中不会发生。

成功响应流程为：handler 提供实现 `JsonValue` 的数据 → `WriteData` 调用 `json_marshal_indent`/`to_json` → 设置 JSON content type → 保存 200 → 追加 UTF-8 JSON 字节。升级链中，`ClusterUpgradeHandler::ServeHTTP` 使用该流程后，`http_status.rs::upgrade_response` 读取状态和 body 并生成外层 HTTP `Response`。

表路由流程为：TiKV handler 从路径参数取出字符串 → `ExtractTableAndPartitionName` 找第一个左括号和第一个右括号 → `serve_table_http` 用表名替换路径中的 `TABLE_NAME` 并继续解析分区，或 `TikvHandlerTool::get_table` 丢弃分区部分后查询表。函数不会验证括号次序或尾随字符；测试明确保留 `orders(p0)ignored` 截取为 `orders`/`p0` 的行为。

## 数据与状态

`ResponseWriter` 有三项请求内状态：`status: Option<u16>`、`content_type: Option<String>`、`body: Vec<u8>`。`Default` 初始时无状态码、无 content type、body 为空；重复调用 `write_header` 会覆盖状态，重复调用 `write` 会追加 body。公开读取方法借用内部数据，不复制 body。

`Error` 只保存展示给客户端的消息，不含来源链、错误码或类型信息。常量和兼容别名均为静态 `&str`，无运行时分配；`ExtractTableAndPartitionName` 的返回值会为两个结果分配 `String`（无匹配时也复制整个输入）。`json_string` 预分配 `input.len() + 2`，对引号、反斜杠、换行、回车、制表符及其他控制字符转义，普通 Unicode 字符直接保留。

## 依赖与调用关系

模块内部调用边经 RustCodeGraph 核对：`WriteError → WriteErrorWithCode`；`WriteErrorWithCode → ResponseWriter::{write_header, write}` 与 `terror_log`；`WriteData → json_marshal_indent`，失败分支到 `WriteError`，成功分支到 `header_set`、`write_header`、`write` 与 `terror_log`。`JsonValue for String/&str → json_string`。

上游直接证据如下：

- [`upgrade_handler.rs`](upgrade_handler.rs) 导入 `Error`、`JsonValue`、`ResponseWriter`、`WriteData`、`WriteError`、`json_string`，用于升级状态接口的错误、字符串与结构化 JSON 响应。
- [`tikvhandler/tikv_handler.rs`](tikvhandler/tikv_handler.rs) 在 `serve_table_http` 和 `TikvHandlerTool::get_table` 中以完整 crate 路径调用 `ExtractTableAndPartitionName`。
- [`pkg/server/http_status.rs`](../http_status.rs) 的 `upgrade_response` 创建该 `ResponseWriter`，调用升级 handler，并读取 `status_code`/`body_bytes` 完成运行时适配。

RustCodeGraph 对这些跨 crate 调用未返回 `callers` 边，因此跨 crate 上游由上述源码引用补证；图中 `callees` 边用于核对文件内部流程。`Cargo.toml` 无 feature 条件，本模块也无 `cfg` 项；根 workspace、`pkg/server/Cargo.toml` 及多个 handler 子 crate 都通过路径依赖接入 `astersql-server-handler`。

## 错误处理与边界

- `WriteErrorWithCode` 将错误消息原样作为响应 body，不设置 content type，也不包装或转义；调用方必须避免把敏感内部信息放进 `Error::message`。
- `ResponseWriter::write` 当前只向 `Vec<u8>` 追加并恒成功；因此 `terror_log(write_err)` 是为未来可失败 writer 保留的结构，`terror_log` 自身当前也是空实现，不能据此声称错误已真正记录。
- `json_marshal_indent` 当前无实际缩进逻辑且恒返回 `Ok`；`JsonValue::to_json` 返回未经校验的字符串。新增实现若拼接无效 JSON，`WriteData` 无法识别。其序列化错误分支目前不可达，但保持了 Go `json.MarshalIndent` 失败后写 400 的控制流形状。
- `json_string` 覆盖常见 JSON 转义和 U+0000–U+001F 控制字符；它不是完整通用序列化器，复合类型仍需其 `JsonValue` 实现负责字段、逗号和数字格式。
- `ExtractTableAndPartitionName` 与 Go 一样采用简单首字符搜索：缺任一括号即不切分；不会检查 `)` 是否位于 `(` 之后。类似 `")("` 的异常输入可能因 Rust 字符串切片区间无效而 panic，调用边界应保证路由名符合预期语法。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、会话或事务。`ResponseWriter` 通过 `&mut` 在一次同步 handler 调用中独占修改，类型本身没有内部同步；外层每次请求新建 writer（`upgrade_response` 即如此），请求结束后其 `String`/`Vec<u8>` 随对象释放。

所有响应 helper 都是同步函数，不持有借用跨越调用，也没有异步取消点。静态常量可安全共享；`JsonValue::to_json` 只借用值。若未来让同一 writer 跨线程共享，应由调用层增加所有权或同步设计，不能依赖当前 API 自动协调并发。

## 与 Go 版本的对应关系

直接对照 [`util.go`](util.go)：两边的路径/查询字符串和公开 Go 拼写一致；Rust 额外提供 idiomatic snake-case 常量。`WriteError` 都默认 400，`WriteErrorWithCode` 都先写状态再写错误文本，`WriteData` 都先序列化、再设置 JSON header、写 200 和 body。`ExtractTableAndPartitionName` 的四个已测分支与 Go 的 `strings.IndexByte` 实现一致。

实现能力仍有差异：Go 接受任意 `error`、`http.ResponseWriter` 和 `any`，使用 `encoding/json.MarshalIndent` 并通过 `terror.Log(errors.Trace(...))` 记录写出错误；Rust 使用本地 `Error`/`ResponseWriter`、显式 `JsonValue`，`json_marshal_indent` 不缩进也不失败，`terror_log` 不记录。Rust 的 status server 适配层只读取此 writer 的状态和 body，content type 最终由外层 `Response::json` 建立。文档因此只确认当前已接线行为，不把这些占位抽象等同于 Go 标准库的全部语义。

Go 目录没有该工具的同名独立 `*_test.go`；Rust 的直接回归集中在 [`util_test.rs`](util_test.rs)，另有 [`handler_aster_unit_test.rs`](handler_aster_unit_test.rs) 覆盖 `WriteData("ok")` 的 200 与 JSON body。

## 扩展指南

- 新增路由参数时，应先在 snake-case 常量区加入唯一真值；若 Go 调用名仍需兼容，再添加直接别名，并在 `util_test.rs::go_exported_constant_spellings_are_preserved` 同步断言。还应检查 Go `util.go` 与实际路由消费者，避免字符串漂移。
- 扩展响应数据时，优先为目标类型实现 `JsonValue`，所有字符串字段复用 `json_string`；同步在独立测试文件中验证合法 JSON、可选字段与转义边界。若需要真正的可失败/通用序列化，应整体替换 `JsonValue`/`json_marshal_indent` 契约并审查 `upgrade_handler.rs` 的实现，而不是继续手写复杂 JSON。
- 若把内存 `ResponseWriter` 接到更多生产 handler，需决定 header、多次写状态、写失败和日志语义；修改后同步 `util_test.rs`，并检查 `http_status.rs::upgrade_response` 的适配是否仍保留 content type 与状态。
- 修改表/分区语法时应同时更新 `ExtractTableAndPartitionName`、TiKV handler 两处调用及 `table_and_partition_extraction_matches_go_branches`，并核对 Go 行为。修复异常括号 panic 会改变当前简单切分契约，应增加回归用例并明确兼容性影响。
- 测试必须继续放在独立的 `util_test.rs` 或相邻测试文件，不应内嵌回生产源文件。性能关注点主要是字符串/响应 body 分配和手写 JSON 的重复拼接；兼容风险主要是路由键值、HTTP 状态/body 顺序及 Go 风格别名。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/server/handler/util.rs`；`files --filter` 报告该文件 84 个符号；`node --file ... --offset 1 --limit 300` 阅读了完整 287 行源码。
- RustCodeGraph 符号/调用查询：查询了 `WriteError`、`WriteErrorWithCode`、`WriteData`、`ExtractTableAndPartitionName`、`json_string`、`ResponseWriter`；`callees --file pkg/server/handler/util.rs` 核对了上述内部调用边。跨 crate `callers` 无输出，已以精确源码引用补证。
- 源码与装配：[`util.rs`](util.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`upgrade_handler.rs`](upgrade_handler.rs)、[`tikvhandler/tikv_handler.rs`](tikvhandler/tikv_handler.rs)、[`pkg/server/http_status.rs`](../http_status.rs)。
- Go 对照与测试：[`util.go`](util.go)、[`util_test.rs`](util_test.rs)、[`handler_aster_unit_test.rs`](handler_aster_unit_test.rs)；全仓 `*_test.go` 搜索未找到该工具函数的直接 Go 单元测试。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核文件定位、运行流程、扩展入口与当前占位限制。
