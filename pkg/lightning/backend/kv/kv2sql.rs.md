# `pkg/lightning/backend/kv/kv2sql.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-backend-kv`（`pkg/lightning/backend/kv/Cargo.toml`），由 `lib.rs` 的 `mod kv2sql` 纳入并通过 `pub use kv2sql::*` 导出。它位于 Lightning KV 编码后端的反向路径：把 TiDB/TiKV 的记录键、索引键和行值恢复为 SQL 行视图，并从恢复后的行重新生成应存在的索引键。

当前 Rust 生产接线见 `pkg/dxf/importinto/conflict_resolution.rs`：`NewImporterConflictCodecWithOptions` 构造 `TableKVDecoder`，`ImporterConflictCodec::DecodeRow` 再调用 `DecodeRawRowData` 恢复冲突记录。其余公开方法目前主要由同 crate 的独立测试覆盖；不能据此推断所有 Go 调用场景均已迁移。

## 核心职责

- 用 `DecodeHandleFromRowKey` 和 `DecodeHandleFromIndex` 从 canonical `tablecodec` 键值格式恢复整数 Handle 或 Common Handle。
- 用 `DecodeRawRowData` 解码物理行值、回填缺省列，并从键中的 Handle 恢复未写入行值的主键列。
- 用 `IterRawIndexKeys` 重算生成列，按表的索引定义重建索引键，供冲突校验或清理逻辑逐个消费。
- 用 `DecodeRawRowDataAsStr`、`datumsToString` 和 `appendDatumText` 生成适合诊断的行文本；解码失败时保留 SQL 注释形错误文本。
- 用 `NewTableKVDecoder` 把表定义、会话选项、表名和预先收集的生成列绑定为可复用解码器。

## 主要符号

- `Handle`：公开行标识枚举。`Int(i64)` 对应整数主键或隐式 RowID；`Common(Vec<DatumKey>)` 对应聚簇复合主键。
- `DatumKey`：公开的 Common Handle 分量，只接受 `Int`、`UInt`、`String`、`Bytes`。这是比通用 `encode::Datum` 更窄的键域。
- `Handle::canonical`：内部适配器。整数直接包装为 `tablecodec::kv::IntHandle`；Common Handle 先把各分量转为 canonical datum，再用 `NewEncoder(false).EncodeKey(UTC, ...)` 编码并交给 `NewCommonHandle` 校验。
- `fromCanonicalHandle`：内部反向适配器。整数 Handle 直接取值；Common Handle 调用 `Handle::Data` 并逐列转换，不在 `DatumKey` 支持集中的 datum 会报错。
- `TableKVDecoder`：公开解码器，持有 `TableDefinition`、`Session`、`tableName` 和已排序的 `Vec<GeneratedCol>`。字段本身不公开，使用者通过构造函数和方法访问。
- `Name` / `Session`：分别返回表的展示名和只读会话引用；`Session` 使调用者能检查表达式上下文。
- `DecodeHandleFromRowKey`：调用 `tablecodec::DecodeRowKey` 后转成本模块 `Handle`。
- `DecodeHandleFromIndex`：先按 `indexID` 查找索引列数，再调用 `tablecodec::DecodeIndexHandle(key, value, column_count)`；唯一索引的 Handle 可来自 value，普通索引的 Handle 可编码在 key 中。
- `DecodeRawRowData`：返回 `(Vec<Datum>, BTreeMap<i64, Datum>)`。前者是按表列顺序整理后的完整行，后者是 canonical 行解码器实际读到的、以从 1 开始的列 ID 为键的原始列映射。
- `DecodeRawRowDataAsStr`：成功时格式化完整行，失败时返回 `/* ERROR: ... */`，本方法不把错误继续向上传播。
- `IterRawIndexKeys`：对恢复后的行求生成列并逐索引生成键，通过 `FnMut(&[u8]) -> Result<(), String>` 回调输出。
- `datumsToString` / `appendDatumText`：内部诊断格式化函数。多列使用括号和逗号，字符串加双引号；单个值最多输出 2048 字节并追加原始字节长度。
- `NewTableKVDecoder`：公开构造函数；调用 `NewSession(options)`，再用 `CollectGeneratedColumnsFromTable` 收集并排序生成列。

本文件没有 trait、模块级可见常量或条件编译项；唯一常量 `LOG_DATUM_LEN` 位于 `appendDatumText` 函数内部。

## 执行流程

