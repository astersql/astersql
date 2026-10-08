# `pkg/util/codec/codec.rs`

## 文件定位

本文件是 `astersql-util-codec` crate 的 Datum/Chunk 通用编解码、等值哈希与 Join key 序列化核心。crate 入口 `pkg/util/codec/lib.rs` 将本模块及 `bytes.rs`、`decimal.rs`、`float.rs`、`number.rs` 的公开项重新导出；`pkg/util/codec/Cargo.toml` 的 `[lib] path = "lib.rs"` 说明它不是独立二进制。其上游包括 `pkg/tablecodec/tablecodec.rs` 的表记录/索引值编解码、`pkg/statistics/runtime_stats_builder.rs` 的统计边界编码与还原、`pkg/planner/util/handle_cols.rs` 的 handle key 生成，以及 `pkg/session/runtime/*` 的行和索引处理。

编码流以首字节 flag 区分 Datum 类型，随后委托同 crate 的数值、字节、浮点、Decimal 编解码器处理载荷。`EncodeKey` 产生保持原值排序关系的 memcomparable 表示，`EncodeValue` 产生更紧凑的存储表示；此外，本文件还直接面向 Chunk 提供 Join、聚合及哈希表所需的等值编码。依据：`codec.rs::{EncodeKey, EncodeValue, DecodeOne, SerializeKeys, HashGroupKey}`、`lib.rs`、上述直接调用文件。

## 核心职责

1. 定义编码协议的类型标记：`NilFlag`、`bytesFlag`、`compactBytesFlag`、`intFlag`、`uintFlag`、`floatFlag`、`decimalFlag`、`durationFlag`、`varintFlag`、`uvarintFlag`、`jsonFlag`、`vectorFloat32Flag`、`maxFlag`。`IntHandleFlag` 是 `intFlag` 的公开别名；这些数值必须与 Go 和既有持久化/网络字节兼容。
2. 由 `Encoder::encode` 按 `Datum::Kind()` 分派，并通过 `comparable1` 在定长可比较编码与紧凑 varint/compact-bytes 编码之间选择。字符串 key 在启用新 collation 时先生成 `ImmutableKey`。
3. 由 `Decode`、`DecodeOne`、`DecodeRange`、`DecodeAsDateTime`、`DecodeAsFloat32` 还原 Datum；由 `CutOne`、`CutColumnID`、`SetRawValues` 和 `peek*` 在不完整解码的情况下切分编码流。
4. 由 `Decoder::DecodeOne` 将编码值直接追加到 Chunk，按目标 `FieldType` 恢复 timestamp 时区、Decimal 小数位、Enum/Set/Bit 等 SQL 语义，并复用 bytes 缓冲。
5. 由 `EncodeHashChunkRowIdx`、`HashChunkSelected`、`HashChunkRow`、`EqualChunkRow`、`HashGroupKey`、`HashCode`/`Hash64` 生成逻辑等值所需的规范字节；字符串、Enum、Set 均受 collation 约束，浮点零值会规范化以避免 `-0` 与 `+0` 哈希不一致。
6. 由 `SerializeKeys` 为 Join 的多列、多行 key 先计算长度、标记被过滤或含 NULL 的行，再按 `SerializeMode` 写入每行缓冲。

## 主要符号

