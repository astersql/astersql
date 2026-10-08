# `pkg/util/rowcodec/decoder.rs`

## 文件定位

`decoder.rs` 是 `astersql-util-rowcodec` crate 的新格式行值解码实现。它不是独立 Rust 模块，而是由 [`lib.rs`](lib.rs) 的 `rowcodec_impl` 通过 `include!("decoder.rs")` 与 [`common.rs`](common.rs)、[`row.rs`](row.rs)、[`encoder.rs`](encoder.rs) 合并，再由 crate 根和 `rowcodec` 命名空间公开。因此，本文件可直接使用同一实现模块中的 `row`、`decodeInt`、`decodeUint` 和旧 Datum flag。

它位于持久化 rowcodec 字节与三种上层表示之间：`DatumMapDecoder` 生成按列 ID 索引的 `Datum` map，`ChunkDecoder` 向列式 `chunk::Chunk` 追加一行，`BytesDecoder` 则把新格式列片段恢复为旧式 `flag + payload` Datum 字节。Rust 当前可确认的直接生产调用位于 `pkg/tablecodec/tablecodec.rs`：`DecodeRowWithMapNew` 使用 map 路径，`decodeRestoredValues` 与 `decodeRestoredValuesV5` 使用 bytes 路径；`ChunkDecoder` 在本 crate 的独立测试与基准中有接线，但搜索未发现其他 Rust 生产调用者，不能据 Go 调用面推断 Rust 已全部接线。

## 核心职责

1. 复用 `row::fromBytes` 解析新格式行头、列 ID、NULL 集合、offset、checksum 和数据区，并按请求列挑选数据。
2. 根据 `ColInfo::Ft` 将同一列片段转换为强类型 `Datum`、追加到 `Chunk`，或编码成旧 Datum 字节。
3. 维持缺列处理优先级：真实 row value 优先于 handle；显式 NULL 优先于默认值；仅真正缺列时才尝试 handle 和默认值。
4. 处理特殊列与兼容分支，包括 `ExtraCommitTSID`、`ExtraRowChecksumID`、虚拟生成列、整数/复合 handle、需要 restored data 的字段、decimal scale、timestamp 时区及空 enum 容错。
5. 暴露行 checksum 查询、`ColumnIsNull` 和新旧 Datum flag 映射等辅助能力。

该文件只负责“按已知列元信息解释字节”，不负责构造列元信息、计算生成列、校验 checksum、补齐 `DatumMapDecoder` 的缺列默认值，也不决定新旧行格式分流。

## 主要符号

- `decoder`：三个具体解码器共享的基础状态，持有可复用的 `row`、`columns: Vec<ColInfo>`、`handleColIDs` 和可空时区 `loc`。`fromBytes`、`getData` 是内部转发；`ColumnIsNull` 委托给 `row::ColumnIsNull`。
- `NewDecoder`：只组装基础解码器，不解析数据。主要用于 `ColumnIsNull` 等基础能力。
- `ColInfo`：每个请求列的最小元数据，包含列 ID、是否整数主键句柄、是否虚拟生成列和拥有所有权的 `FieldType`。与 Go 的 `*types.FieldType` 不同，Rust 字段是值类型。
- `DatumMapDecoder` / `NewDatumMapDecoder`：将新格式行转为 `HashMap<i64, Datum>`，构造时不设置 handle 列。`GetChecksum` 与 `ChecksumVersion` 读取最近一次成功解析到基础 `row` 的 checksum 元数据。
- `DecodeToDatumMap`：解析行后逐列调用 `findColID`；非 NULL 列交给 `decodeColDatum`，显式 NULL 写入 null Datum，缺列不插入 map。
- `decodeColDatum`：按 MySQL 类型生成 Datum，覆盖整数、浮点、字符串/二进制、decimal、时间、duration、enum、set、bit、JSON 和 vector float32。
- `ChunkDecoder` / `NewChunkDecoder`：除基础状态外持有可选 `defDatum` 回调；回调按列下标向同一个 chunk 写默认值。
- `DecodeToChunk`：chunk 主入口，按特殊提交时间列、虚拟列、checksum 列、真实 value、handle、NULL、默认值的顺序为每个请求列追加一个值。
- `tryAppendHandleColumn`：为整数或复合 handle 补缺列。复合 handle 通过 `codec::NewDecoder(...).DecodeOne` 直接追加到 chunk；需要 restored data 时拒绝使用 handle。
- `decodeColToChunk`：与 `decodeColDatum` 的类型矩阵对应，但直接调用 `Append*`，并在 decimal 的编码 scale 大于目标字段 scale 时以 `ModeHalfUp` 四舍五入。
- `BytesDecoder` / `NewByteDecoder`：持有返回旧 Datum 字节的可选 `defBytes` 回调。
- `decodeToBytesInternal`：`DecodeToBytesNoHandle` 与 `DecodeToBytes` 的共同实现；根据 `outputOffset` 排列结果，并依次处理真实 value、handle、NULL、默认值和最终 `NilFlag`。
- `tryDecodeHandle`：把整数主键编码为 `IntFlag/UintFlag + codec integer`，或直接采用复合 handle 的 `EncodedCol(i)` 旧 Datum 片段。
- `encodeOldDatum`：bytes 改用 `CompactBytesFlag`，整数改用 varint/varuint；其余类型保留 `fieldType2Flag` 选出的 flag 并拼接 rowcodec payload。
- `fieldType2Flag`：把 MySQL 字段类型映射到旧 Datum flag；未知类型直接 panic。

