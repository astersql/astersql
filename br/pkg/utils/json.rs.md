# `br/pkg/utils/json.rs`

## 文件定位

本文件属于 library crate `astersql-br-pkg-utils`（`br/pkg/utils/Cargo.toml`），由 `br/pkg/utils/lib.rs` 通过 `#[path = "json.rs"] pub mod json` 公开挂载。它移植 Go 的 `br/pkg/utils/json.go`，在 BR 的 protobuf 元数据对象与便于人工查看、跨语言交换的 JSON 之间转换，覆盖 `BackupMeta`、拆分元数据 `MetaFile` 和统计分片 `StatsFile` 三种根对象。

这里不是通用 JSON 或 SQL JSON 实现，也不负责文件、对象存储或网络 I/O。RustCodeGraph 将该文件识别为 807 行、50 个符号的纯转换模块；仓库引用复核显示，当前 Rust 直接使用者仅有 `br/pkg/utils/json_test.rs` 和 `br/pkg/metautil/debug_test.rs`，尚无 Rust 生产文件调用这六个公开入口。相对地，Go 生产路径已在 `br/cmd/br/debug.go` 使用 `MarshalBackupMeta`/`UnmarshalBackupMeta`，并在 `br/pkg/metautil/debug.go` 使用 `MarshalMetaFile`/`MarshalStatsFile`。因此，本文件目前是公开且有兼容性测试的 Rust 格式实现，但不能据此宣称 Rust BR 调试主链已经完成接线。

## 核心职责

- 提供六个成对公开 API：`MarshalBackupMeta`/`UnmarshalBackupMeta`、`MarshalMetaFile`/`UnmarshalMetaFile`、`MarshalStatsFile`/`UnmarshalStatsFile`。编码结果是紧凑 UTF-8 JSON 字节；解码结果是本 crate `stubs::kvproto::brpb` 中的 protobuf 兼容类型。
- 将 `File.sha256/start_key/end_key` 与 `RawRange.start_key/end_key` 编为小写十六进制；将 `File.cipher_iv` 和 `MetaFile.backup_ranges` 的边界键编为标准、带 `=` padding 的 Base64。两类键编码有意不同（`make_json_file`、`make_json_raw_range`、`make_json_meta_file`）。
- 把 protobuf 中以 JSON 字节保存的字段提升为嵌套 JSON 值：`Schema.db/table/stats`、`BackupMeta.ddls`、`MetaFile.ddls` 的每个元素，以及 `StatsBlock.json_table`；反序列化时再将嵌套值编码回字节（`make_json_schema`、`from_json_schema` 等）。
- 用 `insert_string`、`insert_u64`、`insert_hex` 和各 `*_to_map` 辅助函数实现类似 Go `encoding/json` 的 `omitempty`：空字符串、空字节、零整数、`false` 及空数组通常不输出。
- 在进入字段填充前严格验证已知字段的 JSON 类型和整数范围，避免 `Value::as_*` 失败后静默退回默认值（`validate_backup_meta`、`validate_meta_file`、`validate_stats_file`）。未知字段没有被显式拒绝，会在重建 protobuf 对象时被忽略。

## 主要符号

- `pub fn MarshalBackupMeta(&BackupMeta) -> Result<Vec<u8>, SharedError>`：调用 `make_json_backup_meta` 构造 `serde_json::Value`，再用 `serde_json::to_vec` 输出 JSON。`BackupMeta.ddls` 即使为空字节也会尝试解析，因此非法或空载荷会失败。
- `pub fn UnmarshalBackupMeta(&[u8]) -> Result<BackupMeta, SharedError>`：依次执行 `serde_json::from_slice`、`validate_backup_meta`、`from_json_backup_meta`；缺失的标量和数组保持 protobuf 默认值，缺失 `ddls` 则写回字节 `null`。
- `MarshalMetaFile`/`UnmarshalMetaFile`：处理 `data_files`、`raw_ranges`、`schemas`、逐项 DDL 和 `backup_ranges`。其中 backup range 的键使用 `b64_encode`/`b64_decode`。
- `MarshalStatsFile`/`UnmarshalStatsFile`：处理 `blocks` 数组；每个 `StatsBlock` 的 `json_table` 是嵌套 JSON，`physical_id` 是有符号 `i64`。
- `validate_field`、`validate_array`、`object` 及三个根验证函数：允许字段缺失或为 `null`，但存在的非空已知字段必须符合声明的 string/bool/整数/array/object 类型；`tiflash_replicas` 还必须能落入 `u32`。
- `make_json_file`/`from_json_file`、`make_json_raw_range`/`from_json_raw_range`、`make_json_schema`/`from_json_schema`：三组叶子对象转换器，负责特殊字节编码和嵌套 JSON。
- `make_json_backup_meta`/`from_json_backup_meta`、`make_json_meta_file`/`from_json_meta_file`、`make_json_stats_file`/`from_json_stats_file`：三组根对象组装器，保留输入数组顺序，遇到任一子项错误立即返回。
- `file_to_map`、`map_to_file`、`schema_to_map`、`backup_meta_to_map`：只处理无需特殊编码的标量字段；特殊字段由上层转换器覆盖或补充。
- `b64_encode`/`b64_decode`：内部实现标准 Base64。解码仅忽略 CR/LF，要求长度为 4 的倍数、padding 只出现在最后一组，并拒绝非法字符或位置。
- `trace_err`：用 `astersql_errors::Trace` 包装 hex/Base64 解码错误；`invalid_json_type` 统一构造 `InvalidData` 类型错误。

