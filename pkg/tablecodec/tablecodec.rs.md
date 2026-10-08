# `pkg/tablecodec/tablecodec.rs`

## 文件定位

`tablecodec.rs` 是 `astersql-tablecodec` crate 的主实现文件，由同目录 [`lib.rs`](lib.rs) 通过 `include!("tablecodec.rs")` 纳入 crate。它位于逻辑表模型与底层有序 KV 字节格式之间，负责把 table ID、index ID、行 handle、列 Datum 和 DDL 临时索引操作编码为可排序、可持久化的 key/value，也负责逆向解析这些格式。生产调用已经接入会话 DML/扫描（如 `pkg/session/runtime/row_codec.rs`）、执行器、Lightning KV 后端以及 store 错误诊断路径；它不是门面或桩。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-tablecodec`，默认使用 Classic 内核，`nextgen` feature 仅向 `kerneltype-dependency/nextgen` 透传。实现依赖 canonical `structure::kv` handle/key 类型、`meta/model` 表索引元数据、`util/codec` 的 mem-comparable 编码、`util/rowcodec` 的新行格式、类型/排序规则与标准数据库错误。

## 核心职责

1. 定义并识别 TiDB 兼容的表 key 空间：`t{tableID}_r{handle}`、`t{tableID}_i{indexID}{values...}`，以及 `m{encoded-key}{HashData}{encoded-field}` meta key（`tablePrefix`、`recordPrefixSep`、`indexPrefixSep`、`metaPrefix`）。
2. 在整数 handle、common handle 和带 partition ID 的 `PartitionHandle` 之间编码、解码并保持字节排序语义（`EncodeRowKey*`、`DecodeRecordKey`、`DecodeRowKey`、`DecodeIndexHandle`）。
3. 在旧行格式与 `rowcodec` 新行格式之间分派，完成 SQL Datum 的存储态展平/恢复以及按列解码（`EncodeRow`、`EncodeOldRow`、`flatten`、`Unflatten`、`DecodeRowToDatumMap`）。
4. 生成和解析普通、唯一、全局及 common-handle 索引 key/value；兼容 legacy、IndexValueVersion0 和 clustered common-handle V1 三种物理布局（`GenIndexKey`、`GenIndexValuePortal`、`DecodeIndexKVWithCollate`、`SplitIndexValue`）。
5. 支持 DDL backfill/merge 使用的临时索引 ID 与操作历史编码（`IndexKey2TempIndexKey`、`TempIndexValueElem::{Encode, DecodeOne}`、`FilterOverwritten`）。
6. 提供范围构造、key 分类、table ID 校验等上层扫描辅助函数（`GetTableHandleKeyRange`、`GetTableIndexKeyRange`、`VerifyTableIDForRanges`）。

## 主要符号

- 格式常量：`prefixLen = 11`，`RecordRowKeyLen = 19`；`CommonHandleFlag = 127`、`PartitionIDFlag = 126`、`IndexVersionFlag = 125`、`RestoreDataFlag = rowcodec::CodecVer` 描述索引 value 的可选段；`MaxOldEncodeValueLen = 9` 是新旧索引 value 分派边界。
- 初始化：`init` 向 `kv::DecodeTableIDFunc` 安装闭包，使 `kv` 无需反向依赖本 crate 即可解析 table ID。该函数本身不会被 Rust 自动当作包初始化器调用，调用责任在 crate/应用装配侧；源码中使用 `unsafe` 写全局函数指针。
- 行 key：`EncodeRowKey`、`EncodeRowKeyWithHandle` 和 `EncodeRecordKey` 生成记录 key；后者遇到 `PartitionHandle` 时改用真实 partition ID 的记录前缀。`DecodeRecordKey` 同时返回 table ID 与 handle；`DecodeRowKey` 先移除 keyspace 前缀，只返回 handle。
- 行 value：`EncodeRow` 在 `rowcodec::Encoder.Enable` 时使用新格式，否则调用 `EncodeOldRow` 写入交替的 `(columnID, value)`；`DecodeRowToDatumMap` 根据 `rowcodec::IsNewFormat` 分派到新、旧解码器。
- 类型转换：`flatten` 把 timestamp、duration、enum、set、bit 转为稳定的基础存储 Datum；`Unflatten` 依据 `FieldType` 恢复时区、FSP、collation、enum/set/bit 等 SQL 语义。
- 索引 key：`GenIndexKey` 计算 `distinct`（唯一索引只要含 NULL 就不是 distinct）、按前缀长度截断字符串列并编码列值；非 distinct key 追加 handle。GlobalIndexVersionV1+ 还要求非聚簇表的 `PartitionHandle`，并在 key 中追加 partition ID 防止跨分区 handle 冲突。
- 索引 value：`GenIndexValuePortal` 根据 `TableInfo.IsCommonHandle && CommonHandleVersion == 1` 选择 `GenIndexValueForClusteredIndexVersion1`，否则进入 `genIndexValueVersion0`；`IndexValueSegments`/`SplitIndexValue` 则将 value 拆成 common handle、partition ID、restored values 和 int handle。
- 索引解码：`HandleStatus` 的 `HandleDefault`、`HandleIsUnsigned`、`HandleNotNeeded` 控制是否及如何追加解码后的 handle；`DecodeIndexKVWithCollate` 按 value 长度和版本号选择旧 collation、clustered V1 或通用新布局。
- 临时索引：`TempIndexPrefix`/`IndexIDMask` 在 index ID 高位标记临时索引；`TempIndexValue` 是按时间追加的 `TempIndexValueElem` 列表，`TempIndexValueExt::FilterOverwritten` 从后向前按 handle 去重，仅保留最近操作。

## 执行流程

写入行记录时，上层先通过 `EncodeRowKeyWithHandle` 形成记录 key；随后 `EncodeRow` 校验 `row.len() == colIDs.len()`，清空可复用缓冲，并按 encoder 开关选择新 `rowcodec::Encoder::Encode` 或旧 `(columnID, flattened value)` 序列。旧格式空行必须写一个 `codec::NilFlag`，因为 nil value 不能直接写入 KV。

读取行记录时，`DecodeRecordKey` 验证 `t` 和 `_r` 头、解出 table ID，再以剩余长度是否为 8 字节区分 int/common handle。行 value 经 `DecodeRowToDatumMap` 判断格式：新格式构造请求列 `ColInfo` 并调用 `DatumMapDecoder`；旧格式循环 `CutOne` 读取列 ID/value，只恢复请求列并在数量满足后提前停止。handle 列由 `DecodeHandleToDatumMap` 补入，且需要 restored data 的列不会从 handle 直接恢复。

写索引时，`GenIndexKey` 先判定唯一性、执行前缀索引截断，再写 `t{physicalTableID}_i{indexID}` 和有序列值；非 distinct 索引把 handle 放进 key。`GenIndexValuePortal` 独立选择 value 布局：legacy 紧凑格式、可扩展 version 0，或 clustered common-handle V1；全局索引追加 partition ID，需要 collation 恢复的列追加 rowcodec restored-data 段，untouched 状态写入尾部标记。

读索引时，`DecodeIndexKVWithCollate` 先按 `MaxOldEncodeValueLen` 与 `getIndexVersion` 分派。各路径用 `CutIndexKeyNew` 切出索引列；restored-data 路径通过 `decodeRestoredValues`/`decodeRestoredValuesV5` 恢复截断或排序后的字符串；最后按 `HandleStatus` 从 key suffix 或 value 段恢复 handle。`DecodeIndexHandle` 还处理 global V1 key/value 同时携带 partition ID 的情况：若 key 已解出 `PartitionHandle`，先取其 inner handle，再以 value 中的 authoritative partition ID 包装，避免嵌套。

DDL 临时索引把普通 index ID 就地改写为 `TempIndexPrefix | indexID`。每个 `TempIndexValueElem` 根据 normal/deleted、distinct/non-distinct 四种组合编码不同长度字段；global distinct delete 额外编码 partition ID。`DecodeTempIndexValue` 连续消费元素，merge 阶段可用 `FilterOverwritten` 删除同一 handle 的旧操作。

## 数据与状态

本文件的核心状态是外部持久化字节协议而非内存对象。table/index ID 使用 `codec::EncodeInt` 维持字典序；整数行 handle 的标准记录 key 长 19 字节，其他长度按 common handle 解读。索引 value 首字节在新布局中是尾段长度，随后可有版本、handle、partition、restored-data 段，解析顺序与 flag 常量是磁盘兼容契约。

`Datum` 在写入时可能发生有损表示转换：timestamp 转 UTC packed uint、duration 转纳秒整数、enum/set/bit 转无符号整数；恢复必须依赖准确的 `FieldType`、时区和 collation。索引前缀截断对 binary/ASCII 按字节计数，对其他 UTF-8 charset 按字符计数。

函数广泛接收/返回拥有所有权的 `Vec<u8>`、`Vec<Datum>` 和 boxed trait object；`GetIndexKeyBuf`、`EncodeRow`、`TempIndexValueElem::Encode` 等显式复用调用方缓冲以减少分配。`HashMap<i64, ...>` 保存请求列、列偏移与行结果，其遍历顺序不参与持久化格式。

## 依赖与调用关系

下游依赖中，`codec` 提供整数、Datum、bytes 的可比较编码及切割；`rowcodec` 提供新行格式、restored data 与 keyspace prefix 处理；`kv` 提供 `Key`、`Handle`、`PartitionHandle`、`KeyRanges`；`model` 提供表/索引版本、全局索引和列元数据；`types/mysql/collate/charset` 决定 SQL 类型与前缀截断语义；`dbterror/terror/errno` 生成兼容错误码。

RustCodeGraph 显示 `EncodeRow -> rowcodec::Encoder::Encode | EncodeOldRow`，`DecodeIndexKVWithCollate -> decodeIndexKvOldCollation | decodeIndexKvForClusteredIndexVersion1 | decodeIndexKvGeneral`，`GenIndexValuePortal -> GenIndexValueForClusteredIndexVersion1 | genIndexValueVersion0`，`VerifyTableIDForRanges -> DecodeTableID`。图中 `GenIndexKey` 的直接内部依赖还包括 `TruncateIndexValues`、`GetIndexKeyBuf` 与 `appendTableIndexPrefix`。

生产上游证据包括：`pkg/session/runtime/row_codec.rs` 同时调用 `EncodeRowKeyWithHandle`、`GenIndexKey` 和 `GenIndexValuePortal` 完成 DML 编码；`pkg/lightning/backend/kv/base.rs` 使用相同索引生成入口；`pkg/store/driver/txn/error.rs` 调用 `DecodeRecordKey`/`DecodeIndexKV` 解释冲突键；`pkg/session/runtime/relational_scan.rs`、`pkg/executor/typed_point_get.rs` 和 `pkg/store/helper/helper.rs` 使用记录 key/table ID API。因而本文件处于 SQL 执行、导入与存储诊断共享的格式边界。

## 错误处理与边界

公开解析函数通常返回 `errors::SharedError`。非法 table/record/index key 通过 `errInvalidKey`、`errInvalidRecordKey`、`errInvalidIndexKey` 生成稳定 SQL 错误码；底层 codec/rowcodec 错误由 `trace_error` 转成共享错误。`DecodeTableID` 是特例：非表 key、API V2 前缀后仍非表 key或整数解码失败均静默返回 0，供分类/诊断路径使用。

关键前置条件必须由调用者保证：`hasTablePrefix` 直接索引首字节，`hasRecordPrefixSep` 直接索引两个字节；`CutRowKeyPrefix`、`DecodeIndexID`、若干 split/decode 函数按固定偏移切片；`DecodeIntHandleInIndexValue` 要求至少 8 字节。`TempIndexValueElem::DecodeOne` 也按 flag 直接读取长度和尾字节，只有未知 flag 显式返回错误，因此不得把未经长度校验的任意外部字节直接送入这些低层 helper。

`EncodeRow`/`EncodeOldRow` 明确拒绝行值与 column ID 数量不一致。`GenIndexKey` 对 GlobalIndexVersionV1+ 拒绝聚簇索引，也拒绝不是 `PartitionHandle` 的 handle。`VerifyTableIDForRanges` 拒绝 table ID 非正数以及同一 partition range 组内混用多个 table ID。`Unflatten` 对 enum 的无效值有意回退为空 enum，而 set、时间、bit 等转换错误继续传播，这是与 Go 行为相关的兼容差异点。

## 并发与资源生命周期

除 `init` 外，编解码函数不持有跨调用可变状态，没有锁、异步任务、通道、事务或 I/O；其生命周期限于输入所有权、临时缓冲与返回值。并发调用通常彼此独立，但调用方若复用缓冲，必须维持 Rust 所有权边界且不能假定输入内容在调用后保留。

唯一全局变更是 `init` 在 `unsafe` 块内设置 `kv::DecodeTableIDFunc`。装配层应只在初始化阶段调用，避免与读取该函数指针的线程并发写入。collation 的全局开关通过 `collate::NewCollationEnabled()` 被 `DecodeIndexKV` 读取；需要确定性行为的调用者应选用显式参数版本 `DecodeIndexKVWithCollate`。

处理 `Box<dyn kv::Handle>` 时，代码通过 `Copy()` 创建语义副本，`PartitionHandle` 包装/解包不会共享可变引用。`TempIndexValueExt::FilterOverwritten` 消耗原列表并原地标空后压缩，调用后旧列表不可再用。

## 与 Go 版本的对应关系

同目录 [`tablecodec.go`](tablecodec.go) 是直接对照实现。Rust 基本保持相同的常量、公开函数分组和三套索引 value 布局：Go 的 `GenIndexValuePortal`、`GenIndexValueForClusteredIndexVersion1`、`genIndexValueVersion0` 分别对应同名 Rust 函数；行 key、meta key、临时索引和 range 验证也逐项对应。

语言层差异主要是 Rust 用 `Result`、`Option`、拥有所有权的 `Vec` 与 `Box<dyn Handle>` 表达 Go 的多返回值、nil slice/map/interface；Go 的 `TempIndexValue` 方法在 Rust 中由 `TempIndexValueExt` trait 承载。Rust `lib.rs` 还用依赖再导出和 `include!` 组合 `rowcodec`，而 Go 直接导入包。

已核对的兼容细节包括：API V2 `x001` keyspace 前缀后的 table ID 解码；唯一全局索引含 NULL 时变为 non-distinct 并在 key 中写 partition ID；global V1 key/value 双重 partition 信息不得生成嵌套 `PartitionHandle`；无 handle 的新格式唯一索引 value 返回 `None`，对应 Go 的 `nil, nil`；临时索引四类 value 元素及 overwritten 过滤语义。Rust 独立测试 [`tablecodec_test.rs`](tablecodec_test.rs) 与 Go [`tablecodec_test.go`](tablecodec_test.go) 的同名测试覆盖上述主干，另有 [`tablecodec_1_aster_unit_test.rs`](tablecodec_1_aster_unit_test.rs) 验证 canonical model/kv 类型与额外边界。

## 扩展指南

新增 key/value 格式时，应先确定它是否改变持久化排序或只增加可选 value 段。前者必须同步修改生成、切割、识别、range 边界和所有 handle 解码路径；后者应分配不冲突的 flag/version，并同步 `GenIndexValuePortal`、`getIndexVersion`、`SplitIndexValue`、`DecodeIndexKV*`、`IndexKVIsUnique` 与 untouched 判定。任何格式都要保持旧数据可读，不能只让新编码自洽。

新增 SQL 类型存储转换应成对更新 `flatten`/`Unflatten`，并在独立测试文件覆盖时区、零值、unsigned、collation、FSP 和错误传播。新增索引前缀规则应同时检查 `TruncateIndexValue` 的字节/字符边界以及 restored-data 路径。扩展 global index 时必须分别验证 distinct/non-distinct、int/common/partition handle、key/value 两个 partition 来源及 EXCHANGE PARTITION 后的重复 handle 场景。

测试必须继续放在独立文件，不应内嵌到 `tablecodec.rs`。优先扩展 [`tablecodec_test.rs`](tablecodec_test.rs) 并对照 [`tablecodec_test.go`](tablecodec_test.go)；canonical 类型或迁移特有契约可扩展 [`tablecodec_1_aster_unit_test.rs`](tablecodec_1_aster_unit_test.rs)。涉及 failpoint 的目标测试需遵循仓库 failpoint 启停流程。格式变更还应审查 `pkg/session/runtime/row_codec.rs`、`pkg/lightning/backend/kv/base.rs` 与 `pkg/store/driver/txn/error.rs` 等直接消费者。

## 验证依据

- RustCodeGraph：索引状态覆盖 `pkg/tablecodec/tablecodec.rs`（145 个符号）；读取了文件符号图，并精确查询 `EncodeRow`、`DecodeRowKey`、`EncodeIndexSeekKey`、`GenIndexKey`、`GenIndexValuePortal`、`DecodeIndexKVWithCollate`、`DecodeTempIndexValue`、`VerifyTableIDForRanges` 的源码与调用 trail。
- 源与装配：阅读 [`tablecodec.rs`](tablecodec.rs)、[`lib.rs`](lib.rs) 和 [`Cargo.toml`](Cargo.toml)，确认常量、类型、函数、trait、依赖、feature 与测试模块接线；本包不存在 `doc.go`。
- Go 对照：阅读 [`tablecodec.go`](tablecodec.go) 的完整符号清单及关键布局对应函数，并抽查 [`tablecodec_test.go`](tablecodec_test.go) 中临时索引、API V2、global partition handle、唯一全局索引 NULL 行为。
- Rust 测试：阅读 [`tablecodec_test.rs`](tablecodec_test.rs) 的行/索引/meta/range/错误/临时索引/global index 测试，以及 [`tablecodec_1_aster_unit_test.rs`](tablecodec_1_aster_unit_test.rs) 的整数/common handle、canonical 类型和跨表 range 边界测试。
- 生产调用：通过仓库搜索核对 `pkg/session/runtime/row_codec.rs`、`pkg/session/runtime/relational_scan.rs`、`pkg/lightning/backend/kv/base.rs`、`pkg/store/driver/txn/error.rs`、`pkg/store/helper/helper.rs` 等直接调用点。结构验收命令见任务交付记录；本任务按计划为纯文档分析，未运行 Cargo。
