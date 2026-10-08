# `pkg/server/internal/parse/parse.rs`

## 文件定位

本文件是 `astersql-server-internal-parse` crate 的协议载荷解析实现，crate 边界由同目录 `Cargo.toml` 定义，`lib.rs` 通过 `pub mod parse; pub use parse::*;` 导出其接口。上层 `pkg/server/lib.rs` 又在 `internal::parse` 下再导出整个 crate。它处理两类 MySQL 协议输入：`COM_STMT_FETCH` 的固定 8 字节命令体，以及 Protocol 4.1 的 SSLRequest/HandshakeResponse41。

仓库搜索只发现 Rust 生产层的再导出，没有发现这些函数的非测试 Rust 调用点；当前可验证的 Rust 调用者位于 `handshake_test.rs`、`parse_test.rs` 和 `migration_aster_unit_test.rs`。完整应用中的生产入口仍可由 Go 对照链路确认：`pkg/server/conn.go` 的 `readOptionalSSLRequestAndHandshakeResponse` 调用 `HandshakeResponseHeader`/`HandshakeResponseBody`，`pkg/server/conn_stmt.go` 的 `handleStmtFetch` 调用 `StmtFetchCmd`。因此，本文件是已经实现和测试的 Rust 移植单元，但仅凭当前仓库证据不能声称 Rust 服务运行时已经调用它。

## 核心职责

- `stmt_fetch_cmd` 验证 `COM_STMT_FETCH` 载荷必须恰为 8 字节，以小端序读出 statement ID 与请求行数，并用 `MAX_FETCH_SIZE` 将行数封顶为 1024。
- `handshake_response_header` 解析 SSLRequest 与 Response41 共用的固定 32 字节前缀，仅写入 `Response41.capability` 与 `Response41.collation`，其余 max-packet-size 和保留字节跳过。
- `handshake_response_body` 根据 capability 位依次解析用户名、三种认证数据形态、可选默认库、认证插件、连接属性和 zstd 等级；所有不可信偏移访问都转换为 `ParseError`，不允许客户端包触发 Rust panic。
- `parse_attrs` 将长度编码的连接属性行解码为原始字节键值，应用大小策略，生成弃用/截断警告，并维护 `ConnectAttrsLost` 与 `ConnectAttrsLongestSeen`。
- 文件保持协议文本字段为 `Vec<u8>`/`HashMap<Vec<u8>, Vec<u8>>`，与 Go `string` 可保存任意字节的语义对齐，避免非法 UTF-8 被替换。

## 主要符号

- 常量 `MAX_FETCH_SIZE: u32 = 1024`：单次游标 fetch 行数上限。
- 常量 `MAX_CONNECTION_ATTRIBUTES_SIZE: u64 = 1 << 20`：握手连接属性声明长度的 1 MiB 硬拒绝线。
- 常量 `MAX_NORMAL_CONNECTION_ATTRIBUTES_SIZE: i64 = 65_536`：负配置值的有效上限，也是 `LongestSeen` 指标的观测边界。
- 常量 `RESERVED_CONN_ATTR_TRUNCATED` 与 `DEPRECATED_UNDERSCORE_WARNING`：服务端截断标记键和稳定警告文案。
- 静态量 `CONNECT_ATTRS_METRICS_LOCK: Mutex<()>`：串行化非原子复合的 Load/Store 指标更新。
- `ParseError::{MalformedPacket, ConnectionAttributesTooLarge}`：分别表示字段/边界畸形和属性声明超过硬上限；实现 `Display` 与 `Error`。
- 公开函数 `stmt_fetch_cmd(&[u8]) -> Result<(u32, u32), ParseError>`、`handshake_response_header(&C, &mut Response41, &[u8]) -> Result<usize, ParseError>`、`handshake_response_body(&C, &mut Response41, &[u8], usize) -> Result<(), ParseError>`：crate 对外协议入口。两个握手函数保留泛型上下文参数以贴近 Go API，但当前实现不读取该参数。
- 包内函数 `parse_attrs`：测试可见的属性策略入口；内部再分为 `decode_conn_attrs` 和 `apply_conn_attrs_policy_and_metrics`，把语法解码与策略/副作用分离。
- 辅助函数 `read_nul_terminated`、`take_bytes`、`read_lenenc_int`、`read_lenenc_bytes`：集中维护边界检查和 offset/consumed 推进。
- 内部结构 `ConnAttrKv`、`DecodedConnAttrs`：保存有序属性、每项有效载荷字节数、总大小及非标准下划线键标记。