- `Encoder { useNewCollate }`、`NewEncoder`、`UseNewCollate`：把一次编码使用的新 collation 策略固定在实例上；包级 `EncodeKey`、`EncodeValue`、`HashCode` 则读取 `collate::NewCollationEnabled()` 后创建临时 Encoder。
- `Encoder::{EncodeKey, EncodeValue, HashCode}`：分别生成可排序 key、紧凑 value、逻辑哈希输入。`HashCode` 对 Decimal 使用字符串、对时间使用 core time、对 JSON 使用 type code 加 value，以避免值编码中的精度或转换行为改变等值判定。
- `EstimateValueSize`、`preRealloc`：估计紧凑 value 或整批编码容量；前者对未知 Kind 和 Decimal 尺寸错误返回 `SharedError`，后者遇到未知 Kind 仅放弃预留。
- `EncodeMySQLTime`：timestamp 非 UTC 时先转换到 UTC，再 `ToPackedUint` 并 `EncodeUint`；`DecodeAsDateTime` 和 `appendUintToChunk` 在读取侧按目标时区转换回来。
- `EncodeHashChunkRowIdx`：依据 `FieldType::GetType()` 返回 `(flag, payload)`，是行哈希、列哈希和行等值比较的共同规范化入口。
- `SerializeMode::{Normal, NeedSignFlag, KeepVarColumnLength}`：分别表示普通序列化、需要显式 signed/unsigned 标记、需要为变长字段写长度。
- `SerializeKeys`：公开 Join key 入口；`preAllocForSerializedKeyBuffer` 计算 `serializedKeyLens` 和 `nullVector`，`serializeKeysImpl` 再逐列写值。测试模式下检查实际写入没有突破预估容量。
- `HashChunkColumns`/`HashChunkSelected`：向每行独立的 `Hasher` 写单列；可用选择向量跳过行，并通过 `ignoreNull` 控制 NULL 是否更新 `isNull`。
- `HashChunkRow`、`EqualChunkRow`：分别输出多列行哈希字节、逐列比较规范化编码；后者先校验两侧列数一致。
- `Decode`/`DecodeOne`/`DecodeRange`：一般解码、单值解码和索引范围边界解码。`DecodeRange` 认识尾部 `NilFlag`、`bytesFlag`（MinNotNull）、`maxFlag` 及 `maxFlag + 1`（PrefixNext）。
- `Decoder { chk, timezone, buf }`、`NewDecoder`、`Decoder::DecodeOne`：持有目标 Chunk 裸指针、时区和复用缓冲，逐值写列。
- `HashGroupKey`：聚合执行器按 `EvalType` 为列中每行追加 group key；`ConvertByCollation*` 是字符串规范化辅助。
- `Hash64`、`init`：复用 `base::Hasher` cache 计算 Datum hash；`init` 把全局 `types::Hash64ForDatum` 指向本实现。

## 执行流程

通用编码从 `EncodeKey` 或 `EncodeValue` 开始：读取全局 collation 开关，构造 `Encoder`，预估容量，然后遍历 Datum。整数在 key 模式使用 flag 加 8 字节定长编码，在 value 模式使用 flag 加 varint；bytes/string 在 key 模式使用 8 字节分组的 memcomparable 编码，在 value 模式使用 compact bytes；时间、Duration、Decimal、JSON、Vector 按各自协议附加。任一不支持的 Kind 会终止并返回错误。

通用解码从首字节 flag 分派。`DecodeOne` 只消费一个值并返回余下 slice，`Decode` 循环到输入耗尽；`DecodeRange` 在有列类型时专门恢复时间和 float32，并识别仅剩一个边界 flag 的索引范围。切分路径 `CutOne -> peek -> peekBytes/peekCompactBytes/peekVarint/peekUvarint` 只计算当前值占用长度，`SetRawValues` 据此把连续行数据分配给各 Datum 的 raw 字节。

Chunk 解码由调用方构造 `Decoder`，随后按列重复调用 `Decoder::DecodeOne`。它先按协议解出载荷，再通过 `appendIntToChunk`、`appendUintToChunk`、`appendFloatToChunk` 结合目标 `FieldType` 写入正确的物理列；Decimal 若编码 frac 大于字段 frac，会以 `ModeHalfUp` 四舍五入。

Join key 走两阶段：`preAllocForSerializedKeyBuffer` 遍历 key 列和 `usedRows`，同时应用 `filterVector`、传播 NULL 到 `nullVector`、计算每个逻辑行长度；随后为每行创建准确容量的 Vec，`serializeKeysImpl` 按同样跳过规则写入整数原始字节、规范化浮点、collation key、Decimal hash key、Enum/Set/Bit/JSON 等。`NeedSignFlag` 防止跨有符号类型比较冲突，`KeepVarColumnLength` 防止多变长列拼接歧义。

