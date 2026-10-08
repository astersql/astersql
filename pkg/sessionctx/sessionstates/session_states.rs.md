# `pkg/sessionctx/sessionstates/session_states.rs`

## 文件定位

该文件是 `astersql-sessionctx-sessionstates` crate 中的会话迁移状态数据层：它定义跨节点携带的 `SessionStates` 快照、预处理语句与最近查询/DDL 信息，并实现与 Go `encoding/json` 兼容的序列化。crate 入口 `pkg/sessionctx/sessionstates/lib.rs` 将本模块公开导出；`pkg/sessionctx/sessionstates/Cargo.toml` 声明其依赖 `Datum`、`FieldType`、`SQLWarn`、`IndexInfo` 所在的已移植 crate。

Go 版的完整业务位置是 `SHOW SESSION_STATES` 导出与 `SET SESSION_STATES` 恢复之间的交换格式（`pkg/sessionctx/sessionstates/session_states.go:66-90`）。当前 Rust 索引显示本文件主要被本 crate 测试实例化，`SessionStateError` 则被同 crate 的 `session_token.rs` 复用；Rust server 的 `pkg/server/driver_tidb.rs` 仍定义了另一个仅包含 prepared statements 的简化 `SessionStates`，因此不应把本文件误述为 Rust SQL 主链已统一接线的快照类型。

## 核心职责

1. 用 `SessionStates` 聚合用户变量、系统变量、prepared statements、当前库、最近事务/查询/DDL 信息、语句结果、告警、SQL binding、资源组与假设索引/TiFlash 副本。
2. 保持 Go 的 JSON 协议：字段名使用 Go tag，零值按 `omitempty` 省略，缺失或 `null` 字段恢复为 Go 零值，`[]byte` 以标准 base64 字符串表示。
3. 不绕过业务类型的自定义 JSON：`Datum` 和 `SQLWarn` 分别调用自身 `MarshalJSON`/`UnmarshalJSON`，避免普通 serde 改变 Go 线上形状。
4. 提供会话迁移通用错误 `SessionStateError` 和 TiDB errno 8146 的映射，供会话令牌等同 crate 逻辑复用。

## 主要符号

- `SessionStateType = i32`、`StatePrepareStmt = 0`、`StateBinding = 1`：会话状态 handler 分类，数值顺序对齐 Go `iota`。
- `ErrCannotMigrateSession`：惰性初始化的 TiDB 标准错误，错误码来自 `errno::errcode::ErrCannotMigrateSession`。
- `SessionStateError::{Json, CannotMigrate}`：分别包装 serde JSON 错误和 TiDB shared error；`code()` 对不可迁移返回标准 errno，JSON 错误无固定 SQL errno 时返回 0；`cannot_migrate()` 使用标准错误模板并携带原因。
- `PreparedStmtInfo`：保存文本/二进制协议预处理语句的名称、SQL、库名和参数类型字节。`ParamTypes` 通过 `go_bytes` 序列化。
- `QueryInfo`：最近查询的 transaction scope、start/for-update TS、RU/RU v2 消耗和错误文本。
- `LastDDLInfo`：最近 DDL 的 SQL 文本和序号。
- `SessionStates`：核心快照。其集合字段使用 `HashMap`/`Vec`，可选结构使用 `Option<Box<_>>`，具体 TiDB 值使用 `Box<Datum>`、`Box<FieldType>` 和 `Box<IndexInfo>`。
- `null_default()` 与 `take_json_or_default()`：分别服务 derive 类型和手写 `SessionStates` 解码，把缺失/`null` 统一为 `Default`。
- `datum_map_to_json()` / `datum_map_from_json()`、`warnings_to_json()` / `warnings_from_json()`：在 serde value 和 TiDB 自定义 JSON 之间转换。
- `tiflash_replicas_to_json()` / `tiflash_replicas_from_json()`：将 Rust 单元值 `()` 映射为 Go `struct{}` 的 JSON 空对象 `{}`。
- `impl Serialize/Deserialize for SessionStates`：显式控制 21 个 JSON 字段的名称、省略与容错语义，并在反序列化时忽略未取用的未知字段，与 Go `encoding/json` 默认行为一致。

## 执行流程

导出方向从上层收集好的 `SessionStates` 开始。`Serialize::serialize` 创建 JSON object：普通 map/string 由 `insert_nonempty!` 处理，数值由 `insert_nonzero!` 处理，布尔值仅在 `true` 时写入，可选 query/DDL 信息仅在 `Some` 时写入。`UserVars` 和 `Warnings` 通过 TiDB 类型的 JSON 方法逐项转换，假设 TiFlash 副本的表占位值写成 `{}`。最后将组装好的 `serde_json::Value::Object` 交给调用方 serializer。