## 执行流程

`COM_STMT_FETCH` 路径很短：`stmt_fetch_cmd` 先拒绝非 8 字节输入，再分别读取 `data[0..4]` 和 `data[4..8]` 的小端 `u32`；第二个值经 `min(1024)` 后返回。长度检查保证后续 `try_into().unwrap()` 不会失败。

握手解析分两段进行。调用者先把完整包交给 `handshake_response_header`：不足 32 字节时记录警告并返回 `MalformedPacket`，否则读取 capability、偏移 8 的 collation，并返回正文起点 32。`handshake_response_body` 从该偏移开始：

1. 用户名必须以 NUL 结束。
2. 若有 `ClientPluginAuthLenencClientData`，认证数据按 length-encoded integer 读取；首字节为 1 时兼容 MySQL 5.7 的历史“两字节无认证数据”表示。否则，有 `ClientSecureConnection` 时使用单字节长度；两者都没有时使用 NUL 结尾形式。
3. `ClientConnectWithDB` 置位且仍有尾部数据时，读取 NUL 结尾的默认库名。
4. `ClientPluginAuth` 置位时，仅当剩余区能找到 NUL 才写入非空插件名并推进；找不到终止符时保持插件字段为空，这与迁移测试锁定的兼容行为一致。
5. `ClientConnectAtts` 置位时，空尾部被视为可忽略的畸形客户端行为并成功返回。非空时先读属性区长度：超过 1 MiB 立即拒绝；长度超出实际包边界返回 `MalformedPacket`；属性行内部解码失败则只记录警告并让握手成功。解码成功后才替换 `packet.attrs`。
6. `ClientZstdCompressionAlgorithm` 置位时，从当前 offset 读取一个字节作为 `zstd_level`；缺字节返回 `MalformedPacket`。

连接属性路径先由 `decode_conn_attrs` 循环读取 length-encoded key/value，对每项记录 `key.len() + value.len()`（不计编码前缀），同时检查以下划线开头且不在标准白名单的键。随后 `apply_conn_attrs_policy_and_metrics` 按输入顺序累计大小：首次使累计值超过有效限制时递增一次 `ConnectAttrsLost`，该项及之后各项都不再写入结果；最终注入 `_truncated=<丢弃字节数>` 并拼接警告。无超限时，重复键遵循 `HashMap::insert` 的后值覆盖前值语义。

## 数据与状态

主要可变输出是调用者提供的 `Response41`（定义于 `pkg/server/internal/handshake/handshake.rs`）：本文件写入 `capability`、`collation`、`user`、`auth`、`db_name`、`auth_plugin`、`attrs` 和 `zstd_level`。字段使用拥有所有权的字节容器，返回后不借用网络输入缓冲区。

属性策略读取 `astersql-sessionctx-vardef` 暴露的全局值：`ConnectAttrsSize == 0` 时完全禁用收集和指标更新；负值经 `normalize_connect_attrs_limit` 映射为 65,536；非负值原样作为有效限制。发生截断时 `ConnectAttrsLost` 每次调用最多递增一次。`ConnectAttrsLongestSeen` 记录小于 65,536 字节的最大已解码总大小，达到或超过该阈值的载荷不参与此指标。

`DecodedConnAttrs.items` 保留线上的属性顺序，确保“第一次越界后不再接受后续属性”的前缀截断规则可复现。`total_size`、`byte_size` 和 offset 使用 checked conversion/addition防止整数溢出被误判为合法包；策略函数内部的累计加法依赖前一解码阶段已将每项和总和约束在 `i64` 可表示范围内。

## 依赖与调用关系

直接依赖由 `pkg/server/internal/parse/Cargo.toml` 声明：