哈希/等值路径统一依赖类型规范化：`HashChunkRow` 和 `HashChunkSelected` 调用 `EncodeHashChunkRowIdx` 后写 flag 与 payload，`EqualChunkRow` 比较同一结果；`HashGroupKey` 则按评估类型批量为聚合分组生成等价字节。

## 数据与状态

协议状态主要存在于输出字节中：一个 flag 后接该类型的载荷。可比较整数/浮点/时间通常为 flag 加 8 字节；紧凑整数为 varint；bytes 有 memcomparable 分组和 compact 长度前缀两种形态。`NilFlag` 既代表 NULL，也参与 join/hash 中的空值标记；`bytesFlag` 在 `DecodeRange` 的单字节尾部语境代表 MinNotNull，`maxFlag`/`maxFlag + 1` 代表 MaxValue 边界。

`Encoder` 只有不可变布尔状态 `useNewCollate`。`Decoder` 保存目标 Chunk 的 `*mut chunk::Chunk`、`timezone` 和可复用 `buf`；每次 bytesFlag 解码用 `mem::take` 暂时移出缓冲，解码后再保存。`SerializeKeys` 会原位更新 `nullVector`、`serializedKeys`、`serializedKeyLens`，并返回按总估算长度 resize 的 `serializedKeysBuffer`；Rust 实现无法像 Go slice 一样让各行共享同一 backing array，因此每行 Vec 独立分配，但容量约束由测试检查。

全局状态有两处：包级入口读取 `collate::NewCollationEnabled()`；`init` 通过 unsafe 赋值设置 `types::Hash64ForDatum`。本文件本身不持有事务、文件、网络连接或后台任务。

## 依赖与调用关系

下游 crate 依赖由 `pkg/util/codec/Cargo.toml` 给出：Datum/Decimal/Field/JSON/Vector 类型来自 `astersql-types-*`，Chunk 来自 `astersql-util-chunk`，字符序来自 `astersql-util-collate`，错误来自 `astersql-errors`，MySQL 类型常量来自 parser/mysql，哈希接口来自 planner/cascades/base；`logutil` 用于意外 Datum Kind 警告，`intest` 控制测试期容量断言。底层字节、数字、浮点和 Decimal 函数由 `lib.rs` 的兄弟模块重导出后在本文件直接调用。

RustCodeGraph 将 `codec.rs` 识别为 89 个符号的文件，并显示其被 `pkg/statistics/histogram.rs`、`pkg/statistics/runtime_stats_builder.rs`、`pkg/session/runtime/typed_adapter_bridge.rs` 等文件使用。仓库直接搜索进一步确认：`pkg/tablecodec/tablecodec.rs` 大量调用 `EncodeValue`、`DecodeOne`、`CutOne` 组装表记录与索引；`pkg/statistics/runtime_stats_builder.rs` 调用 `EncodeKey`/`EncodeValue`/`Decode`/`DecodeRange` 保存统计边界；`pkg/planner/util/handle_cols.rs` 用 `EncodeKey` 生成 handle；`pkg/session/runtime/{dml,row_codec,relational_scan}.rs` 经 tablecodec 重导出编码或解析行值。这些边说明本文件位于 SQL 类型值与 KV/执行器字节表示之间，而不是负责表 key 前缀或行格式全部语义。

RustCodeGraph 的精确 `callers/callees` 命令对本文件函数 ID 未输出调用边，因此以上具体边以仓库搜索和调用点源码为准；未把索引缺失推断成“无调用者”。

## 错误处理与边界