恢复方向由 `Deserialize::deserialize` 先要求顶层是 JSON object，然后按 Go tag 从 map 中逐个 `remove` 字段。通用字段走 `take_json_or_default()`，用户变量、告警和 TiFlash 占位图走专用解码器。任意子类型或 JSON 形状错误被转为 serde custom error；全部字段成功后才构造快照，不会暴露半成品。

Go 应用主链的对照流程为：`SessionVars.EncodeSessionStates` 等 handler 把运行态填入快照，`SHOW SESSION_STATES` 返回 JSON；另一会话的 `executeSetSessionStates` 用 `json.Decoder` 解码后调用 `DecodeStates`。这一上层编排在 Go 文件中有直接证据，但不是本 Rust 文件的实现范围。

## 数据与状态

`SessionStates` 是值快照，通过 `Clone + Default` 创建独立副本或空状态。它不持有 session 对象、锁、文件或网络连接。map 中的 `Box` 表示所有权隔离，不是共享同步容器。

关键不变量是 JSON 兼容性：空 map/list/string、数值 0、布尔 `false` 与 `None` 在输出时不产生键；输入缺键或 `null` 产生同样的 Rust 默认值。这意味着对零值而言“未出现”与“显式 `null`”在该层不可区分。未知顶层键也不保存，重新序列化后会消失。

`HashMap` 不提供稳定键顺序，因此兼容性应比较 JSON 语义而非原始字符串顺序。`RUV2Consumption` 已与 Go 当前字段对齐，与 `RUConsumption` 一起无条件出现在非空 `QueryInfo` 对象中。

## 依赖与调用关系

- 下游数据依赖：`types::datum::Datum`、`parser_types::types::FieldType`、`contextutil::SQLWarn`、`model::group_4::IndexInfo`。其 JSON 形状是迁移协议的一部分，修改这些类型的 JSON 实现会间接改变本快照。
- 序列化依赖：`serde`/`serde_json` 处理容器与 derive 类型，`base64` 处理 Go `[]byte`。
- 错误依赖：`dbterror` 与 `errno` 建立 `ErrCannotMigrateSession`。`pkg/sessionctx/sessionstates/session_token.rs` 调用 `SessionStateError::cannot_migrate` 报告缺少证书、验签、用户名不匹配和过期等迁移失败。
- 模块边界：`lib.rs` 通过 `pub use session_states::*` 再导出公开符号；`pkg/session/sessionapi/lib.rs` 当前仅再导出该 crate 的 `SessionStateType`。
- Go 上游：`pkg/sessionctx/variable/session.go` 的 `EncodeSessionStates`/`DecodeSessionStates`、`pkg/bindinfo/session_handle.go` 的 binding handler、session 的 prepared-statement handler 组装/恢复快照；`pkg/executor/simple.go:3628` 处理 `SET`。
- Rust 接线限制：`pkg/sessionctx/context.rs` 仅定义关联类型化的 `SessionStatesHandler` trait，`pkg/server/driver_tidb.rs` 有自身简化快照，本文件的完整 `SessionStates` 尚无同等 Rust SQL 导出/恢复主链证据。

## 错误处理与边界

JSON 边界上，顶层非 object、`user-var-values`/字段类型错误、`warnings` 非 array、`hypo-tiflash-replicas` 或其数据库值非 object、base64 非法，以及下游 TiDB 类型拒绝 JSON，都会终止整个解码。错误通过 `serde::de::Error::custom` 传播，不会静默丢弃已知字段的错误。另一方面，未知顶层键会被忽略，这是 Go 默认 JSON 解码的向前兼容选择，不是严格 schema 验证。

`SessionStateError::Json` 可从 `serde_json::Error` 自动转换；`CannotMigrate` 保留 TiDB shared error 并暴露 8146。但该文件本身不判定活跃事务、临时表、未取完游标或权限等迁移条件；这些业务拒绝条件必须由上层 handler 在构造快照时检查。

## 并发与资源生命周期

本文件没有内部可变全局会话状态、异步任务、通道或锁。`ErrCannotMigrateSession` 使用 `LazyLock` 进行线程安全的一次性初始化；之后只读。序列化只借用 `&self`，反序列化构建全新的所有权对象，因此不跨调用共享临时 buffer。

真正的并发一致性责任在快照收集者：例如 Go `SessionVars.EncodeSessionStates` 在复制用户变量时持有读锁。如果 Rust 日后把本类型接入活跃 session，上层必须在一致性边界内克隆所有字段，不能把对快照类型本身的可序列化性当成原子捕获保证。

## 与 Go 版本的对应关系

