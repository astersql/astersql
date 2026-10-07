# `br/pkg/metautil/debug.rs`

## 文件定位

`debug.rs` 属于 Cargo 包 `astersql-br-pkg-metautil`，由同目录 `lib.rs` 以 `pub mod debug` 挂载并通过 `pub use debug::*` 再导出。它不是备份元数据的生产写入路径，而是调试解码辅助：从 `Storage` 读取备份中由索引引用的加密 `MetaFile`/`StatsFile`，验证明文摘要后，将适合人工检查的 JSON 写回同一存储的 `jsons/<对象名>.json`。对应的 Go 实现是 `br/pkg/metautil/debug.go`。

当前 Rust 应用接线存在重要限制：仓库搜索只发现独立测试直接调用本文件的入口；`br/cmd/br/debug.rs` 中的 `debug decode` 使用的是该命令 crate 自己 `br/cmd/br/stubs.rs` 里的同名空操作 `DecodeMetaFile`/`DecodeStatsFile`，而 `br/cmd/br/Cargo.toml` 也没有依赖 `astersql-br-pkg-metautil`。因此，本文件提供了可测试的真实解码实现，但尚不能据此声称 Rust BR 命令已经调用它。Go 的 `br/cmd/br/debug.go` 则直接调用 Go 版实现来展开 file、raw-range、schema 索引及根级 schemas。

## 核心职责

- `DecodeMetaFile` 处理根 `MetaFile.meta_files` 指向的一层子元数据：读取对象、解密、校验 SHA-256、解析 protobuf、拒绝超出一层的嵌套、生成 JSON，并继续解码子元数据中 schemas 引用的统计文件。
- `DecodeStatsFile` 遍历 `Schema.stats_index`：按索引名读取对象，使用索引 IV 解密，对明文做 SHA-256 校验，解析成 `StatsFile` 并生成 JSON。
- `marshal_meta_file_json`、`marshal_stats_file_json` 及其字段辅助函数将本地 `brpb` 桩类型转换为与 Go 调试格式兼容的 JSON 表示；二进制 key/hash 使用十六进制，`cipher_iv`、统计索引的 hash/IV/inline data 使用 Base64，内嵌 db/table/stats/DDL/统计表内容按 JSON 值展开。
- `trace_err` 给对象存储 I/O 和非法层级等本地构造错误附加 `Trace`；摘要不匹配统一产生 `ErrInvalidMetaFile`。

## 主要符号

- `pub const JSONFileFormat: &str = "jsons/%s.json"`：保留 Go 风格的公开格式常量。Rust 实际写路径没有用 `%` 替换，而是在两个入口内使用 `format!("jsons/{}.json", name)`；扩展时不能直接把它当作 Rust `format!` 模板。
- `pub fn DecodeStatsFile(ctx: &Context, s: Arc<dyn Storage>, cipher: Option<&CipherInfo>, schemas: &[Schema]) -> Result<(), SharedError>`：统计旁路文件入口。空文件名索引被跳过；其他项按传入顺序串行处理，任一失败立即返回。
- `pub fn DecodeMetaFile(ctx: &Context, s: Arc<dyn Storage>, cipher: Option<&CipherInfo>, metaIndex: Option<&MetaFile>) -> Result<(), SharedError>`：子元数据入口。`None` 是成功的空操作；每处理一个节点前检查取消状态；成功写入子元数据 JSON 后调用 `DecodeStatsFile`。
- `marshal_meta_file_json(&MetaFile)`：输出非空的 `data_files`、`raw_ranges`、`schemas`、`ddls`。DDL 字节必须各自是合法 JSON。
- `marshal_stats_file_json(&StatsFile)`：总是输出 `blocks` 数组；每块的 `json_table` 必须是合法 JSON，非零 `physical_id` 才输出。
- `file_json`、`schema_json`：分别构造备份文件描述和 schema 的 JSON。`schema_json` 解析 `db`，并按非空条件解析 `table`、`stats`；统计索引保留名称、摘要、长度、IV 与 inline data。
- `insert_string`、`insert_u64`、`insert_hex`、`insert_base64`：实现 protobuf JSON 常见的“省略默认值”规则；`base64_encode` 是文件内的三字节分组编码器。
- `trace_err`：把 `SharedError` 包进 `Trace(Some(err)).expect("trace")`；其调用假定 `Some` 一定能产生错误值。

本文件没有自定义 struct、enum、trait、`impl` 或条件编译项；公开面只有常量和两个函数，其余函数均为模块私有。

## 执行流程

`DecodeMetaFile` 的主流程如下：