本文件没有 trait、模块级常量、宏或条件编译项；所用 flag 和特殊列 ID 分别来自同模块 `common.rs` 与 `lib.rs::model`。

## 执行流程

三条公开解码路径都先依赖 `row::fromBytes` 建立对当前输入的视图。`DatumMapDecoder::DecodeToDatumMap` 会复用调用方传入的 map，或按列数创建新 map；对每个 `ColInfo` 查找列 ID，找到非 NULL 值就按字段类型转换，显式 NULL 插入 null Datum，未出现的列则保留 map 原状。它不会读取 handle，也不会调用默认值回调。

`ChunkDecoder::DecodeToChunk` 每次先重置并解析内部 `row`，再按 `columns` 顺序向对应 chunk 列追加。`ExtraCommitTSID` 从参数 `commitTS` 填充；虚拟生成列与 `ExtraRowChecksumID` 先写 NULL，留给后续计算或处理。普通列若在 value 中存在则直接解码；只有 value 缺列时才调用 `tryAppendHandleColumn`，这是为了避免前缀聚簇索引中不完整的 handle 覆盖完整 row value。若仍未补齐，显式 NULL 写 NULL，真正缺列才调用默认值回调；无回调时也写 NULL。

handle 分两类。整数 handle 在列 ID 等于 `handleColIDs[0]` 时直接追加 `IntValue()`；复合 handle 遍历 ID 列表，找到对应序号后，若字段不需要 restored data，则用通用 codec decoder 解释 `EncodedCol(i)`。`DecodeOne` 失败在这里被转成 `false`，调用方随后按 NULL/默认值路径继续，而不是返回原错误。

`BytesDecoder` 不复用基础 `row`，而是在每次调用中创建局部 `row` 并解析 `value`。它按 `columns` 遍历、按 `outputOffset[colID]` 写二维字节数组。真实列经 `fieldType2Flag` 和 `encodeOldDatum` 恢复旧表示；缺列时可从 handle 生成旧字节；显式 NULL、空默认值或无默认值最终均为单字节 `NilFlag`。`DecodeToBytesNoHandle` 只是传入空 handle/cache 的便捷入口。

类型级转换中，整数与 duration 使用 `decodeInt/decodeUint`；float、decimal 交给基础 codec；字符串/二进制保留原始 bytes；时间从 packed uint 恢复；enum 解析失败刻意变为空 enum；set、vector 和时区转换错误向上传播；JSON 将首字节作为 type code、其余字节作为 value。

## 数据与状态

`decoder::row` 是可变、可复用的最近一次解析状态。`DatumMapDecoder` 与 `ChunkDecoder` 的公开解码方法需要 `&mut self`，成功调用后 `GetChecksum`/`ChecksumVersion` 反映最近一行。`BytesDecoder` 使用局部 `row`，其公开解码方法只需 `&self`，不会把最近一行保存在对象中。