本文件没有自定义结构体、trait、模块级可变变量或条件编译项；公开面只包含上述六个函数，其余符号均为模块私有实现。

## 执行流程

编码流程以 `MarshalBackupMeta` 为例：

1. `backup_meta_to_map` 写入非零/非空的头部标量，如 cluster/version/TS，并仅在 `is_raw_kv == true` 时输出布尔字段。
2. `make_json_backup_meta` 按原顺序映射 `files`、`raw_ranges` 和 `schemas`。文件与 raw range 的键转为 hex，文件 IV 转为 Base64；schema 内的 db/table/stats 字节先解析成 JSON 树。
3. `ddls` 字节无条件经 `serde_json::from_slice` 解析；结果不是 `null` 才插入对象。
4. `serde_json::to_vec` 将最终 `Value::Object` 输出为紧凑字节。`MetaFile` 与 `StatsFile` 入口遵循相同骨架，但分别处理逐项 DDL/Base64 backup range，以及统计 block。

解码流程以 `UnmarshalBackupMeta` 为例：

1. `serde_json::from_slice` 先验证整体 JSON 语法。
2. `validate_backup_meta` 要求根为 object，并递归验证已知文件、raw range、schema 数组元素及标量类型；`null` 与缺失字段被允许。
3. `from_json_backup_meta` 从默认 protobuf 值开始逐字段填充。数组按 JSON 原顺序追加；hex/Base64 解码失败立即中止，不返回半成品。
4. `ddls` 取存在值或 `Value::Null`，再通过 `serde_json::to_vec` 保存回 protobuf 字节。MetaFile 和 StatsFile 同样先验证后填充，所以已知字段类型错误不会被误当成缺省值。

往返契约是 JSON 逻辑树相等，而不是输出字节完全相等：`serde_json::Map` 的键序和源文本空白不属于接口保证；数组顺序和字符串内容则会保留。

## 数据与状态

所有转换状态都是函数栈上的 `serde_json::Value`、`Map<String, Value>`、临时 `Vec` 和新建 protobuf 对象。函数不缓存结果，不读取环境变量，不访问磁盘，也没有全局状态。编码会为 JSON 树、字符串以及 hex/Base64 文本分配内存；解码会为输入对应的值树和输出 protobuf 字段重新分配。

数字保持整数表示：无符号字段通过 `Value::as_u64`，`version`/`physical_id` 通过 `as_i64`；`version` 在验证阶段限制到 `i32`，`tiflash_replicas` 限制到 `u32`。浮点数或以字符串表示的数字不会被接受。`file_to_map`、`schema_to_map` 和 `backup_meta_to_map` 省略零值；解码缺失值时保留 protobuf 默认零值。

嵌套 JSON 字节不是原始字节透明往返：它们先解析为 JSON 值，再重新序列化，因此空白和对象键排列可能变化，但逻辑值保持。缺失的 `Schema.db`、`BackupMeta.ddls`、`StatsBlock.json_table` 会被写为字节 `null`；`table`/`stats` 缺失或显式 `null` 则保留空字节。`MetaFile.ddls` 是“每项一段 JSON 字节”，不同于 `BackupMeta.ddls` 的“整段 JSON 字节”。

## 依赖与调用关系