- `astersql-parser-mysql` 提供六个 `Client*` capability 位。
- `astersql-server-internal-handshake` 提供 `Response41`。
- `astersql-server-internal-util` 提供 `ParseLengthEncodedInt` 和 `ParseLengthEncodedBytes`。
- `astersql-sessionctx-vardef` 提供连接属性限制及指标。
- `log` 用于畸形握手、属性解码失败和警告文本的日志；Cargo 中还声明并由 `lib.rs` 再导出 `astersql-util-logutil`，但 `parse.rs` 本身没有直接调用它。

RustCodeGraph 将 `parse.rs` 标为被测试文件、`pkg/server/pg_result.rs` 等文件使用；对四个核心函数的精确搜索与全仓库 `rg` 进一步表明，具名函数调用均来自本 crate 的三个独立测试文件，生产 Rust 侧只有 `pkg/server/lib.rs` 的 crate 再导出。内部调用链为 `handshake_response_body -> read_nul_terminated/take_bytes/read_lenenc_int/parse_attrs`，以及 `parse_attrs -> decode_conn_attrs -> read_lenenc_bytes` 和 `parse_attrs -> apply_conn_attrs_policy_and_metrics -> normalize_connect_attrs_limit/increment_connect_attrs_lost/update_connect_attrs_longest_seen`。

Go 生产调用链为 `clientConn.readOptionalSSLRequestAndHandshakeResponse -> parse.HandshakeResponseHeader -> parse.HandshakeResponseBody`，解析结果继续驱动 TLS、安全传输、认证和连接属性逻辑；游标链为 `clientConn.handleStmtFetch -> parse.StmtFetchCmd`，结果用于查找 prepared statement 并执行 cursor fetch。这些 Go 调用说明移植单元在总体服务器协议中的目标位置，但不是 Rust 已接线的证据。

## 错误处理与边界

`MalformedPacket` 覆盖固定长度不符、NUL 缺失、长度编码工具报错、声明区间越过切片、offset 算术溢出、长度无法转为 `usize` 以及 zstd 等必需尾字节缺失。固定头错误会记录包含包字节的 warning；属性行语法错误在 `handshake_response_body` 中被降级为 warning 并返回成功，因为属性是可选元数据。与之不同，属性区外层 length-encoded integer 错误、声明长度超出实际包体和超过 1 MiB 都是握手失败。

`ConnectionAttributesTooLarge` 有独立错误变体和稳定错误消息。硬上限检查发生在 `u64 -> usize` 转换和切片之前，既限制资源消耗，也避免平台字宽差异。解析函数不会回滚已经写入 `Response41` 的早期字段；调用者应在错误时丢弃该握手对象，而不是使用部分结果。

需要注意两个兼容边界：认证插件尾部没有 NUL 时当前实现不报错而是保持空插件名；capability 声明连接属性但正文在该位置已结束时也直接成功。这两点都有测试/Go 注释依据，扩展时不应随意“严格化”。

## 并发与资源生命周期

文件不创建线程、异步任务、通道、事务或外部 I/O；解析工作同步完成，临时切片只活到函数返回。写入 `Response41` 和局部 `HashMap` 的数据均复制为拥有所有权的 `Vec<u8>`，因此不会悬挂引用。

唯一共享可变状态是 `ConnectAttrsSize`、`ConnectAttrsLost` 和 `ConnectAttrsLongestSeen`。本文件只读取 `ConnectAttrsSize`；更新后两者时获取 `CONNECT_ATTRS_METRICS_LOCK`，毒锁通过 `into_inner()` 恢复。该锁使 `Load + Store` 或比较后写入成为本文件调用间的临界区，但它不能约束仓库中绕过此锁直接修改同一全局值的代码。`wrapping_add` 明确规定 `ConnectAttrsLost` 在 `i64` 溢出时回绕，而不是 panic。