公开可失败接口统一返回 `errors::SharedError`。空输入、未知 flag、截断的定长/bytes/varint/JSON/Vector 载荷、无效索引列类型长度、未知 Datum Kind 或 FieldType 都会返回错误。`peek` 在计算长度后还校验不超过原输入，`peekCompactBytes` 使用 checked arithmetic 防止负长度或溢出。

有意的宽容行为必须保留：BinaryLiteral/Bit 转 u64 失败时若调用处使用 `unwrap_or_default()` 会退化为 0；HashCode 遇到未知 Datum Kind 只记录警告并返回已有 buffer；Enum 解码失败产生空 Enum；Enum 哈希找不到值时以空字符串参与 collation；这些均与相邻 Go 逻辑的“记录/忽略并继续”意图对应。Set 解析、时间打包/时区转换、Decimal 编码/round/hash key 生成则传播错误。

安全边界集中在裸指针参数：`EncodeHashChunkRowIdx`、`SerializeKeys`、Chunk 哈希、`Decoder`、`HashGroupKey` 均会解引用调用方提供的 `*mut Chunk`、`*mut Column` 或 `*mut FieldType`。调用方必须保证指针非空、生命周期覆盖调用、列索引和向量长度匹配；Rust 类型系统未在这些签名中验证这些不变量。`SerializeKeys` 还假定 `tps`、`buildKeyIndexs`、`serializeModes` 等按 key 列对齐，`serializedKeys`/`serializedKeyLens` 按 `usedRows` 对齐。

## 并发与资源生命周期

函数总体是同步、CPU/内存内操作，不创建线程、future、channel 或锁。`Encoder` 可按值独立使用；`Decoder` 持有可变 Chunk 裸指针且会修改 Chunk 与内部缓冲，不应跨线程共享，也不得在目标 Chunk 失效后继续使用。`SerializeKeys`、`HashGroupKey` 和 Chunk 哈希函数同样原位修改调用者缓冲或哈希器，调用方负责独占可变访问。

资源优化主要是容量预估和缓冲复用：`preRealloc` 减少 Datum 编码扩容；`SerializeKeys` 先计算容量并在 `intest::InTest` 下断言未再扩容；`Decoder::buf` 复用 memcomparable bytes 的解码存储；JSON 序列化复用局部 `jsonHashBuffer`。无 RAII 外部资源需要关闭。全局 collation 开关与 `types::Hash64ForDatum` 属于进程级共享状态；测试通过 `CollationRestore::drop` 恢复开关，业务代码应避免并发改变测试开关。

## 与 Go 版本的对应关系

直接对照为 `pkg/util/codec/codec.go`，Cargo metadata 也声明 `go-package = "pkg/util/codec"`。Rust 保留相同 flag 数值、主要公开名称、Kind/FieldType 分派、timestamp UTC 往返、collation key、Join 序列化模式、NULL 传播、范围尾标记及 Chunk 追加语义。`pkg/util/codec/codec_test.rs` 与 `codec_test.go` 覆盖同名的 key/value 往返、排序、数字/浮点/bytes/时间/Duration/Decimal/JSON、Cut/RawValues、Chunk 解码、Group/Row/Column hash、范围解码和 Datum Hash64；`collation_test.rs` 与 `collation_test.go` 覆盖 Encoder、GroupKey、行/列哈希的字符序等价性。

语言适配包括：Go `[]byte`/variadic Datum 变成拥有所有权的 `Vec<u8>`/`Vec<Datum>`；Go 指针与 slice 共享 backing array 在 Rust 中多处改为复制或独立 Vec；固定宽度尺寸常量由 `unsafe.Sizeof` 改为字面量；Go `hash.Hash64`/`io.Writer` 对应 Rust trait object；错误经 `shared_error` 转为 `SharedError`。特别是 `SerializeKeys` 的共享大 buffer 不能直接承载每行 Vec，Rust 返回同长度 buffer，同时为每行独立预分配容量；`TestSerializeKeysUsesPreallocatedCapacityWithoutZeroPrefix` 固定了“容量不等于初始长度”的 Rust 回归语义。Rust 还包含 `go_merge_11_type_null_marks_join_key_as_null`，确保 TypeNull 行进入 `nullVector`。