`columns` 的顺序决定 chunk 列下标，也决定默认值回调收到的 `usize`；`BytesDecoder` 的输出顺序则完全由 `outputOffset` 决定。`outputOffset.len()` 决定结果数组长度，但源码没有验证 offset 是否越界、是否重复或是否覆盖所有 `columns`。缺失 key 使用 `0`，这是对 Go map 零值行为的显式复刻。

列数据由基础 `row` 以借用 slice 暴露，随后按目标类型复制或追加。Datum map 中字符串、JSON 等会拥有新数据；chunk 的具体所有权由各 `Append*` 实现承担；bytes 路径为每个结果创建 `Vec<u8>`。`cacheBytes` 在 Rust 中先 `to_vec()` 再追加，因此不会像 Go slice 那样复用调用方容量或与输入共享底层存储。

默认值闭包存放在解码器内：`defDatum` 可修改传入 chunk，`defBytes` 返回拥有所有权的旧 Datum 字节。两者均为 `Box<dyn Fn...>`，没有 `Send`/`Sync` 约束。

## 依赖与调用关系

[`Cargo.toml`](Cargo.toml) 将本文件归入 `astersql-util-rowcodec`，默认 feature 为空；`nextgen` 只透传给 kerneltype 和 kv 依赖，本文件没有 feature 条件分支。直接依赖由 [`lib.rs`](lib.rs) 再导出：`astersql-util-chunk` 提供列式输出，`astersql-util-codec` 提供基础 Datum 解码和错误，`astersql-kv` 提供 `Handle`，meta model 提供特殊列 ID，types 的 datum/metadata/scalar crate 提供 `Datum`、`FieldType`、时间、decimal、JSON、vector 和 restored-data 判断，`chrono-tz` 表示时区。

已核对的 Rust 生产调用边是：

- `pkg/tablecodec/tablecodec.rs::DecodeRowWithMapNew` → `NewDatumMapDecoder` → `DecodeToDatumMap`；
- `pkg/tablecodec/tablecodec.rs::decodeRestoredValues` → `NewByteDecoder` → `DecodeToBytesNoHandle`；
- `pkg/tablecodec/tablecodec.rs::decodeRestoredValuesV5` → `NewByteDecoder` → `DecodeToBytesNoHandle`；
- `pkg/store/driver/txn/error.rs::rowcodec_columns` 构造再导出的 `ColInfo`，供 tablecodec 相关错误恢复路径使用。

RustCodeGraph 的文件节点报告 `decoder.rs` 被 `pkg/tablecodec/tablecodec.rs`、`pkg/store/driver/txn/error.rs`、`encoder.rs`、`bench_test.rs` 和 `rowcodec_test.rs` 五个文件关联。精确 `query` 找到 Rust/Go 两套 `DecodeToDatumMap`、`DecodeToChunk`、`DecodeToBytes*` 和 `fieldType2Flag`；但对 include 内 Rust 方法执行 `callers/callees` 未在限定时间内返回结果，因此直接 Rust 调用边又以精确 `rg` 核对，没有把 Go 的 23 个使用文件当作 Rust 已接线证据。

内部下游链为：三个入口 → `row::fromBytes/findColID/getData`；类型分支 → `common.rs::decodeInt/decodeUint`、codec 的 float/decimal/integer/compact-bytes API、types 的解析/时区/round/vector API；handle 分支 → `kv::Handle` 与 `codec::Decoder::DecodeOne`；输出 → `Datum::Set*` 或 `Chunk::Append*`。

## 错误处理与边界

行格式、float/decimal/vector 解码、time 构造/时区转换、set 解析、decimal round 和默认值回调的错误通过 `SharedError` 返回，通常在首个失败列中止。未知字段类型在 Datum/chunk 路径返回 `unknown type` 错误，但 `fieldType2Flag` 在 bytes 路径会 panic；enum 是特例，解析错误被故意替换为空 enum。复合 handle 的 `DecodeOne` 错误也被降级为“未从 handle 补值”，可能继续写 NULL 或默认值。

