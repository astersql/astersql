# `pkg/util/keydecoder/keydecoder.rs`

## 文件定位

[`keydecoder.rs`](keydecoder.rs) 是 `astersql-util-keydecoder` crate 的核心实现，负责把 TiDB/TiKV 表键解析为面向诊断输出的 [`DecodedKey`](keydecoder.rs#L43)。crate 根 [`lib.rs`](lib.rs) 将本文件声明为私有模块并用 `pub use keydecoder::*` 再导出公开符号；[`Cargo.toml`](Cargo.toml) 将 crate 根设为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/util/keydecoder"` 标明 Go 对照目录。

该能力面向“解释已有存储键”，不是通用编解码器：公开入口 [`DecodeKey`](keydecoder.rs#L283) 只接受能被 `tablecodec::IsRecordKey` 或 `tablecodec::IsIndexKey` 识别的行键/索引键，再结合调用方提供的 `infoschema::InfoSchema` 补齐库、逻辑表、分区和索引名称。其结果字段与 Go 诊断 JSON 契约一致，可用于类似 `DEADLOCKS`、`DATA_LOCK_WAITS` 的 KEY_INFO 信息；这项用途由 Go 源码注释明确给出，Rust 文件本身没有直接连接这些上层表。

仓库当前可验证的 Rust 装配链是本文件 → [`lib.rs`](lib.rs) → 根 facade 的 `pkg::util::keydecoder`（[`pkg/lib.rs`](../../lib.rs)）。`pkg/executor/Cargo.toml` 声明了对该 crate 的依赖，但仓库 Rust 生产源码全文引用检查没有发现对 `DecodeKey` 的直接调用。因此当前事实是“API 已公开、已有独立测试、被 executor 声明为依赖”，不能据此声称 Rust 应用主链已经消费它。

## 核心职责

本文件承担四项相互关联的职责：

1. 定义稳定的诊断数据模型 `DecodedKey` 及其 JSON 字段名、省略规则。
2. 识别并解析记录键或索引键的头部，取得物理表/分区 ID、索引 ID 和键类别。
3. 通过 `InfoSchemaLookup` 把物理 ID 还原成库、逻辑表、分区和索引元数据，同时容忍 DDL/schema 变化导致的元数据缺失。
4. 对记录键解析 handle 类型和值；对索引键解析索引列值，并只在索引 ID 能与表元数据匹配时填入名称和值。

设计目标是尽可能返回可用的诊断片段，而不是把任何缺失都视为致命错误。完全未知的键类型、键头失败和记录键载荷失败会返回 `DecodeKeyError`；表或库元数据缺失以及索引载荷损坏则可能返回部分 `DecodedKey`。调用方必须把空字符串、零 ID、空向量理解为“未解析/不适用”，不能理解为实体确实具有这些标识。

## 主要符号

- `pub type HandleType = &'static str`：handle 类型标签的静态字符串别名。`IntHandle`、`CommonHandle`、`UnknownHandle` 分别为 `"int"`、`"common"`、`"unknown"`；`DecodedKey::default()` 中的空字符串表示尚未填入类型，与 `UnknownHandle` 的“已检查但类型不认识”不同。
- `pub struct DecodedKey`：公开结果类型。名称字段、handle/index 值、数据库/分区/索引 ID 和 `IsPartitionHandle` 在空、零或 `false` 时由 serde 省略；`TableID` 不带省略条件，JSON 中始终存在。字段保留 Go 风格大写命名，序列化时显式改为 snake_case。
- `fn is_zero`、`fn is_false`：仅服务于 serde 的跳过谓词，没有业务状态。
- `pub struct DecodeKeyError(String)`：公开错误类型，内部消息字段和构造器 `new` 私有；实现 `Display` 与 `std::error::Error`，没有错误分类或 source 链。
- `fn handleType(&dyn kv::Handle) -> HandleType`：通过 `Any` 下转识别 `kv::IntHandle`、`kv::CommonHandle`；遇到 `kv::PartitionHandle` 时递归检查其内层 handle；其他实现记录警告并返回 `UnknownHandle`。
- `pub(crate) struct KeyMetadata`：内部元数据快照，保存 schema/table/partition 身份、`(index_id, index_name)` 列表，以及 `table_found`、`schema_missing_for_table` 两个控制标记。
- `pub(crate) trait MetadataLookup`：按物理表或分区 ID 获取 `KeyMetadata` 的内部抽象。它把键解码与真实 infoschema 查询隔离，使独立测试能注入固定结果。
- `struct InfoSchemaLookup<'a>`：对借用的 `dyn infoschema::InfoSchema` 的私有适配器。其 `lookup` 先尝试 `TableByID`，失败后尝试 `FindTableByPartitionID`。
- `pub(crate) fn decodeKeyWithLookup`：实际状态机，供公开入口和同 crate 独立测试共用。
- `pub fn DecodeKey`：唯一公开解码函数，接受任意 `AsRef<[u8]>`，构造 `InfoSchemaLookup` 后委托给 `decodeKeyWithLookup`。

文件没有条件编译项，也没有异步函数、泛型业务类型或可变全局量。文件级 `allow(non_snake_case, non_upper_case_globals)` 是为了保留 Go API/字段命名。

## 执行流程

`DecodeKey` 的完整流程如下：

1. 将输入借用为字节切片并建立只借用本次 `InfoSchema` 的 `InfoSchemaLookup`。
2. `decodeKeyWithLookup` 同时用 `IsRecordKey`、`IsIndexKey` 做入口判定；两者都不匹配时立即返回包含原始字节调试表示的 `DecodeKeyError`，且不会查询元数据。
3. 将字节复制进 `kv::Key`，调用 `DecodeKeyHead` 取得 `table_or_partition_id`、`index_id`、`is_record_key`。先把物理 ID 写入 `result.TableID`，所以元数据未命中时仍能显示键内 ID。
4. `InfoSchemaLookup::lookup` 先以该 ID 调用 `TableByID`。普通表命中时读取表名、索引列表，并以表元数据中的 `db_id` 调用 `SchemaByID` 获取库身份。
5. 若普通表未命中，则调用 `FindTableByPartitionID`。分区命中时把 `TableID` 改为逻辑表 ID，并填入物理 `PartitionID`、分区名、库身份和逻辑表的索引列表；完全未命中只记录警告并保留物理 ID。
6. 如果表已找到但 `SchemaByID` 未找到所属库，`schema_missing_for_table` 为真：返回已填入表 ID/表名的部分结果，不再解析 handle 或索引载荷。这与 Go 的提前返回一致。
7. 记录键分支调用 `DecodeRecordKey`。成功后由 `handleType` 得到内层 handle 类型，通过 `String()` 取得可读值，并单独判断外层是否为 `PartitionHandle`。
8. 索引键分支调用 `DecodeIndexKey`。载荷解析失败时记录警告并返回此前的部分表身份；成功后填入 `IndexID`。只有元数据表存在且 `index_id` 命中 `metadata.indices` 时，才同时填入 `IndexName` 和 `IndexValues`。
9. 返回 `DecodedKey`。该流程不做 JSON 序列化；调用方可利用其 `serde::Serialize` 实现完成序列化。

一个重要不变量是 `TableID` 的语义随元数据质量变化：初始值是键中的物理 ID；普通表命中时仍为普通表 ID；分区命中时改为逻辑表 ID，而物理分区 ID 移到 `PartitionID`。因此不能只看 `TableID` 判断原始键前缀中的 ID。

## 数据与状态

`DecodedKey` 是本次调用独占的可变累积结果，没有跨调用缓存。`KeyMetadata` 也是每次 lookup 新建的拥有型快照：库表分区名称和索引名称均克隆为 `String`，避免结果借用 infoschema 内部对象。`InfoSchemaLookup` 自身仅保存一个不可变 trait-object 引用，其生命周期不超过 `DecodeKey` 调用。

输入在 `decodeKeyWithLookup` 中由 `&[u8]` 复制为 `kv::Key(key.to_vec())`，之后为了先解析键头再解析完整记录/索引载荷会克隆一次 `kv::Key`。元数据索引也被收集为 `Vec<(i64, String)>`，随后线性查找目标索引 ID。因此单次调用的额外空间与键长度及表索引数量线性相关；对诊断路径通常可接受，但如果扩展到高频热路径，应先测量这些复制和扫描成本。

默认值具有协议意义：`HandleType == ""` 表示非记录键或未走到 handle 解码；`UnknownHandle` 表示走到了 handle 分类但具体动态类型未知。`IndexValues` 只有索引载荷成功且索引元数据匹配时才非空；即使载荷已成功解码，索引元数据缺失或 ID 不匹配也会保留空向量。serde 测试证明除 `table_id` 外的零/空/false 字段会被省略。

## 依赖与调用关系

上游公开关系为：[`lib.rs`](lib.rs) 的 `pub use keydecoder::*` 暴露 `DecodeKey`、`DecodedKey`、错误类型和 handle 标签，根 facade [`pkg/lib.rs`](../../lib.rs) 再通过 `pkg::util::keydecoder` 暴露 crate。直接测试调用者是 [`keydecoder_test.rs`](keydecoder_test.rs)；其中既调用 crate 内部 `decodeKeyWithLookup`，也从 crate 根调用公开 `DecodeKey`。RustCodeGraph 的文件级 used-by 结果还列出两个 TopSQL 文件，但仓库文本复核没有找到它们对 keydecoder 符号的引用，因此不把它们记为已证实的调用边。

主要下游依赖是：

- `astersql-tablecodec`：键类型判定、键头/记录键/索引键解析，以及 `kv::Key` 和各类 `Handle`。
- `astersql-infoschema`：`InfoSchema` 查询、普通表到 schema 的解析和分区 ID 反查。
- `astersql-util-logutil`：元数据缺失、未知 handle、记录/索引载荷解码失败时的后台警告。
- `serde`：`DecodedKey` 的 JSON 兼容序列化元数据。
- 标准库 `fmt`：错误显示。

[`Cargo.toml`](Cargo.toml) 还声明 `astersql-meta-model`，但本文件没有直接引用它；不能把 manifest 中出现依赖等同于本文件存在调用边。dev-dependencies 为独立测试构造真实编码键和 mock infoschema 服务。

## 错误处理与边界

返回错误的边界有三类：未知键类型直接构造带原始字节的消息；`DecodeKeyHead` 错误保留其 `to_string()`；记录键载荷失败会记录包含底层错误的警告，但对外统一为 `cannot decode record key of table <id>`。`DecodeKeyError` 不暴露机器可匹配的枚举类别，调用方若依赖消息文本会比较脆弱。

部分成功边界同样重要：表元数据完全缺失时记录警告，但继续解析记录 handle 或索引载荷；普通表存在而所属 schema 缺失时提前返回表身份；索引载荷失败时返回已有表身份且不报错；索引 ID 未在表元数据中命中时保留 `IndexID`，但不填 `IndexName`/`IndexValues`。这些策略优先保障诊断可用性，修改为“任何异常都失败”会破坏 Go 兼容行为。

`handleType` 对分区 handle 递归分类内层对象，并用外层动态类型设置 `IsPartitionHandle`。当前 Rust 判断只匹配 `kv::PartitionHandle` 这一具体动态类型；Go 版本同时接受值类型和指针类型。是否存在另一种 Rust 包装表示需要由 `kv::Handle` 实现集合验证，不能从 Go 类型分支直接推断。

输入键会出现在未知类型错误消息中，索引解码警告也来自底层错误；若未来日志或错误暴露给不可信边界，应评估键字节是否包含需要脱敏的信息。当前实现没有长度上限、取消点或超时，因为所有工作是本地同步解析和元数据读取。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络连接，也没有静态可变状态。`DecodeKey` 只共享调用方传入的不可变 `&dyn InfoSchema`；并发安全性由 `InfoSchema` trait 对象的实现和调用方保证，本文件不持有跨调用引用。

所有分配都在调用栈对应的结果、键副本和元数据快照中，返回或报错后按 Rust 所有权规则释放。日志调用是唯一可观察的错误副作用。`DecodedKey` 返回后完全拥有其字符串与向量，可脱离 infoschema 生命周期使用或序列化。

若未来增加元数据缓存，必须明确 schema version、DDL 变更后的失效规则以及并发同步方式；当前每次 lookup 都观察传入 infoschema 的当前快照，正是元数据缺失可被当作正常诊断边界的原因。不要为了减少克隆而让公开结果借用 infoschema，除非愿意改变公开类型和调用生命周期。

## 与 Go 版本的对应关系

直接对照是 [`keydecoder.go`](keydecoder.go)。两版具有相同的公开数据字段、handle 标签、普通表优先/分区回退的元数据流程，以及“尽可能返回部分信息”的错误策略。Rust `InfoSchemaLookup` 对应 Go `DecodeKey` 中的 `TableByID`、`SchemaByTable` 和 `FindTableByPartitionID` 片段；Rust 将其抽象成 `MetadataLookup`，主要用于独立测试隔离。

关键对应关系包括：Rust `DecodeKeyHead`、`DecodeRecordKey`、`DecodeIndexKey` 分别对齐同名 Go 调用；Rust 的 `Handle::String()` 对齐 Go `handle.String()`；`schema_missing_for_table` 显式保存 Go 在 `SchemaByTable` 失败时立即返回的控制流；索引载荷失败时两版都记录警告并返回部分结果；分区键都把逻辑表 ID 与物理分区 ID 分开保存。

可见差异如下：Rust `HandleType` 是 `&'static str`，Go 是命名字符串类型；Rust 错误是本地 `DecodeKeyError`，Go 使用 `errors.Errorf`；Rust 的 `InfoSchema` API 通过表元数据 `db_id` 调 `SchemaByID`，Go 用 `SchemaByTable`；Rust 为序列化省略添加显式谓词；Rust 通过拥有型快照克隆名称和索引列表。Rust 对普通表命中后将 `TableID` 设为 `meta.id`，正常情况下与输入 ID 相同；Go 在普通表分支依靠初始物理 ID，不重复赋值。

[`keydecoder_test.rs`](keydecoder_test.rs) 的 `decode_key_matches_go_test_decode_key_scenarios` 明确复刻 [`keydecoder_test.go`](keydecoder_test.go) 的普通整数 handle、公共 handle、普通/分区索引、分区记录、非法键和 schema 变化后未知表场景。Rust 还增加了注入 lookup 的缺失 schema、JSON 省略规则等聚焦测试。Go 的 [`main_test.go`](main_test.go) 安装 goroutine 泄漏检查；Rust [`main_test.rs`](main_test.rs) 明确记录没有对应进程级 goroutine harness，这不是生产解码语义差异。

## 扩展指南

- 新增或修改输出字段时，从 `DecodedKey` 入手，同步 serde 字段名和省略语义，并在独立 [`keydecoder_test.rs`](keydecoder_test.rs) 增加 JSON 契约测试；同时核对 Go `DecodedKey`，避免 KEY_INFO 消费方收到不兼容结构。
- 修改普通表、分区或 schema 解析时，从 `InfoSchemaLookup::lookup` 入手。必须保持“物理 ID → 逻辑 TableID + PartitionID”的不变量，并覆盖表不存在、schema 缺失和分区反查场景。
- 增加新 handle 实现时，修改 `handleType` 及 `IsPartitionHandle` 判定，并在独立测试文件构造真实编码键。不要把测试放回生产源文件；未知实现应继续有明确日志和 `UnknownHandle` 行为。
- 修改记录/索引键错误策略时，从 `decodeKeyWithLookup` 的两个分支入手。需分别测试键头损坏、记录载荷损坏、索引载荷损坏、索引元数据不匹配，因为它们当前并非统一失败语义。
- 若接入上层运行时，应让真实调用点传入与诊断事件一致的 infoschema 快照，明确如何序列化/记录 `DecodeKeyError`，并添加调用层独立回归测试。目前没有可证实的 Rust 生产调用，不应只改 manifest 依赖来宣称接入完成。
- 若优化性能，先基准化 `key.to_vec()`、`kv::Key::clone()`、全部索引名称克隆与线性 ID 扫描。缓存或借用优化必须评估 DDL 变化、对象生命周期和并发一致性，不能以牺牲部分结果兼容语义为代价。

兼容性风险主要是 JSON 字段/省略规则和 Go 对齐的部分成功策略；正确性风险集中在物理/逻辑表 ID 转换与 schema 缺失早退；性能风险集中在键和元数据的复制及索引线性扫描。该文件不处于已证实的生产热路径，任何性能结论都需要实际接入点和基准支持。

## 验证依据

- Rust 源码：[`keydecoder.rs`](keydecoder.rs)，完整核对常量、`DecodedKey`、`DecodeKeyError`、`handleType`、`KeyMetadata`、`MetadataLookup`、`InfoSchemaLookup::lookup`、`decodeKeyWithLookup` 和 `DecodeKey`。
- crate/装配：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 与根 facade [`pkg/lib.rs`](../../lib.rs)，核对 crate 名、Go 移植元数据、依赖、crate 根和再导出链；`pkg/executor/Cargo.toml` 仅证明依赖声明，不证明源码调用。
- Go 对照：[`keydecoder.go`](keydecoder.go)，核对输出字段、handle 分类、元数据解析顺序、部分成功策略与错误边界。
- 独立测试：[`keydecoder_test.rs`](keydecoder_test.rs) 覆盖未知键、整数/公共 handle、分区身份、普通索引、表/schema 缺失、JSON 字段，以及完整 Go 场景；[`keydecoder_test.go`](keydecoder_test.go) 提供原始移植语义；两个 `main_test` 文件只用于核对测试运行时差异。
- RustCodeGraph：`status` 报告索引可用（11,467 个文件、307,296 个节点）；`files --filter pkg/util/keydecoder` 报告 `keydecoder.rs` 含 15 个符号；`node --file ... --offset 1 --limit 400` 返回完整 288 行源码，并报告三个文件级使用者；`query` 精确定位 Rust `DecodeKey`、`decodeKeyWithLookup`、`handleType`。路径限定的 `callers`/`callees` 没有返回边明细，且文件级结果中的两个 TopSQL 文件经 `rg` 未发现符号引用，因此文档只把独立测试记为已证实直接调用者。
- 仓库文本复核：对 `astersql_util_keydecoder`、`keydecoder::`、`DecodeKey(` 的 Rust 搜索只发现 facade、crate 内实现和独立测试；对 Cargo manifest 的搜索发现 `pkg/executor/Cargo.toml` 依赖声明。
- 人工复核：确认文档能回答文件为何存在、普通表/分区记录/索引如何运行、哪些异常返回错误或部分结果、ID 字段如何变化、当前接线状态以及扩展时应修改的符号与独立测试位置；未把未返回的图边或 manifest 依赖推断为生产调用。