未发现条件编译项。当前文件不是桩或门面：编码、解码、哈希与序列化逻辑均在此实现；底层原语才位于兄弟模块。

## 扩展指南

新增 Datum 编码类型时，至少同步：flag 常量；`Encoder::encode` 与 `Encoder::HashCode`；`preRealloc`/`EstimateValueSize`；`DecodeOne`；`peek`；`Decoder::DecodeOne`；必要时 `DecodeRange` 的类型恢复，并在 `codec_test.rs` 添加往返、截断输入、尺寸估算和 key 排序测试。flag 是跨语言/存储协议，不能复用既有值，必须同步 Go 对照和兼容性评估。

新增 Chunk FieldType 支持时，需检查 `EncodeHashChunkRowIdx`、`preAllocForSerializedKeyBuffer`、`serializeKeysImpl`、`HashGroupKey` 及三个 append helper，保证预估长度与实际写入完全一致、NULL/filter 规则一致、逻辑相等值产生相同字节。同步测试应放在独立的 `pkg/util/codec/codec_test.rs` 或字符序专用 `collation_test.rs`，不要把测试内嵌进生产文件。

修改字符串、Enum、Set 或 JSON 规则时，应同时验证 Join、聚合和 hash row/columns 三条路径；修改 timestamp 时同时验证 encode UTC、DecodeRange 和 Chunk decoder 的反向时区转换；修改浮点哈希时保留 `-0 == +0`。性能变更需关注大 Chunk 下的二阶段预分配、克隆/复制和 `to_ne_bytes` 的平台本地字节序兼容，不能只以编译成功作为行为证据。

任何裸指针 API 的重构都应优先收窄为借用；若为保持 Go 形状必须保留指针，新增入口需明确并测试长度、索引、生命周期不变量。`init` 修改还需核对 crate 初始化路径，避免 `Hash64ForDatum` 未注册或并发写入。

## 验证依据

- 已完整读取：`pkg/util/codec/codec.rs`（RustCodeGraph 报告 1928 行、89 个符号）、`pkg/util/codec/Cargo.toml`、`pkg/util/codec/lib.rs`、`pkg/util/codec/codec.go`。
- 已读取独立测试：`pkg/util/codec/codec_test.rs`、`pkg/util/codec/collation_test.rs`，并与 `codec_test.go`、`collation_test.go` 的同名用例核对。关键证据包括 `TestCodecKey`、`TestCodecKeyCompare`、`TestDecodeOneToChunk`、`TestHashGroup`、`TestSerializeKeysUsesPreallocatedCapacityWithoutZeroPrefix`、`TestDecodeRange`、`TestHashChunkRow`、`TestHashChunkColumns`、`TestDatumHashEquals` 及四个 collation 测试。
- RustCodeGraph：`status` 显示本仓库索引包含 11467 文件、307296 节点、1848419 边；`files --filter pkg/util/codec` 覆盖 Rust/Go 源与测试；`query` 精确定位 Rust `SerializeKeys`（1105）、`DecodeOne`（1328/1612）、`Decoder`（1594）、`HashGroupKey`（1782）。精确函数 ID 的 `callers/callees` 未返回内容，调用边改由仓库直接搜索核验并在“依赖与调用关系”中限定说明。
- 直接调用点证据：`pkg/tablecodec/tablecodec.rs`、`pkg/statistics/runtime_stats_builder.rs`、`pkg/statistics/{builder,fmsketch,cmsketch_util}.rs`、`pkg/planner/util/handle_cols.rs`、`pkg/session/runtime/{dml,row_codec,relational_scan,typed_adapter_bridge}.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务给定的 11 章节结构验证，并人工检查文档只描述可由上述符号、调用点、Cargo 或 Go/测试文件支持的事实。