1. 调用者把 `TableDefinition`、唯一表名和 `SessionOptions` 交给 `NewTableKVDecoder`。构造过程先创建 Lightning 轻量 `Session`，再缓存表定义中的生成列。
2. 处理记录键时，`DecodeHandleFromRowKey` 交由 canonical `tablecodec` 判定整数或 Common Handle，`fromCanonicalHandle` 再转成本地表示。
3. 处理索引键时，`DecodeHandleFromIndex` 必须先从 `tbl.indices` 找到索引并取得列数；该数量决定 `tablecodec` 从 key/value 的何处解析 Handle。
4. `DecodeRawRowData` 先用 `decodeCanonicalRow` 得到按列排列且可含 `Null` 的行，再用 `DecodeRowToDatumMap` 得到实际编码列映射。只有映射中不存在的列才从 `tbl.defaults` 回填。
5. 若 `pk_is_handle`，整数 Handle 覆盖主键列；无主键列或传入 Common Handle 均报错。若 `common_handle`，Handle 分量数必须与所有 `primary_key` 列数一致，然后按这些列在表中的顺序回填。
6. `IterRawIndexKeys` 调用上述行恢复流程；存在生成列时按 `genCols` 顺序写回计算结果。随后跳过 Common Handle 表的聚簇主键索引，对其余索引按列下标抽取值，转成 canonical datum，并调用 `tablecodec::GenIndexKey`。
7. 每个生成的键立即借用给回调；回调返回错误时迭代立刻终止并原样返回该错误。

## 数据与状态

`TableKVDecoder` 拥有表定义和会话，因此构造后使用固定的列、索引、默认值、Handle 形态和会话语义。`tableName` 只用于标识，不参与键编码；真正写入键的表 ID 来自 `tbl.id`。

行向量使用零基列下标，而 `DecodeRowToDatumMap` 的列 ID 由本文件按 `index + 1` 建立，因此 `decoded` 映射是从 1 开始。`defaults` 和 `IndexDefinition::columns` 仍是零基下标；扩展时必须避免混用两套坐标。

生成列列表在构造时从 `TableDefinition::generated` 复制并按目标下标排序。`evalGeneratedColumns` 原地改写本次解码得到的局部 `row`，不会改变解码器内的表定义。Handle 的 canonical 编码固定使用 UTC 和 `NewEncoder(false)`。

## 依赖与调用关系

上游：

- `pkg/dxf/importinto/conflict_resolution.rs::NewImporterConflictCodecWithOptions` 调用 `NewTableKVDecoder`；`ImporterConflictCodec::DecodeRow` 把外部 Handle 转成本模块 `Handle` 后调用 `DecodeRawRowData`，再投影可见列。
- `pkg/lightning/backend/kv/sql2kv_test.rs` 从 `TableKVEncoder` 生成的 KV 调用本文件 API，验证正反向闭环。
- `pkg/lightning/backend/kv/kv2sql_test.rs` 直接覆盖索引键枚举、Handle 解码、默认值、主键回填、错误文本及越界索引列。

下游：

- `encode` crate 提供 `Datum`、`ColumnType`、`SessionOptions` 和列定义。
- 同 crate 的 `canonical.rs` 提供 datum/字段/表/索引元数据适配以及 `decodeCanonicalRow`；`base.rs` 提供 `TableDefinition`、生成列收集与求值；`session.rs` 提供轻量会话。
- `tablecodec` crate 提供行键/索引 Handle 解码、Common Handle 编码、行 map 解码及索引键生成，是持久格式兼容边界。

RustCodeGraph 对 `NewTableKVDecoder` 给出的 Rust 下游边包括 `NewSession` 和 `TableKVDecoder` 构造；对 `IterRawIndexKeys` 给出的本文件内边包括 `DecodeRawRowData` 与 `Handle::canonical`。图查询未完整解析所有 canonical 自由函数，因此这些调用以源码为准。

## 错误处理与边界

公开的可失败操作统一使用 `Result<_, String>`，把 `tablecodec` 错误转为文本。主要显式边界包括：索引 ID 不存在、索引值不含 Handle、Common Handle 含不支持的 datum、默认列下标越界、Handle 形态与表定义不符、表缺少整数主键列、Common Handle 分量数不匹配、索引列下标越界、生成列求值失败、canonical datum 转换或索引键生成失败。

`DecodeRawRowDataAsStr` 是刻意的容错诊断接口：它吞掉结构化错误并把内容放进注释字符串，不能用于需要区分成功/失败的控制流。`appendDatumText` 的 2048 限制按 UTF-8 字节计算，并向前退到字符边界；`Bytes` 等二进制值通过 `from_utf8_lossy` 展示，所以文本不是原始字节的无损表示。`value as u64` 用于无符号整数主键回填，负的整数 Handle 会按 Rust 转换规则变成对应的大 `u64`；调用者应保证表键与无符号主键语义一致。

当前 `DatumKey` 不支持浮点、十进制、时间、JSON、枚举、集合等 Common Handle 分量，遇到这些 canonical datum 会明确报错。`IterRawIndexKeys` 每个 `IndexDefinition` 调用一次单键 `GenIndexKey`，没有实现 Go `GenIndexKVIter` 的一对多迭代语义；对可能产生多个索引条目的索引类型不能假定与 Go 等价。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务提交。解码器持有一个 `Session`，但这里仅通过只读 `Session()` 暴露；每次解码和索引重建都使用局部 `Vec`、map 和 canonical 元数据对象。

