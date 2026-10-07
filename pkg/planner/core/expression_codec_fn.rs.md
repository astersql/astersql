# `pkg/planner/core/expression_codec_fn.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。`pkg/planner/core/lib.rs` 以私有模块 `mod expression_codec_fn` 装载它，再通过 `pub use expression_codec_fn::*` 导出其中的公开类型和方法；测试则由同一 `lib.rs` 将独立文件 `expression_codec_fn_test.rs` 与 `panicrisk_regression_test.rs` 接入。crate 边界由 `pkg/planner/core/Cargo.toml` 定义，目标文件自身只直接使用标准库的 `BTreeMap`，没有条件编译项、异步入口或外部依赖。

它提供一套规划器侧的轻量键编解码模型：用本地 `CodecTable`、`CodecIndex`、`CodecColumn`、`CodecRow` 和 `Datum` 描述元数据与行，再生成或解析形如 `t{table_id}_r{handle}`、`t{table_id}_i{index_id}_{values}` 的**文本化键**。这与 Go `pkg/planner/core/expression_codec_fn.go` 服务于 `tidb_decode_key`、`tidb_encode_record_key`、`tidb_encode_index_key` 的职责相似，但当前 Rust 实现并不是 TiDB 真实二进制 tablecodec 的等价替代，也没有接入 Rust 表达式层的 `KeyCodec` trait。

## 核心职责

1. `Datum::key_string`、`handle_key_string` 将本地值与 handle 规范化为文本片段。
2. `TiDBCodecFuncHelper::buildHandle` 根据表的 `common_handle` 标志，在整数 handle 与多列 common handle 之间选择，并校验必需列。
3. `encodeHandleFromRow` 与 `encodeIndexKeyFromRow` 根据本地元数据和行构造可读记录键、索引键；非唯一索引把 handle 追加到索引值之后。
4. `findCommonOrPartitionedTable`、`extractTablePartition` 解析大小写不敏感的表名和 `table(partition)` 语法。
5. `decodeKeyFromString` 接受十六进制字符串，恢复 UTF-8 文本键，再分派到记录、索引或表前缀解码，并输出字段顺序稳定的 JSON 字符串。
6. `datumToJSONObject` 生成用于展示/调试的 JSON 字面量片段。

这些职责的边界很重要：源文件没有数据库名、infoschema、权限检查、SQL 表达式求值、时区、告警上下文或真实 KV handle/tablecodec 类型。它当前更接近独立、可测试的兼容模型，而非完整应用主链中的生产 codec。

## 主要符号

- `Datum`：本地 SQL 值联合，含 `Null`、有/无符号整数、浮点、字节串、字符串、布尔和 JSON 文本。`key_string(&self) -> String` 把值转成键片段；字节串转小写十六进制，`Null` 转大写 `NULL`，`String`/`Json` 原样取内部文本。
- `CodecColumn`：列 ID、名称、整数主键标志和 `unsigned` 标志。当前 `unsigned` 字段不参与任何分支，只有元数据形状意义。
- `CodecIndex`：索引 ID、名称、按顺序排列的列 ID 和唯一性标志。
- `CodecTable`：逻辑表 ID/名称、列/索引集合、common-handle 标志，以及 `BTreeMap<partition_id, partition_name>` 分区映射。
- `Handle`：`Int(i64)` 或 `Common(Vec<Datum>)`。common handle 的列顺序由名为 `PRIMARY` 的索引的 `column_ids` 决定。
- `CodecRow`：从列 ID 到 `Datum` 的 `BTreeMap`。
- `TiDBCodecFuncHelper`：无字段、可克隆的零大小帮助器，承载全部公开操作。
- 私有函数 `handle_key_string`：整数直接十进制输出，common handle 用 `|` 拼接各 Datum。
- 私有函数 `json_string`：为引号、反斜线、常见控制字符及 U+0000..U+001F 做 JSON 转义。
- 私有函数 `decode_hex`：要求偶数长度且每个半字节均为十六进制字符，否则返回 `invalid hex key`。
- 公开方法：`encodeHandleFromRow`、`findCommonOrPartitionedTable`、`extractTablePartition`、`buildHandle`、`encodeIndexKeyFromRow`、`decodeKeyFromString`、`datumToJSONObject`。
- 私有方法：`decodeRecordKey`、`decodeIndexKey`、`decodeTableKey`。