Rust 的 `SessionStateType`、两个常量、`PreparedStmtInfo`、`QueryInfo`、`LastDDLInfo` 和 `SessionStates` 逐项对应 `pkg/sessionctx/sessionstates/session_states.go:26-90`。公开字段名保留 Go 风格，JSON tag 也对齐 `user-var-values`、`prepared-stmt-id`、`query-info`、`rs-group` 等原名称。Go `[]byte` 默认 base64 行为由 `go_bytes` 显式复刻；Go `map[string]map[string]struct{}` 由嵌套 `HashMap<String, HashMap<String, ()>>` 与空 object 转换复刻。

与 Go derive 无需手写代码不同，Rust 必须手写 `SessionStates` 的 serde 实现，以复用 `Datum`/`SQLWarn` 的 Go 兼容 JSON 并精确处理 omitempty。`PreparedStmtInfo`、`QueryInfo` 和 `LastDDLInfo` 则用 serde derive 配合 `null_default`。当前 Rust 数据模型已含 Go 的 `RUV2Consumption` 字段，而 Rust SQL 主链的组装/恢复仍未完全对齐；两者需要分开评估。

Go 回归 `pkg/sessionctx/sessionstates/session_states_test.go` 还覆盖用户/系统变量、权限、prepared statement、binding、事务、临时表、游标、资源组、假设索引等端到端场景。Rust `session_states_test.rs` 前部大量内容是保存的 Go 参考文本，可运行的 Rust 断言位于文件末部，不应把参考文本误当成已接线的 Rust 集成测试。

## 扩展指南

- 新增快照字段时，同步修改 `SessionStates`、`Serialize::serialize`、`Deserialize::deserialize`、Go `SessionStates` 字段/tag，以及收集和恢复该状态的 handler。必须先决定零值是否 omitempty，以及旧节点遇到新键时的兼容策略。
- 新增自定义 TiDB 类型时，先验证其 serde 形状是否与 Go JSON 一致；若不一致，参照 `datum_map_*` 通过该类型的 Go 兼容 `MarshalJSON`/`UnmarshalJSON` 建立桥接。
- 新增迁移拒绝条件时，不应把 session 运行态检查塞入这个纯数据文件；应在对应 handler 检查，并用 `SessionStateError::cannot_migrate`/`ErrCannotMigrateSession` 统一错误码。
- 将完整快照接入 Rust SQL 主链时，需先消除 `pkg/server/driver_tidb.rs` 中同名简化类型的边界分裂，让 server、session variables、prepared statements 和 bindings 共用本 crate 类型；这是跨文件接线工作，不属于本文档任务。
- 测试保持在独立文件 `pkg/sessionctx/sessionstates/session_states_test.rs` 和 `session_states_1_aster_unit_test.rs`，不内嵌到生产源文件。至少覆盖：字段 tag、base64、缺失/`null`、零值省略、自定义 Datum/SQLWarn、TiFlash `{}`、非法形状与新旧 JSON 往返。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/sessionctx/sessionstates` 确认本 crate 的 Rust/Go 源文件与独立测试均已索引。
- RustCodeGraph `node --file pkg/sessionctx/sessionstates/session_states.rs` 核对了 502 行源文件的完整定义；`node session_states.rs::SessionStates` 确认类型位于 198 行，可见实例化调用者为 `session_states_json_round_trip_matches_go_tags_and_omitempty`。
- 源码与 crate：`pkg/sessionctx/sessionstates/session_states.rs`、`lib.rs`、`Cargo.toml`；直接错误消费者 `session_token.rs`。
- Go 对照与主链：`pkg/sessionctx/sessionstates/session_states.go`、`pkg/sessionctx/variable/session.go` 的 `EncodeSessionStates`/`DecodeSessionStates`、`pkg/executor/simple.go` 的 `executeSetSessionStates`、`pkg/sessionctx/sessionstates/session_states_test.go` 的 `showSessionStatesAndSet`。
- Rust 边界证据：`pkg/sessionctx/context.rs` 的 `SessionStatesHandler` trait、`pkg/server/driver_tidb.rs` 的同名简化快照与 `EncodeSessionStates`/`DecodeSessionStates`、`pkg/bindinfo/session_handle.rs` 的 binding 快照逻辑。
- 独立 Rust 测试：`session_states_test.rs` 的 `prepared_statement_state_uses_go_json_shape_and_base64_types`、`empty_optional_prepared_fields_match_go_omitempty`、`null_fields_decode_to_go_zero_values`、`session_states_json_round_trip_matches_go_tags_and_omitempty`；`session_states_1_aster_unit_test.rs` 还核对常量和 prepared statement JSON。
- 本任务仅新增说明文档，按计划不运行 Cargo；结构验证使用任务文件指定的 11 章节检查。