1. 若 `metaIndex` 为 `None`，直接返回 `Ok(())`。
2. 顺序遍历 `metaIndex.get_meta_files()`；每个节点开始前通过 `Context::is_cancelled` 检查取消，取消时返回 `Interrupted("context canceled")`。
3. 用节点 `name` 调用 `Storage::ReadFile`，再以可选 `CipherInfo` 和节点 `cipher_iv` 调用 `Decrypt`。
4. 对解密明文调用 `sha256_bytes`，与节点 `sha256` 比较；不一致时返回带期望值和实际值十六进制文本的 `ErrInvalidMetaFile`。
5. 使用本地 protobuf `parse_from_bytes::<MetaFile>` 解析明文。若子对象仍包含 `meta_files`，以 `InvalidData` 拒绝，因为根元数据最大只允许一层子索引。
6. `marshal_meta_file_json` 将叶子内容转换成 JSON，并写到 `jsons/<节点名>.json`。
7. `take_schemas` 将 schemas 移出子对象，然后复用同一存储和 cipher 调用 `DecodeStatsFile`；全部节点完成后返回成功。

`DecodeStatsFile` 对每个非空名称的统计索引执行相同的“读取 → 解密 → 明文摘要校验 → protobuf 解析 → JSON 转换 → 写回”链路。输出名是 `jsons/<statsIndex.name>.json`。循环没有回滚语义：前面已写出的 JSON 会在后续节点失败时保留。

## 数据与状态

函数本身无全局可变状态。输入状态由 `MetaFile`/`Schema` 索引、只读 `Context`、可选 cipher 以及共享 `Arc<dyn Storage>` 组成；输出状态是存储中新增或覆盖的 JSON 对象。

校验和始终针对解密后的 protobuf 明文，而不是存储中的密文。`CipherInfo` 是整个调用共享的，IV 则来自每个 `File` 或 `StatsFileIndex`。JSON 转换会省略空字符串、空字节、零整数和 `false`，但数组容器行为有所区别：meta 的四类集合为空时整体省略，stats 输出始终包含 `blocks`。

二进制字段编码不是统一策略：SST 文件的 `sha256`、start/end key 走 `hex_encode`；`cipher_iv` 走 Base64；`Schema.stats_index.sha256` 也走 Base64。这一选择由 `file_json` 与 `schema_json` 明确决定，修改时需保持现有反序列化方和 Go JSON 兼容性。

## 依赖与调用关系

上游方面，`br/pkg/metautil/lib.rs` 公开本模块；`br/pkg/metautil/debug_test.rs` 直接调用 `DecodeMetaFile`，并通过后者间接覆盖 `DecodeStatsFile`。仓库级 Rust 搜索没有找到来自其他生产 crate 的真实调用。名称相同的 `br/cmd/br/stubs.rs` 函数属于另一个 crate，不是本文件的调用者。

下游方面：

- `astersql-objstore-storeapi::{Context, Storage}` 提供取消状态与对象读写抽象。
- `astersql-br-pkg-utils::encryption::Decrypt` 执行解密；`crate::metafile::{sha256_bytes, hex_encode}` 提供摘要和十六进制编码。
- `crate::stubs::kvproto::brpb` 与 `crate::stubs::protobuf` 提供 `MetaFile`、`Schema`、`StatsFile` 等本地 protobuf 兼容类型和解析入口。
- `serde_json` 解析内嵌 JSON 并组装最终对象；`astersql-errors` 和 `astersql-br-pkg-errors` 提供共享错误、trace 与 `ErrInvalidMetaFile`。

`br/pkg/metautil/Cargo.toml` 将该目录定义为独立 library crate，`lib.rs` 是 crate 根。清单注释说明它为 darwin arm64 保持精简，使用本地 stubs 避免引入完整 kvproto/grpcio/statistics 依赖；这也是分析具体类型语义时必须以本地 `stubs.rs` 为准的原因。

## 错误处理与边界

- `metaIndex == None` 和空索引集合成功返回；统计索引名称为空也被安静跳过。
- 读取、写入错误先被转换成 `std::io::Error::other`，原错误的具体类型会丢失，但字符串被保留并附加 trace。
- `Decrypt`、protobuf 解析和 `serde_json` 解析/序列化错误通过 `?` 立即传播。内嵌 db/table/stats/DDL/json_table 不是合法 JSON 时不会降级为字符串，而会中止整个调用。
- 摘要不匹配在 meta 和 stats 两条路径都返回 `ErrInvalidMetaFile`，消息包含期望与实际摘要；写 JSON 之前完成校验，不会为当前坏对象生成输出。
- `DecodeMetaFile` 明确拒绝孙级 `meta_files`，错误类型为 `InvalidData`；它只支持根索引到叶子的一层展开。
- 取消只在每个 meta 节点开始前显式检查；`DecodeStatsFile` 没有自己的显式取消检查，但各存储操作仍接收同一 `Context`。长循环中的纯解密、校验和 JSON 转换也没有中途取消点。
- 输出路径直接拼接索引中的 `name`。本文件没有路径清理、目录预建、冲突检测或事务提交；这些约束交给 `Storage` 实现和可信备份元数据。

## 并发与资源生命周期

当前 Rust 实现完全串行：meta 节点、schema 以及 stats 索引均按切片顺序处理。共享存储由 `Arc<dyn Storage>` 持有，`DecodeMetaFile` 在调用统计解码时克隆一次 `Arc`，不复制存储本体；借用的 cipher 和 context 必须覆盖同步调用期间。读取到的密文、明文、protobuf 和 JSON `Vec<u8>` 都是函数局部所有权，迭代结束或错误返回时释放。