公开 API 保留了 Go 风格的 camelCase 命名；没有 trait、模块级常量或静态可变状态。

## 执行流程

记录键编码从 `encodeHandleFromRow` 开始：先调用 `buildHandle`；整数表寻找首个 `primary_key` 列并接受 `Datum::Int` 或 `Datum::UInt`，common-handle 表寻找名称大小写不敏感等于 `PRIMARY` 的索引并按 `column_ids` 取值；随后 `handle_key_string` 串行化，最终输出 `t{table.id}_r{body}` 的字节。

索引键编码由 `encodeIndexKeyFromRow` 按 `index.column_ids` 取值并调用 `Datum::key_string`。无论索引是否唯一，它都会先调用 `buildHandle`，因此缺少或非法主键始终报错；只有非唯一索引才把 `|{handle}` 加入键体。最终输出 `t{table.id}_i{index.id}_{body}`。这条“唯一索引也必须先成功构造 handle”的分支由 `expression_codec_fn_test.rs::non_unique_index_key_requires_and_contains_the_handle` 附近测试及源码注释锁定。

解码从 `decodeKeyFromString` 开始：`decode_hex` 将输入转字节，`from_utf8` 要求内容是 UTF-8 文本，首字符必须为 `t`。含 `_r` 的键进入 `decodeRecordKey`，含 `_i` 的键进入 `decodeIndexKey`，否则把 `t` 后内容解析为表 ID 并进入 `decodeTableKey`。记录解码按元数据决定整数/common/未知表三种输出；索引解码在已知表时按索引列名构造对象，在未知表时把 `|` 分隔值合成字符串。各 JSON 字段或索引字段在输出前排序，确保确定性。

表/分区查找先由 `extractTablePartition` 取第一个 `(` 与第一个 `)`。没有完整、正序括号时返回原串和 `None`；空括号产生空分区名，但 `findCommonOrPartitionedTable` 会过滤为空并按普通表处理。之后表名和分区名均用 ASCII 大小写不敏感比较。

## 数据与状态

所有业务数据都由调用者以拥有值或不可变借用传入；帮助器本身无字段。`CodecTable` 和 `CodecRow` 使用 `BTreeMap`，分区查找与行值查找不会修改映射。编码过程创建临时 `String`/`Vec`，解码过程创建字段列表并排序，没有缓存、全局注册或跨调用状态。

几个数据不变量直接决定结果：common handle 必须存在名为 `PRIMARY` 的索引，且该索引的每个列 ID 都必须在行中；整数 handle 必须存在标记 `primary_key` 的列且值是整数；索引的所有 `column_ids` 必须在行中；已知表的索引解码要求索引 ID 存在且键中值片段不少于索引列数。额外的索引片段（例如非唯一索引追加的 handle）会被保留在拆分结果中，但已知表输出只消费与索引列数量相同的前缀。

`Datum::Float` 使用 Rust 默认浮点字符串表示，`String`、`Json` 和 `|` 分隔没有转义层，因此值中含 `|` 时文本键不能无歧义往返；这进一步说明该格式是轻量模型，不是持久化/线上 TiKV key 格式。

## 依赖与调用关系

RustCodeGraph 的 `callees` 证据显示：

- Rust `encodeHandleFromRow` 调用 `buildHandle` 和 `handle_key_string`。
- Rust `encodeIndexKeyFromRow` 调用 `buildHandle`、`Datum::key_string`、`handle_key_string` 及拼接逻辑。
- Rust `decodeKeyFromString` 调用 `decode_hex`，并分派到 `decodeRecordKey`、`decodeIndexKey`、`decodeTableKey`。