源码依赖若干未运行时检查的前置条件：整数 handle 路径会访问 `handleColIDs[0]`，因此传入整数 handle 时该数组必须非空；JSON 路径直接读取 `colData[0]`，数据必须至少一字节；`decodeInt/decodeUint` 要求合法的 1/2/4/8 字节编码；`outputOffset` 的所有 offset 必须落在结果长度内；复合 handle 的 ID 序号必须与 `EncodedCol(i)` 对齐。违反这些条件可能 panic，而不是返回 `SharedError`。

`ExtraCommitTSID` 要求 `commitTS > 0`，源码调用 `intest::Assert`，随后仍以运行时分支在非正值时追加 NULL。虚拟列和 `ExtraRowChecksumID` 无条件追加 NULL。显式 NULL 永远不会触发默认值回调；缺列在 map 路径完全省略，在 chunk/bytes 路径才补默认值或 NULL。

时区语义有意区分两条路径：chunk 仅在 `loc.is_some()` 时转换非零 timestamp；Datum map 对非零 timestamp 使用 `loc.unwrap_or(UTC)`。这使 Rust 的 `None` 在 map 路径成为 UTC，而 Go `decoder.loc` 会直接传给 `ConvertTimeZone`；不能假设 nil 时的错误/崩溃细节完全一致。零时间不会转换。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或事务。所有解析和转换都在调用栈内同步完成；错误返回后，已经向 chunk 追加的前序列不会回滚，因此调用方不应把失败视为对输出的原子操作。

`DatumMapDecoder`/`ChunkDecoder` 因可变复用内部 `row`，同一个实例不能无同步地并发解码；`BytesDecoder` 的局部 row 本可支持只读共享，但其 trait-object 回调没有 `Send + Sync` 声明，类型层面不承诺跨线程共享。chunk 和 map 均由调用方拥有，解码器不延长其生命周期。

资源成本主要是按列扫描和目标分配：每个请求列通过 `findColID` 查找，具体复杂度由 `row.rs` 的 small/large ID 表决定；map/JSON/string/bytes 转换会分配，handle 的 codec decoder 是短生命周期局部对象。解码器可复用其基础 `row` 状态，但 Rust bytes 路径的局部 row、结果 vectors 和 `cacheBytes.to_vec()` 仍会产生分配。

## 与 Go 版本的对应关系

直接对照文件是 [`decoder.go`](decoder.go)，测试对照是 [`rowcodec_test.go`](rowcodec_test.go) 与 [`rowcodec_test.rs`](rowcodec_test.rs)。三类解码器、列元数据、分支顺序、类型矩阵、handle 仅在 value 缺列时使用、NULL/默认值优先级、decimal half-up、空 enum 容错、旧 Datum flag 映射和未知类型策略总体逐项对应。

已确认的 Rust 表达或行为差异包括：

- Go 解码器通过匿名嵌入 `row`/`decoder` 暴露方法；Rust 显式持有字段并写转发方法。Go 构造函数返回指针，Rust 返回拥有所有权的值。
- Go `ColInfo::Ft` 是指针且理论上可为 nil；Rust 是值类型，消除了该空指针状态，但构造时会发生 clone/移动。
- Go 的 nil map、nil handle、nil 回调用语言 nil 表示；Rust 分别使用 `Option<HashMap<...>>`、`Option<&dyn Handle>` 和 `Option<Box<dyn Fn...>>`。
- Go `SetString(string(colData), collate)` 以字节构造 string；Rust 使用 `SetBytesAsString(colData.to_vec(), ...)`，保留任意字节的能力取决于 Rust Datum 的具体实现，不能简单等同于 UTF-8 `String`。
- Datum map 的 nil 时区在 Rust 回退 UTC；chunk 的 nil 时区才保持“不转换”。Go map 路径不做 nil 检查，chunk 路径做 nil 检查。
- Go JSON 的 `Value` 借用 `colData[1:]` 所对应 slice；Rust复制到 `Vec`。Go handle/cache 可复用传入 slice 容量；Rust会复制。
- Go `tryAppendHandleColumn` 也把复合 handle 解码错误转成 `false`；Rust保持该语义。两边整数 handle 路径都依赖非空 `handleColIDs`。
- Rust 当前直接生产调用面小于 Go：Go 搜索结果显示 DDL、DistSQL、executor 等多个包调用 rowcodec decoder；Rust 证据只确认 tablecodec 的 map/bytes 路径，`ChunkDecoder` 的应用主链接线尚未由本任务验证。