`IterRawIndexKeys` 传给回调的 `&[u8]` 借用自当前循环的局部 `Vec<u8>`，只在回调期间有效；需要跨回调保存键时必须像测试一样复制为 `to_vec()`。回调按索引定义的顺序同步执行，任一错误会停止后续索引处理。`TableKVDecoder` 没有声明额外的并发保证；是否可在线程间共享取决于其字段类型的自动 trait，API 本身不提供内部同步。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/backend/kv/kv2sql.go`，测试对照是 `kv2sql_test.go`。

- Rust `TableKVDecoder` 对应 Go 同名类型；`TableDefinition` 是对 Go `table.Table` 所需信息的简化持有形式，Rust `Session` 对应 Go `*Session`。
- Handle 解码、原始行恢复、错误注释文本、跳过 Common Handle 聚簇主键索引、生成列后重建索引等主流程保持一致。
- Go 构造器会按 `tbl.UseNewCollate()` 设置新排序规则，并用可能失败的 `CollectGeneratedColumns`；Rust 构造器调用 `CollectGeneratedColumnsFromTable`，没有对应的排序规则开关，收集过程本身不返回错误。
- Go 生成列求值前会把生成列槽位设为该类型最小值，并使用完整表达式上下文；Rust 直接使用简化的 `GeneratedExpression::{Copy, Add, Constant}` 依次覆写目标列。
- Go 使用 `index.FetchValues` 与 `GenIndexKVIter`，允许一个索引产生多个键并复用缓冲；Rust 按列下标直接取值并用 `GenIndexKey` 每个索引只产生一个键。
- Go `types.DatumsToString(row, true)` 支持完整 TiDB datum 格式；Rust 是本地格式化器，覆盖当前 `encode::Datum` 变体，但二进制展示为有损 UTF-8，且自行执行 2048 字节截断。
- Rust 额外显式返回 `BTreeMap`，保证列 ID 的迭代顺序；Go 返回普通 `map[int64]types.Datum`，无顺序保证。

因此，本文件是有真实生产接线的移植实现，但并非 Go 完整表/表达式/索引能力的逐项等价替代。扩展支持时应以 Go 行为和 canonical `tablecodec` 格式为兼容基准。

## 扩展指南

- 新增 Handle datum 类型时，同时修改 `DatumKey`、`Handle::canonical`、`fromCanonicalHandle`，并在独立的 `kv2sql_test.rs` 增加编码—解码往返及拒绝非法类型测试。
- 调整行恢复时优先修改 `DecodeRawRowData`，分别覆盖缺省值、整数/无符号主键、Common Handle 分量数和列 ID 映射；还应在 `sql2kv_test.rs` 保留由真实编码器产生 KV 的闭环测试。
- 扩充生成列表达式时，能力实际归属 `base.rs::GeneratedExpression` 与 `evalGeneratedColumns`；本文件只负责在索引生成前调用。测试必须独立放在 `*_test.rs`，不要内嵌到生产文件。
- 支持多值索引或其他一索引多键语义时，应把 `IterRawIndexKeys` 从单次 `GenIndexKey` 扩展为与 Go `GenIndexKVIter` 等价的迭代，并验证回调顺序、中途错误和唯一/非唯一索引的 Handle 存放位置。
- 改动 `TableDefinition` 的列 ID 表示时，必须同步审查 `field_types` 的 `index + 1`、默认值零基下标和索引列零基下标，避免静默错列。
- 改诊断输出需同步 `decode_raw_row_data_as_str_*` 测试，并评估日志兼容、敏感值暴露、二进制可逆性和超长 UTF-8 截断行为。
- 性能热点是每个索引重复构造 canonical `TableInfo`、转换 datum 和分配键；若缓存元数据或复用缓冲，必须保证回调借用生命周期不变，并用等价键结果验证优化。

## 验证依据

- 生产源码：`pkg/lightning/backend/kv/kv2sql.rs`；相邻模块入口与 crate 声明：`pkg/lightning/backend/kv/lib.rs`、`pkg/lightning/backend/kv/Cargo.toml`。
- 直接依赖实现：`pkg/lightning/backend/kv/base.rs`、`canonical.rs`、`session.rs`。
- Rust 独立测试：`pkg/lightning/backend/kv/kv2sql_test.rs`；额外闭环测试：`pkg/lightning/backend/kv/sql2kv_test.rs`。
- Go 对照：`pkg/lightning/backend/kv/kv2sql.go`、`pkg/lightning/backend/kv/kv2sql_test.go`。
- 生产调用证据：`pkg/dxf/importinto/conflict_resolution.rs::NewImporterConflictCodecWithOptions` 与 `ImporterConflictCodec::DecodeRow`。
- RustCodeGraph：`status` 显示索引包含目标 Rust/Go 文件；对 `TableKVDecoder`、`NewTableKVDecoder`、`DecodeRawRowData`、`IterRawIndexKeys`、两个 Handle 解码方法及格式化函数执行了 `query`，并对构造、行解码和索引键迭代执行了 `callers`/`callees`。由于图对部分自由函数调用存在漏边，调用关系同时以源码和 `rg` 交叉核对。
- 结构验收以任务指定命令验证本文件存在且恰好包含十一个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