RustCodeGraph/仓库搜索只发现独立 Rust 测试直接调用这些公开 API；没有发现生产 Rust 文件构造 `TiDBCodecFuncHelper` 或使用 `CodecTable`。`pkg/planner/core/core_init.rs` 只把 `DecodeKeyFromString`、`EncodeRecordKeyFromRow`、`EncodeIndexKeyFromRow` 作为字符串名称登记到回调注册表，并未安装函数对象。表达式侧真实 Rust 扩展点位于 `pkg/expression/builtin_info.rs::KeyCodec`，其 `encode_record_key`、`encode_index_key`、`decode_key` 接口目前未由本文件实现。

Go 主链则不同：`pkg/planner/core/core_init.go` 把 helper 的三个方法赋给 `expression.DecodeKeyFromString`、`EncodeRecordKeyFromRow`、`EncodeIndexKeyFromRow`；`pkg/expression/builtin_info.go` 和向量化路径调用这些槽位，SQL 集成测试位于 `pkg/expression/integration_test/integration_test.go`。因此不能从 Go 的生产接线反推 Rust 已接线。

## 错误处理与边界

本文件统一使用 `Result<_, String>`，没有结构化错误或 warning sink。编码错误包括缺少主索引/主键列、行缺列、索引列缺失或 handle 类型错误；表查找错误区分表不存在、非分区表和分区不存在。`extractTablePartition` 对 `)` 出现在 `(` 之前、缺任一括号等畸形输入不切片，避免 panic；这一历史边界由 `panicrisk_regression_test.rs::test_extract_table_partition_malformed` 验证。

解码先拒绝奇数长度/非法十六进制、非 UTF-8、错误前缀及非法表/索引 ID。common handle 解码还检查主索引存在、值数量精确相等、列 ID 能映射到列；已知索引检查索引存在、值数量下限及列存在。整数记录 handle 必须能解析为 `i64`。

无符号整数超过 `i64::MAX` 时，`buildHandle` 通过 `i64::from_ne_bytes(value.to_ne_bytes())` 保留同一位模式，效果是二补码式回绕为负数；它不读取 `CodecColumn::unsigned`。JSON 输出由手写 `json_string` 保护字符串，但 `datumToJSONObject` 对浮点、整数和布尔直接返回 `key_string`，没有检查非有限浮点是否是合法 JSON。调用者不应把该函数当通用 JSON 序列化器。

## 并发与资源生命周期

`TiDBCodecFuncHelper` 是无状态零大小类型，所有方法只读借用输入或创建局部拥有值；本文件没有锁、原子变量、线程、任务、通道、事务、文件句柄或网络资源。只要调用者安全共享其 `CodecTable`/`CodecRow`，帮助器调用之间没有内部竞争或顺序依赖。