- 上游装配：`br/pkg/utils/lib.rs` 公开 `json` 模块，但没有在 crate 根再导出六个函数；调用方应经 `astersql_br_pkg_utils::json::*` 访问。
- protobuf 边界：`crate::kvproto::brpb::{BackupMeta, BackupRange, File, MetaFile, RawRange, Schema, StatsBlock, StatsFile}` 实际来自本 crate 的 `stubs` 再导出。该 crate 的 Cargo 注释明确说明当前 darwin arm64 形态使用本地 proto/KV/SQL stubs。
- JSON/错误依赖：`serde_json::{Value, Map, Number}` 提供动态 JSON 树；`astersql-errors` 提供 `SharedError` 与 `Trace`。`br/pkg/utils/Cargo.toml` 显式声明这两个依赖。
- 键编码依赖：同 crate `key::{hex_encode, hex_decode_string}` 负责 hex；Base64 在本文件内部实现，没有引入 Base64 crate。
- 已验证的 Rust 上游：`br/pkg/utils/json_test.rs` 调用全部六个入口；`br/pkg/metautil/debug_test.rs` 调用 `UnmarshalMetaFile` 与 `UnmarshalStatsFile` 解析调试输出。全仓 Rust 限定搜索未发现生产文件直接调用。
- 对应的 Go 生产主链：`br/cmd/br/debug.go` 将备份元数据在 protobuf 与 JSON 间转换；`br/pkg/metautil/debug.go` 将拆分元数据和统计文件转成 JSON。这些是 Go 对照和预期用途证据，不是 Rust 已接线证据。
- RustCodeGraph `query` 为每个公开入口同时返回 Go、Rust及部分 stub 同名候选；精确 Rust 函数 ID 的 `callers` 查询没有输出，因此调用现状用限定 `*.rs` 引用搜索补充核验，未把同名 stub 当作直接调用边。

## 错误处理与边界

三类错误都通过 `SharedError` 返回：JSON 语法/嵌套 JSON 解析与序列化错误由 `SharedError::new` 包装；已知字段类型错误由 `invalid_json_type` 生成 `std::io::ErrorKind::InvalidData`；hex/Base64 解码错误再经 `trace_err` 包装。任何数组元素失败都会使整次转换失败，不会暴露部分填充对象。

边界规则包括：根值必须是 object；已知数组字段必须是 array，数组元素必须符合相应 object 结构；已知字段可缺失或为 `null`；未知字段被忽略。无 padding、包含空格、padding 超量/错位等非标准 Base64 会被拒绝，但 CR/LF 会被忽略。hex 规则由 `hex_decode_string` 决定，错误向上传播。编码空 `BackupMeta.ddls`、空 `Schema.db` 或空 `StatsBlock.json_table` 会因其不是合法 JSON 而失败，这与 Go 测试意图保持一致。

当前校验保证转换前类型安全，但不验证业务语义，例如 `cipher_iv` 长度、SST checksum 长度、区间 start/end 顺序、DDL 内容结构、schema 是否为有效 TiDB 模型，或统计 JSON 的具体模式；调用方必须保证这些 protobuf 字段在业务层有效。字段扩展若只加入填充逻辑却漏加验证，错误类型可能被静默当作缺省值，因此验证器和转换器必须同步维护。

## 并发与资源生命周期

本文件无锁、原子量、线程局部变量、异步任务、通道、文件句柄或网络连接。公开函数只借用输入并返回拥有所有权的新 `Vec<u8>` 或 protobuf 对象，没有借用逃逸；临时 JSON 树在返回时释放。只要输入对象本身能安全共享，多个线程可独立调用这些纯转换函数，因为它们没有共享可变状态。

资源成本主要是完整 materialization：输入 JSON、动态 `Value` 树、protobuf 字节字段和输出 JSON 会在转换期间共存，hex 文本约为原字节两倍，Base64 约为四分之三膨胀的逆比（输出约为输入的 4/3）。该实现不提供流式解析/输出或大小上限；对超大 metadata 文件，峰值内存和 CPU 随输入总量增长。调用方负责限制不可信输入尺寸，并负责把返回字节写入外部存储。

## 与 Go 版本的对应关系

六个公开函数及三层对象转换结构对应 `br/pkg/utils/json.go` 的同名函数与 `jsonFile`、`jsonRawRange`、`jsonSchema`、`jsonBackupMeta`、`jsonMetaFile`、`jsonStatsBlock`、`jsonStatsFile` 包装结构。Rust 手工构造 `serde_json::Map` 来模拟 Go protobuf 字段与 `omitempty` 的组合效果；`br/pkg/utils/json_test.rs` 复用 Go `json_test.go` 的真实 backup/meta/stats 夹具，并用解析后的 `serde_json::Value` 比较逻辑相等。