与 Go 版不同，Rust 没有 `errgroup` 或 8 槽 `WorkerPool`，因此没有并发写入、首错取消多个 worker 或等待 worker 收敛的生命周期。本实现的确定性更强但并行吞吐更低。若未来补齐 Go 并发语义，必须同时定义共享 `Storage` 的线程安全约束、取消传播、首错选择和部分输出行为，而不能只把循环替换成无界任务创建。

## 与 Go 版本的对应关系

主体流程与 `br/pkg/metautil/debug.go` 对齐：同样容忍空 meta 索引，读取和解密对象，对明文校验 SHA-256，解析 protobuf，限制 meta 最大深度，写 `jsons/<name>.json`，并从子 meta 的 schemas 继续展开 stats。Rust 独立测试 `debug_test.rs` 对照 Go 的 `debug_test.go::TestDecodeMetaFile`，覆盖 data 叶子的 SST 字段、schema 与完整 `StatsFileIndex` 字段，以及两个 stats blocks。

已验证的差异包括：

- Go `DecodeMetaFile` 使用 8 工作者的 `errgroup` 并发；Rust 顺序执行。Go stats 循环本身也是顺序执行。
- Go 通过 `utils.MarshalMetaFile`/`MarshalStatsFile` 生成 JSON；Rust 在本文件内实现专用转换器。测试证明关键字段可被 Rust `UnmarshalMetaFile`/`UnmarshalStatsFile` 读回，但这不等同于证明所有 protobuf 字段均与 Go JSON 完全覆盖。
- Go 的解密、protobuf 和 marshal 错误普遍再包一层 `errors.Trace`；Rust 仅对特定 I/O 和非法层级显式调用 `trace_err`，其他错误直接传播。
- Rust 在每个 meta 节点前显式检查 `ctx.is_cancelled()`；Go 依赖 `errgroup` 派生 context 和存储调用传播取消。
- Go BR 命令已接入包实现；当前 Rust BR 命令仍由本地同名 stub 截断，属于调用接线缺口，而不是本文件内部解码逻辑。

## 扩展指南

- 增加或修正 meta/schema/file/stats JSON 字段时，应修改 `marshal_meta_file_json`、`marshal_stats_file_json`、`file_json` 或 `schema_json` 中最窄的对应位置，并先确认 Go `br/pkg/utils/json.go` 的 protobuf JSON 约定。二进制字段要明确选择 hex 还是 Base64，默认值是否省略也应保持稳定。
- 改变解码链路（新索引类型、递归深度、校验算法或输出命名）时，应从 `DecodeMetaFile`/`DecodeStatsFile` 接入，同时更新独立的 `br/pkg/metautil/debug_test.rs`；不要把测试嵌入生产源文件。还应同步核对 Go 的 `debug.go` 与 `debug_test.go`，避免无依据简化 Go 行为。
- 要让 Rust BR CLI 真正使用此实现，需要在命令 crate 与 metautil crate 之间完成类型和依赖接线，并移除或绕过 `br/cmd/br/stubs.rs` 的同名占位；这超出本文件说明任务，且不能仅靠修改这里完成。
- 若恢复 Go 的 8 worker 并发，应增加多节点、错误取消和部分写入的独立测试，并评估大型 meta/stats 同时驻留造成的内存峰值以及存储后端的并发限制。
- 若强化安全边界，可在入口处验证对象名/输出键，但必须先确认所有 `Storage` 后端对键名的契约和现有备份兼容性，避免改变合法对象的寻址语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/metautil` 确认 `debug.rs`、crate 入口和对照测试均被索引；`explore "br/pkg/metautil/debug.rs ..."` 与 `node --file br/pkg/metautil/debug.rs --offset 180 --limit 240` 核对了本文件 26 个符号、两个公开入口及其内部调用。
- 生产源码：`br/pkg/metautil/debug.rs`（完整实现）、`br/pkg/metautil/lib.rs`（模块挂载和再导出）、`br/pkg/metautil/Cargo.toml`（crate 边界与依赖）。该目录不存在 `doc.go`。
- Go 对照：`br/pkg/metautil/debug.go`、`br/pkg/utils/json.go`，以及实际命令调用点 `br/cmd/br/debug.go`。
- Rust 接线核对：仓库范围搜索 `DecodeMetaFile|DecodeStatsFile|JSONFileFormat`，并读取 `br/cmd/br/debug.rs`、`br/cmd/br/stubs.rs`、`br/cmd/br/Cargo.toml`，确认命令 crate 当前使用本地占位而非本 crate。
- 测试证据：`br/pkg/metautil/debug_test.rs::test_decode_meta_file` 与 Go `br/pkg/metautil/debug_test.go::TestDecodeMetaFile`；二者都验证 data、schema 和 stats 输出，Rust 测试额外直接检查 JSON 的 hex/Base64/数值形态。本任务按计划不运行 Cargo，未执行这些测试。
- 交付结构通过任务指定的 11 标题检查；人工复核重点是实际接线限制、顺序/并发差异、错误与部分写入边界均未被描述成未验证的“已支持”能力。