主要资源成本来自临时分配：每个 Datum 的字符串化、`Vec` 收集、`join`、JSON 字段排序和 `format!` 都会分配。编码/解码均按列数或字符串长度线性工作；表、索引和列查找使用切片线性扫描。当前适合辅助/调试规模，若进入高频执行路径，应先评估分配量和元数据查找成本。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/planner/core/expression_codec_fn.go`，方法名称与职责一一可辨，但参数、接线和编码语义差异显著：

- Go 编码入口从 SQL `Expression` 求值数据库名、表名、索引名和列值，经 infoschema 找表并执行权限校验；Rust 直接接收本地元数据和行，没有数据库名或权限模型。
- Go 使用 `tablecodec.EncodeRecordKey`、`tables.NewIndex().GenIndexKey`、`codec.EncodeKey` 和 `kv.Handle` 生成真实 TiDB 二进制键；Rust 生成带分隔符的 UTF-8 文本键。
- Go common handle 会做类型转换、索引值截断、时区相关编码和错误上下文处理；Rust 仅按列 ID 克隆 `Datum`。
- Go 解码允许外层 mem-comparable bytes，使用 tablecodec 判断键类，并在失败时追加 warning 后返回原字符串；Rust 只解普通十六进制 UTF-8 文本，失败直接返回 `Err(String)`。
- Go 从 infoschema 恢复真实列类型和分区表，`datumToJSONObject` 通过 `types.Datum::ToString` 返回值；Rust 依靠调用者提供的简化元数据，并手写 JSON 片段。
- 两边都保留整数记录路径把 `table_id` 输出为 JSON 字符串、common/索引/表路径输出数值的现有形状；Rust 测试显式锁定了这一不对称。
- `extractTablePartition` 的畸形括号保护与 Go 回归测试语义一致。

所以迁移状态应描述为“局部行为模型和测试已存在，但生产 codec、权限/元数据接线及二进制兼容尚未完成”，不能描述为 Go 功能已完整移植。

## 扩展指南

若只扩展轻量模型，应在本文件相应符号修改，并同步独立测试 `pkg/planner/core/expression_codec_fn_test.rs`；括号解析的安全性还应同步 `pkg/planner/core/panicrisk_regression_test.rs`。新增 Datum 种类需同时审查 `Datum::key_string`、`datumToJSONObject`、handle/索引编码和 JSON 合法性。改变分隔格式必须覆盖值内含 `|`、空值、额外 handle 片段和未知表回退。

若目标是接入 Rust SQL 内建函数，不应仅扩写当前字符串格式：应围绕 `pkg/expression/builtin_info.rs::KeyCodec` 设计适配器，明确 infoschema/权限/会话类型上下文所有权，并复用或移植真实 tablecodec/codec 类型。接线后需补表达式层独立测试与 SQL 集成测试，覆盖 NULL、权限拒绝、分区物理 ID、整数/common handle、唯一/非唯一索引、类型转换、前缀截断、时区、非法键 warning/原值回退，以及 Go 集成测试中的真实十六进制向量。

任何兼容性改动都要先决定目标是“维持当前文本模型”还是“与 Go/TiKV 二进制格式兼容”；两者不能无版本标记地混用。性能优化应优先减少多次 `format!`/`join` 分配及重复线性元数据查找，同时保持确定性字段顺序。Rust 单元测试继续放在独立测试文件，不嵌回生产源文件。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/planner/core/expression_codec_fn.rs --offset 1 --limit 500`，确认文件共 459 行及所有类型、函数、方法和分支。
- 符号与调用边：RustCodeGraph `query TiDBCodecFuncHelper`、`query decodeKeyFromString`，以及对 `decodeKeyFromString`、`encodeHandleFromRow`、`encodeIndexKeyFromRow`、`buildHandle` 的 `callers`/`callees` 查询。
- 模块/crate：`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/doc.go`。
- Rust 接线边界：`pkg/planner/core/core_init.rs` 与 `pkg/expression/builtin_info.rs::KeyCodec`。
- Go 对照与生产接线：`pkg/planner/core/expression_codec_fn.go`、`pkg/planner/core/core_init.go`、`pkg/expression/builtin_info.go`。
- 独立 Rust 测试：`pkg/planner/core/expression_codec_fn_test.rs` 覆盖 JSON 转义、记录/索引/表/分区解码、handle 约束和确定性输出；`pkg/planner/core/panicrisk_regression_test.rs` 覆盖畸形括号不 panic。
- Go 测试：`pkg/planner/core/panicrisk_regression_test.go` 与 `pkg/expression/integration_test/integration_test.go::{TestTiDBDecodeKeyFunc, TestTiDBEncodeKey}`，用于核对真实二进制 key、权限、分区和错误行为。
- 仓库级 `rg` 搜索确认 Rust 生产侧没有 `TiDBCodecFuncHelper`/`CodecTable` 调用者；这一结论受当前索引与工作树版本限制，后续接线新增时应重新查询。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求本文恰好包含上述十一个固定二级标题。
