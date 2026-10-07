# `br/pkg/rtree/stubs.rs`

## 文件定位

`br/pkg/rtree/stubs.rs` 是 `astersql-br-pkg-rtree` crate 内部的依赖隔离层。crate 入口 `br/pkg/rtree/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并选择性再导出其中的文件元数据、进度写出接口、键区间、校验和及键编码辅助。该 crate 的 `Cargo.toml` 仅声明本地 `astersql-br-pkg-logutil` 依赖，没有引入 `kvproto`、gRPC、`metautil` 或完整 `tablecodec`；本文件因此以小型 Rust 类型和函数承接 `rtree.rs` 实际需要的 Go API 形状。

它不是独立的区间树实现，也没有自行启动的执行入口。业务算法位于 `br/pkg/rtree/rtree.rs`：后者直接导入 `File`、`RpcKeyRange`、`ChecksumStats`、`MetaWriter`、`FreeListG` 和 `AppendDataFile`，并通过 `stubs::DecodeKeyHead`、`stubs::SummaryFiles`、`stubs::redact_key` 使用本文件。`lib.rs` 还把这些符号暴露给 crate 外调用方及独立测试，所以修改公开形状会影响 crate 的兼容面。

## 核心职责

本文件集中承担四类职责：

1. 用 `File`、`RpcKeyRange`、`ChecksumStats` 表示区间树所需的最小备份元数据、RPC 键区间和校验和状态；字段名刻意保持 Go/protobuf 风格。
2. 用 `MetaWriter` trait 与 `AppendDataFile` 常量抽象完成区间的元数据写出，使 `ProgressRangeTree` 不必依赖真实 `metautil.MetaWriter`。
3. 用 `DecodeKeyHead` 及一组编码函数实现 `NeedsMerge` 和测试构键所需的 table key 子集；支持普通表键以及由调用方剥离后的 API V2 keyspace 内层键。
4. 用 `SummaryFiles`、`FreeListG` 和 `redact_key` 补齐 Go 侧工具函数、B-tree freelist 参数和日志展示接口在 Rust 迁移中的最小契约。

“桩”表示依赖边界被缩小，并不表示函数可以返回任意结果。`rtree.rs::NeedsMerge` 的跨表/跨索引隔离、`ProgressRangeTree::collectRangeFiles` 的写出与统计，以及 `GetIncompleteRange` 的返回类型都依赖这里的实际语义。

## 主要符号

- `File`：`backuppb.File` 的任务所需子集，保存 `Name`、`Cf`、`StartKey`、`EndKey`、`TotalBytes`、`TotalKvs`、`Crc64Xor`。`GetName`、`GetStartKey`、`GetEndKey` 提供 Go 风格只读访问器；返回借用，不复制底层字符串或字节。
- `RpcKeyRange`：对应 `kvrpcpb.KeyRange` 的 `StartKey`/`EndKey`。其 `Display` 实现以 `redact_key` 生成 `[start, end)` 十六进制文本。
- `ChecksumStats`：保存异或聚合的 `Crc64Xor` 与累加的 `TotalKvs`、`TotalBytes`，由 `ProgressRangeTree` 按物理表 ID 维护。
- `AppendDataFile`：传给 `MetaWriter::Send` 的类别标记，当前值为 `1`；它是本地契约值，不是完整 Go 枚举定义。
- `MetaWriter: Send`：唯一方法为 `Send(&self, files: &[File], kind: i32) -> Result<(), String>`。`Send` 超 trait 要求允许 writer 被装入 `ProgressRangeTree` 的可发送 trait object；方法用共享引用，具体实现若记录状态需自行提供内部可变性与同步。
- `FreeListG<T>`：只持有 `PhantomData<T>` 的零状态占位类型。`new` 不分配池；`NewRangeTreeWithFreeListG` 接收后忽略它，因为 Rust 主体使用 `BTreeMap`。
- `SummaryFiles(&[File]) -> (u64, u64, u64)`：按 `(crc, kvs, bytes)` 顺序返回；CRC 逐项异或，键数和字节数使用 `wrapping_add`。
- `DecodeKeyHead(&[u8]) -> Result<(i64, i64, bool), String>`：返回 `(table_id, index_id, is_record)`。记录键返回 `index_id = 0`、`is_record = true`；索引键解出索引 ID 并返回 `false`。
- `EncodeIntToCmpUint`、`EncodeRowKeyPrefix`、`EncodeIndexKeyPrefix`、`GenTableRecordPrefix`、`EncodeRecordKey`、`EncodeIndexSeekKey`：构造本 crate 所需的有序整数、行键和索引键布局。
- `EncodeKeyspaceKey`：添加 `b'x'` 与 24 位大端 keyspace ID 前缀，供 API V2 合并路径测试。
- `redact_key`：把每个字节转为两个小写十六进制字符；当前实现是稳定展示而非隐藏敏感信息的不可逆脱敏。

## 执行流程

区间合并路径从 `RangeStatsTree::MergedRanges` 进入 `NeedsMerge`。`NeedsMerge` 先检查文件字节数和键数阈值，再由 `parse_inner_key` 尝试去掉 `x + uint24(keyspace_id)` 前缀，随后调用 `DecodeKeyHead`。解码成功后，两个记录键只有 table ID 相同才可合并；两个索引键还要求 index ID 相同；记录键与索引键不会混并。编码或前缀不合法时，解码返回错误，`NeedsMerge` 保守返回 `false`。

`DecodeKeyHead` 的步骤是：先要求首字节为 `t`；再从其后八字节调用私有 `decode_cmp_uint`，通过翻转最高位恢复 `i64` table ID；若剩余字节以 `_r` 开头则判定为记录键；否则必须以 `_i` 开头，并继续解码八字节 index ID。它只解析键头，不校验 `_r` 后是否存在 row handle，也不校验索引 ID 后的 encoded values。

进度完成路径从 `ProgressRangeTree::GetIncompleteRanges` 进入 `collectRangeFiles`。若没有 writer，该函数直接返回默认零统计；若有 writer，则按 `RangeTree` 的键序遍历每个完成区间，先用 `SummaryFiles` 计算该文件批次的 CRC、键数和字节数，再调用 `MetaWriter::Send(files, AppendDataFile)`，成功后才把批次统计并入结果。最终 `UpdateChecksum` 按 `PhysicalID` 合并统计。

测试及调用方构键时，`EncodeRowKeyPrefix`/`EncodeIndexKeyPrefix` 先写入 `t`、有序编码的 table ID 和 `_r`/`_i`；记录键追加有序编码 row ID，索引 seek 键追加原样的 encoded value。需要 keyspace 时，`EncodeKeyspaceKey` 最后包裹完整业务键。该顺序由 `rtree_test.rs::make_encode_keyspaced_table_record` 明确验证。

## 数据与状态

所有元数据容器都拥有自己的 `String`/`Vec<u8>`，并派生 `Clone`、`Default`、`PartialEq`、`Eq`，便于区间树替换、测试比较和默认构造。访问器返回切片或字符串切片；编码函数则创建新的 `Vec<u8>`，不会修改输入缓冲。

`SummaryFiles` 本身无持久状态，遍历顺序不影响 XOR 和无溢出时的求和结果。为匹配 Go `uint64` 模 2^64 行为，键数与字节数显式使用 `wrapping_add`；`rtree_test.rs::test_uint64_accumulators_match_go_wrapping_semantics` 覆盖了 `u64::MAX + 1 == 0`。CRC 使用 XOR，相同 CRC 出现偶数次会抵消。

`FreeListG<T>` 只通过 `PhantomData<T>` 保留泛型类型关系，不保存节点、不共享内存，也没有回收生命周期。`MetaWriter` 的真实状态由实现者负责；测试实现使用 `Arc<AtomicUsize>` 统计发送文件数。`RpcKeyRange::Display` 和 `redact_key` 每次格式化都会新建字符串。

## 依赖与调用关系

上游模块关系由 RustCodeGraph 文件节点确认：`stubs.rs` 被 `lib.rs`、`parity_test.rs`、`rtree_test.rs` 直接使用；`rtree.rs` 通过 crate 模块路径消费其公开符号，而 `lib.rs` 将选定符号扁平再导出。RustCodeGraph 的调用流还确认 `DecodeKeyHead` 被 `rtree.rs::parse_inner_key` 间接用于 `NeedsMerge`，`SummaryFiles` 被 `ProgressRangeTree::collectRangeFiles` 调用，编码函数被 parity、fuzz 与区间树测试用于生成合法键。

下游仅依赖 Rust 标准库：`std::fmt` 用于 `RpcKeyRange` 展示，`PhantomData` 表达 `FreeListG<T>` 的类型占位，字节切片与整数大端转换完成 codec 子集。`Cargo.toml` 没有为本文件引入外部依赖；crate 唯一列出的 `astersql-br-pkg-logutil` 由相邻模块使用，不是这里的直接依赖。

Go 对应实现并不集中在一个 `stubs` 文件：`br/pkg/rtree/rtree.go` 直接依赖 `kvproto` 的 `backuppb.File`/`kvrpcpb.KeyRange`、`metautil.MetaWriter`/`ChecksumStats`、`utils.SummaryFiles`、`tablecodec.DecodeKeyHead`、TiKV `DecodeKey` 和 Google B-tree freelist。本文件把这些跨包依赖压缩为 rtree crate 的局部边界。

## 错误处理与边界

`decode_cmp_uint` 对不足八字节返回 `"insufficient bytes to decode int"`。`DecodeKeyHead` 对空键、非 `t` 前缀、缺少 `_r`/`_i` 分隔符返回包含原始字节调试表示的 `"invalid key"`；索引 ID 截断则传播整数解码错误。调用方 `NeedsMerge` 不传播这些错误，而是拒绝合并，避免错误键导致跨表或跨索引误并；`merge_fuzz_test.rs::fuzz_merge` 用空键、随机短键、伪 keyspace 前缀和畸形 `t` 前缀验证该路径不得 panic。

编码助手不返回错误。`EncodeKeyspaceKey` 只保留 `u32` 的低 24 位，因为输出固定为三个 ID 字节；调用方若传入大于 `0x00ff_ffff` 的值，高位会被截断，当前接口不会报告这一点。`DecodeKeyHead` 不识别 keyspace 外层，必须由 `rtree.rs::parse_inner_key` 先剥离四字节前缀。

`MetaWriter::Send` 的字符串错误由 `collectRangeFiles` 立即向 `GetIncompleteRanges` 传播。失败批次不会并入 checksum，遍历停止，已收集但尚未进入第二阶段的完成项也不会从进度树删除；完成回调仅在该项写出成功后执行。没有 writer 时，完成项仍可被移除和回调，但其返回统计为零。

数值累加采用回绕语义而不是溢出错误。`Display` 不失败于键内容，但格式化 writer 自身仍可返回 `fmt::Error`。`redact_key` 的十六进制输出会泄露完整键字节，不能在需要真正敏感信息遮蔽的边界上直接等同于 Go `redact.Key` 的策略配置。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部连接。编码、解码和汇总函数都是同步的纯内存计算；输入借用只持续到函数返回，输出由调用方拥有。

`MetaWriter: Send` 允许 writer 所有权随 `ProgressRangeTree` 跨线程移动，但 trait 没有要求 `Sync`。`Send` 接收 `&self`，因此带计数、缓冲或 I/O 状态的实现必须使用锁、原子或其他内部可变性保证自己的并发约束；本文件不替实现者串行化调用。`ProgressRangeTree` 当前按顺序调用 writer，且持有 `Box<dyn MetaWriter>` 直至树被释放。

传给 `MetaWriter::Send` 的 `&[File]` 借用来自区间树，writer 不得在调用返回后保留该切片引用。`FreeListG` 没有资源可释放；其构造和销毁均不影响 `BTreeMap`。各编码结果和十六进制字符串由普通 Rust 所有权管理。

## 与 Go 版本的对应关系

`File`、`RpcKeyRange`、`ChecksumStats` 分别映射 Go 路径中的 `backuppb.File`、`kvrpcpb.KeyRange`、`metautil.ChecksumStats`。Rust 使用值类型 `Vec<File>`，Go 使用 `[]*backuppb.File`；因此 Rust 不表达 nil 文件元素，也不会共享单个 protobuf 指针的后续变更。Rust `MetaWriter` 是窄 trait，Go 则使用具有异步启动、完成和存储后端行为的具体 `*metautil.MetaWriter`；本地 trait 只保留 rtree 真正调用的 `Send`。

`SummaryFiles` 对齐 `br/pkg/utils/misc.go::SummaryFiles` 的返回顺序与数值聚合：CRC XOR、KVs 求和、bytes 求和。Go 函数还按 CF 收集全局 summary 指标，Rust 桩没有这项观测副作用；因此它在数值结果上对齐，但不是完整监控替代品。

`DecodeKeyHead` 对齐 `pkg/tablecodec/tablecodec.go::DecodeKeyHead` 的 `t + cmp(tableID) + _r/_i` 判定及返回形状。Go 使用通用 codec 和带栈错误，Rust 在本文件中直接实现八字节有序整数并返回 `String`。API V2 外层在 Go `NeedsMerge` 中由 `tikv.DecodeKey` 处理；Rust 当前只识别固定的 `x + uint24` 形式，并在 `parse_inner_key` 中处理，覆盖面小于完整 TiKV codec。

Go `btree.FreeListG` 真实复用 B-tree 节点；Rust `FreeListG` 只是保持构造签名，底层 `BTreeMap` 没有对应池化优化。Go `redact.Key` 受日志脱敏配置控制，而本文件的 `redact_key` 始终输出完整 hex。编码函数主要为 Rust 测试及合并构键服务；它们只覆盖整数 handle 和已编码索引值拼接，不应被当成完整 `tablecodec` API。

## 扩展指南

若区间树需要新增 `backuppb.File` 字段，先确认 `rtree.rs` 的实际读取/写出需求，再扩展 `File`，并同步 `br/pkg/rtree/rtree_test.rs` 中的构造器、内存 writer 和 Go 对照断言。不要为了“看起来完整”复制整个 protobuf；这样会破坏本文件隔离重依赖的目的。

若增加新的键形态，应优先扩展 `DecodeKeyHead`/`parse_inner_key` 的明确契约，并同步 `Encode*` 助手。必须覆盖合法记录键、合法索引键、不同表、不同索引、截断整数、错误分隔符、普通键与 keyspace 键；相关独立测试位置是 `rtree_test.rs`、`merge_fuzz_test.rs` 和 `parity_test.rs`，不要把测试内嵌进 `stubs.rs`。涉及通用 tablecodec 的能力应复用 canonical crate，而不是继续扩大局部桩。

若 writer 需要异步、批量、flush 或关闭语义，应在 `MetaWriter` 与 `ProgressRangeTree::collectRangeFiles` 之间设计清晰的成功边界，保持“写出成功后才回调、删除并计入 checksum”的不变量。需要真实 `metautil` 行为时，应在上游独立 crate 中提供可发布依赖，而不是把实现复制进本文件。

性能方面，`redact_key` 当前逐字节调用 `format!`，大键或高频错误日志可能产生较多分配；若优化，应保持小写、两位补零和现有 `Display` 形状。`SummaryFiles` 为 O(n) 且无额外分配；不要为了统计 CF 等观测项引入会改变热路径的全局锁，除非同步评估 Go 行为和调用频率。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/rtree` 列出本 crate 的源文件与独立测试。
- RustCodeGraph `node --file br/pkg/rtree/stubs.rs --offset 1 --limit 400`：核对本文件 212 行完整源码、23 个符号及 `lib.rs`/测试使用关系。
- RustCodeGraph `node`：读取 `br/pkg/rtree/lib.rs` 和 `br/pkg/rtree/rtree.rs`，确认再导出面、`NeedsMerge -> parse_inner_key -> DecodeKeyHead`、`collectRangeFiles -> SummaryFiles/MetaWriter::Send`、checksum 与错误传播流程。
- RustCodeGraph `explore "br/pkg/rtree/stubs.rs symbols callers callees role in rtree crate"`：确认 `DecodeKeyHead`、编码助手、`SummaryFiles` 等调用流；同名符号较多，因此最终归属以文件限定的 `node` 和相邻调用源码交叉核验。
- `br/pkg/rtree/Cargo.toml`：确认 crate 名、`lib.rs` 入口、Go package 映射及未引入 kvproto/gRPC/tablecodec 重依赖。
- Go 对照：`br/pkg/rtree/rtree.go`、`pkg/tablecodec/tablecodec.go::DecodeKeyHead`、`br/pkg/utils/misc.go::SummaryFiles`，分别核对实际接线、键头解析和文件统计语义。
- 独立测试：`br/pkg/rtree/rtree_test.rs` 覆盖普通/keyspace 合并、writer 类别、checksum 和 `uint64` 回绕；`br/pkg/rtree/merge_fuzz_test.rs` 覆盖畸形键不 panic；`br/pkg/rtree/parity_test.rs` 覆盖公开再导出、编码、写出与摘要合约。Go 对照测试为 `br/pkg/rtree/rtree_test.go` 与 `merge_fuzz_test.go`。
- 本任务只新增说明文档，未运行 Cargo。交付前使用任务指定命令验证恰好存在 11 个固定二级章节，并检查文档只链接真实路径、没有把局部桩描述成完整依赖实现。