两侧关键一致点是：File/RawRange 键用 hex；Go `[]byte` 默认 JSON 编码对应 Rust 对 IV 和 backup range 使用的标准 Base64；schema 和统计载荷嵌套为 JSON；BackupMeta DDL 是单个 JSON 值，MetaFile DDL 是值数组；零值字段省略；缺失嵌套值按 `null`/空字节规则恢复。

实现方式存在语言差异。Go 借助匿名嵌入 protobuf 结构和 `encoding/json` 自动处理普通字段，Rust 必须在 `*_to_map`/`from_*` 中显式列举字段，并新增递归类型校验以避免动态 `Value` 的静默丢失。Go 的标准库负责 Base64，Rust 在本文件手写等价编码器/解码器；后续修改必须持续用负面用例核对 padding、换行与非法字符行为。当前 Rust proto 类型来自本地 stubs，而 Go 使用 `github.com/pingcap/kvproto/pkg/brpb`，新增字段不会自动出现在 Rust JSON 中，必须同步 stub、映射、验证及双侧夹具。

## 扩展指南

- 新增 protobuf 标量字段时，同步修改相应 `*_to_map` 与 `from_json_*`，并在根或叶子 `validate_*` 中声明精确 JSON 类型和整数范围；确认零值是否应省略，不能只改单向编码。
- 新增字节字段前先确定线格式：键/摘要通常可能是 hex，Go `[]byte` 默认是 Base64，嵌套模型则应解析为 JSON 值。不可复用错误的辅助函数，尤其不要混淆 `File` 的 hex key 与 `backup_ranges` 的 Base64 key。
- 修改 Base64 实现时，扩展 `br/pkg/utils/json_test.rs` 的独立负面测试，覆盖 0/1/2/3 字节尾部、CR/LF、缺失/错误 padding、空格和非法字符，并与 Go `encoding/base64.StdEncoding` 行为核对。
- 修改嵌套 JSON 或 `omitempty` 规则时，同步 `br/pkg/utils/json_test.rs` 与 `br/pkg/utils/json_test.go` 的 BackupMeta、MetaFile、StatsFile 夹具；比较 JSON 逻辑值，不依赖键序或空白。Rust 测试继续放在独立 `json_test.rs`，不要内嵌到生产源文件。
- 若将 Rust 生产调试路径接到本模块，应从 Go 的 `br/cmd/br/debug.go`、`br/pkg/metautil/debug.go` 核对调用时机、读写载体和错误注释链，并新增调用方级测试；公开模块存在本身不能替代接线验证。
- 兼容性风险最高的是落盘字段名、hex/Base64 选择、零值省略、整数有符号/宽度及嵌套 `null` 语义。性能风险来自整树解析和重复分配；如需流式化，应先保证输出逻辑树和错误原子性不变，而非直接替换现有格式。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utils` 确认 Rust/Go 实现与测试均已索引；`node --file br/pkg/utils/json.rs --offset 1 --limit 260` 和 `--offset 261 --limit 600` 读取完整 807 行，并报告目标被 58 个文件建立文件级使用关系；`query` 精确确认六个公开函数的 Rust 定义 ID、签名及 Go 同名候选。对六个 Rust 函数 ID执行 `callers --limit 50` 无输出，未将其当作生产调用证据。
- 源码与 crate：`br/pkg/utils/json.rs`（全部实现）、`br/pkg/utils/lib.rs`（公开模块装配）、`br/pkg/utils/Cargo.toml`（crate 名、library 入口、`serde_json`/`astersql-errors` 依赖及本地 stub 边界）、`br/pkg/utils/key.rs`（hex 辅助的直接依赖）。目标包不存在 `doc.go`。
- Go 对照：RustCodeGraph 读取 `br/pkg/utils/json.go` 全部 371 行；限定生产 Go 搜索确认 `br/cmd/br/debug.go` 和 `br/pkg/metautil/debug.go` 的真实调用位置。
- 测试：RustCodeGraph 读取 `br/pkg/utils/json_test.rs` 的夹具说明与第 580 至 678 行测试入口；测试覆盖三类往返、空嵌套 JSON 错误、缺省 `null`、非标准 Base64、错误已知字段类型。Go 对照测试为 `br/pkg/utils/json_test.go`。另以 Rust 引用搜索确认 `br/pkg/metautil/debug_test.rs` 的解码调用。
- 本任务为纯文档分析，按计划未运行 Cargo。交付检查使用任务指定的结构命令，要求文档存在且恰好包含这 11 个固定二级标题；同时人工复核唯一生产物为本文件、没有修改 Rust/Go/Cargo/`plan.md`，且扩展建议保持测试在独立文件中。