测试通过 `migration_aster_unit_test.rs::GLOBALS_LOCK` 串行化会修改全局指标的用例，并在结束时恢复原值。新增涉及这些指标的独立测试也应采用同一锁和恢复模式，避免并行测试互相污染。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/internal/parse/parse.go`，Rust 基本保持以下语义：8 字节 fetch 布局与 1024 封顶；32 字节握手头；capability 驱动的认证/数据库/插件/属性/zstd 顺序；MySQL 5.7 特殊认证表示；1 MiB 属性硬上限；非标准下划线属性警告；按前缀截断并覆盖客户端 `_truncated`；负限制映射 64 KiB；超大载荷不更新 `LongestSeen`。

Rust 的安全实现替代了 Go 的 `defer recover`：每个索引、范围和 offset 推进显式检查并返回 `ParseError::MalformedPacket`。Go 使用 `string` 和 `map[string]string`，Rust 使用字节向量以保留同等的任意字节能力。Go 对 `ConnectAttrsLost` 使用原子 `Add`、对 `LongestSeen` 使用 CAS 循环；Rust vardef 接口在此处以 Load/Store 暴露，所以使用进程内 mutex 包住复合操作。

存在一项表面结构差异但策略一致：Go 的 `connAttrKV` 在应用策略时重新计算 key/value 大小，Rust 在解码时把 `byte_size` 缓存在 `ConnAttrKv`。此外 Rust `ParseError` 不等同于 Go 的具体 `mysql.ErrMalformPacket` 类型；上层 Rust 接线时需要负责将其映射到协议错误响应，目前仓库中尚未找到该生产映射。

## 扩展指南

- 新增握手字段或 capability 分支时，应修改 `handshake_response_body`，严格保持协议字段顺序，并用 `take_bytes`/`read_nul_terminated` 等受检辅助函数推进 offset；同时在 `migration_aster_unit_test.rs` 增加正常、截断、缺终止符和非 UTF-8 用例。
- 修改固定头布局时同步检查 `handshake_response_header`、`body_packet` 测试构造器和真实抓包测试 `handshake_test.rs`；不要把 max-packet-size 等跳过字段误写入不对应的结构字段。
- 修改连接属性格式时优先在 `decode_conn_attrs` 处理线格式，在 `apply_conn_attrs_policy_and_metrics` 处理限制、指标和警告，避免把副作用重新混进解码循环。相关测试放在独立的 `parse_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件。
- 改动白名单、`_truncated` 或警告文案时同步核对 Go `standardConnAttrs`、`reservedConnAttrTruncated` 和两端测试；这些文本与覆盖规则可能被日志分析或兼容测试依赖。
- 真正接入 Rust 服务主链时，需要在连接握手和 prepared-statement cursor 分派处调用公开入口，并定义 `ParseError` 到服务端 MySQL 错误的映射；接线前不能仅因 `pkg/server/lib.rs` 已再导出就认为功能已启用。
- 性能风险主要在客户端声明的属性区复制和 HashMap 分配；1 MiB 硬上限不可在没有资源评估与 Go 对齐的情况下放宽。并发风险集中于全局指标更新，若 vardef 改为提供原子 `fetch_add`/CAS，应整体替换锁方案并加入并发回归测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标目录已索引；`files --filter pkg/server/internal/parse` 列出 `parse.rs` 及独立测试；`node --file pkg/server/internal/parse/parse.rs --offset 1 --limit 500` 返回完整 402 行、19 个符号；`query` 分别唯一定位 `stmt_fetch_cmd`、`handshake_response_header`、`handshake_response_body`、`parse_attrs`。callers/callees 命令未产生可用文本，因此调用结论又用全仓库具名搜索核验。
- 源码与 crate：`pkg/server/internal/parse/parse.rs`、`pkg/server/internal/parse/Cargo.toml`、`pkg/server/internal/parse/lib.rs`、`pkg/server/internal/handshake/handshake.rs`、`pkg/server/Cargo.toml`、`pkg/server/lib.rs`。
- Rust 独立测试：`pkg/server/internal/parse/parse_test.rs`（fetch、下划线警告、截断/Lost），`pkg/server/internal/parse/handshake_test.rs`（MySQL 8.0 抓包），`pkg/server/internal/parse/migration_aster_unit_test.rs`（全部 capability、畸形边界、非 UTF-8、硬上限、禁用收集及 LongestSeen）。
- Go 对照与生产入口：`pkg/server/internal/parse/parse.go`、`pkg/server/internal/parse/parse_test.go`、`pkg/server/internal/parse/handshake_test.go`、`pkg/server/conn.go`、`pkg/server/conn_stmt.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核唯一新增生产物、真实路径、符号名称、Go/Rust 接线边界和扩展建议。