测试一一覆盖了主要契约：`TestDecodeRowWithHandle` 验证三条路径补 handle；`TestTypesNewRowCodec` 验证类型矩阵；`TestNilAndDefault` 验证缺列、NULL 和默认值；`TestVarintCompatibility` 验证旧整数编码；`TestCodecUtil` 验证 `ColumnIsNull`；`TestEncodeDecodeRowWithChecksum` 验证 checksum 读取；`TestDecodeWithCommitTS` 验证特殊提交时间列。Go 同名测试提供移植意图对照。

## 扩展指南

新增 MySQL 类型时，至少同步检查 `decodeColDatum`、`decodeColToChunk` 和 `fieldType2Flag/encodeOldDatum` 三条路径，并与 encoder、Go `decoder.go` 和旧 codec 能力对齐；不能只让某一个目标表示通过。测试应继续放在独立的 [`rowcodec_test.rs`](rowcodec_test.rs) 或同目录其他 `*_test.rs`，覆盖 Datum map、chunk、old bytes 三条路径以及未知/畸形输入，不要把测试嵌入生产文件。

修改缺列逻辑时必须保持“row value > handle > 显式 NULL > 缺列默认值”的实际约束，其中显式 NULL 不应被默认值覆盖，handle 也不应覆盖前缀聚簇索引的完整 value。新增特殊列应在 `DecodeToChunk` 的普通 `findColID` 之前处理，并补充有值、无值和非法参数测试。

调整 handle 支持时，应同时验证整数/无符号/复合 handle、空 `handleColIDs`、`NeedRestoredData`、`EncodedCol` 失败和前缀索引。若希望把复合 handle 解码错误向上传播，需要评估与 Go 现有 `bool` 降级语义的兼容性，不能静默改变为硬错误。

性能优化应关注 bytes 路径的局部 row 与结果分配、`cacheBytes` 克隆、JSON/string 复制和逐列查找；但不得通过借用外部缓冲破坏 Datum/chunk 的所有权或生命周期。协议兼容风险集中在 type flag、varint/compact bytes、timestamp 时区、decimal rounding 和 vector/JSON payload；任何变化都应使用 Go 固定夹具或跨语言字节结果证明。

Rust 当前 `ChunkDecoder` 缺少已确认的生产主链调用；若后续接线，应从实际上游列元数据、默认值闭包、时区、handle 与 chunk schema 一起验证，而不是把测试 helper 当作完整应用入口。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/rowcodec` 找到 19 个 Rust/Go 文件；`node --file pkg/util/rowcodec/decoder.rs` 读取完整 738 行，并报告五个关联文件；精确 `query` 核对 `ChunkDecoder`、`DecodeToDatumMap`、`DecodeToChunk`、`DecodeToBytes*`、`fieldType2Flag`。`callers/callees` 对 include 内 Rust 方法未在限定时间内返回，故调用边以精确 `rg` 补证并在本文明确限制。
- Rust 源码与 crate 边界：[`decoder.rs`](decoder.rs)、[`row.rs`](row.rs)、[`common.rs`](common.rs)、[`encoder.rs`](encoder.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)，以及 `pkg/tablecodec/tablecodec.rs`、`pkg/store/driver/txn/error.rs` 的直接调用/构造位置。
- Go 对照：[`decoder.go`](decoder.go) 完整实现与 [`rowcodec_test.go`](rowcodec_test.go) 的同名回归测试。
- Rust 独立测试：[`rowcodec_test.rs`](rowcodec_test.rs) 的 `TestEncodeLargeSmallReuseBug`、`TestDecodeRowWithHandle`、`TestTypesNewRowCodec`、`TestNilAndDefault`、`TestVarintCompatibility`、`TestCodecUtil`、`Test65535Bug`、`TestEncodeDecodeRowWithChecksum`、`TestDecodeWithCommitTS`；[`common_1_aster_unit_test.rs`](common_1_aster_unit_test.rs) 的 Datum map 往返和旧 flag 映射；[`bench_test.rs`](bench_test.rs) 的 chunk 解码基准路径。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工检查本文只描述有源码、图查询、Cargo、Go 或独立测试支撑的当前事实。
