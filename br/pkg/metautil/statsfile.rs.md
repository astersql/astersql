# `br/pkg/metautil/statsfile.rs`

## 文件定位

该文件属于 `astersql-br-pkg-metautil` library crate（见 `br/pkg/metautil/Cargo.toml`），实现 BR 统计信息的备份文件写入与恢复读取协议。`br/pkg/metautil/lib.rs` 以 `pub mod statsfile` 挂载并通过 `pub use statsfile::*` 再导出其公开符号；同 crate 的 `MetaWriter::NewStatsWriter`（`br/pkg/metautil/metafile.rs`）会复用元数据写入器的对象存储与加密配置来构造 `StatsWriter`。

它处在 schema 元数据与对象存储之间：备份侧把每张表或分区的 `JSONTable` 聚合成 `StatsFile`，小文件可内联到 `StatsFileIndex`，其余文件写入 `Storage`；恢复侧读取 index，完成解密、校验、旧物理 ID 到新物理 ID 的重写，再将 `PartitionStatisticLoadTask` 送给统计加载器。当前 RustCodeGraph 索引未发现 `RestoreStats` 的 Rust 生产调用者，只有本文件内部调用和测试覆盖；Go 生产恢复入口仍见 `br/pkg/restore/snap_client/pipeline_items.go`。因此恢复实现是已具备的 crate API，但不能据此断言 Rust 恢复主链已经接线。

## 核心职责

1. 定义两个 Go 对齐的可调阈值：`maxStatsJsonTableSize` 默认 32 MiB，控制累计统计 JSON 何时刷盘；`inlineSize` 默认 8 KiB，控制第一个 stats 文件是否直接内联到索引。
2. 通过 `StatsWriter` 累积 `StatsBlock`，维护刷盘后的 `StatsFileIndex`，并保证文件名、摘要、原始/加密大小和 IV 可供恢复使用。
3. 通过 `marshalStatsJSONTable` / `unmarshalStatsJSONTable` 保存 `JSONTable` 的完整字段形状。
4. 通过 `RestoreStats`、`downloadStats` 和 `downloadOneStatsFile` 并发下载 stats 文件，完成解密、明文 SHA-256 校验、反序列化和 physical ID 重写。
5. 在刷盘和恢复过程中尽早清空大块临时字节，降低统计数据较大时的峰值内存占用。

这里有一个重要的迁移边界：`StatsFile`、`StatsFileIndex`、`JSONTable`、`StatsReadWriter` 目前来自 `br/pkg/metautil/stubs.rs`。其中 `Message::write_to_bytes` 和 `parse_from_bytes` 实际使用 `serde_json`，并非真实 protobuf 编码；所以现有实现和测试证明的是 Rust crate 内部闭环及 Go 逻辑对齐，尚不能证明它生成的 stats 容器字节可与 Go protobuf 产物互操作。

## 主要符号

- `maxStatsJsonTableSize: AtomicUsize`、`inlineSize: AtomicUsize`：公开的进程级原子阈值。生产读取使用 `Ordering::SeqCst`；测试通过 `StatsConfigTestGuard` 串行改写并恢复默认值。
- `getStatsFileName(physicalID: i64) -> String`：按 Go 的 `%09d` 规则生成 `backupmeta.schema.stats.<physical-id>`。位数超过九位时不截断。
- `marshalStatsJSONTable(&JSONTable)` / `unmarshalStatsJSONTable(&[u8])`：使用 `serde_json` 编解码统计表，错误经 `trace_err` 包装。
- `StatsWriter`：持有共享对象存储 `storage`、可选 `cipher`、已完成的 `statsFileIndexes`，以及当前批次的 `totalSize` 和 `statsFile`。
- `newStatsWriter(...) -> StatsWriter`：构造空写入器；它本身公开，但通常由 `MetaWriter::NewStatsWriter` 间接创建。
- `StatsWriter::BackupStats`：追加一张表或一个分区的统计块；`None` 是无操作；累计 JSON 字节数严格大于阈值时刷盘。
- `StatsWriter::BackupStatsDone`：收尾入口；无残余块时返回既有索引，否则用残余批次第一个 block 的 physical ID 命名并刷出。
- `flushTemporary` / `clearTemporary` / `writeStatsFileAndClear`：内部刷盘实现。前两者重置临时状态，后者决定内联或写对象存储并创建索引。
- `RestoreStats`：恢复编排入口，同时运行下载生产者和 `StatsReadWriter::LoadStatsFromJSONConcurrently` 消费者。
- `downloadStats`：固定四 worker 的 index 调度器，使用容量为 8 的同步通道传递任务，并保留首个 worker 错误。
- `downloadOneStatsFile`：单个 index 的实际读取器，负责内联/远端分支、解密、校验、解析、ID 重写和任务发送。

## 执行流程

备份写入流程如下：

1. `MetaWriter::NewStatsWriter` 克隆 `Storage` 与当前 `CipherInfo`，调用 `newStatsWriter`。
2. 调用方对每个表或分区调用 `BackupStats`。若 `jsonTable` 为 `None`，函数立即成功返回；否则先序列化为 JSON 字节，累加 `totalSize`，再把旧 `physicalID` 和 JSON 字节组成 `StatsBlock`。
3. 只有当 `totalSize > maxStatsJsonTableSize` 时才调用 `writeStatsFileAndClear`；等于阈值不会刷盘。
4. `writeStatsFileAndClear` 先序列化整个 `StatsFile` 并立刻清空临时缓冲。若这是第一个 index 且序列化内容严格小于 `inlineSize`，内容直接进入 `StatsFileIndex.inline_data`，不访问对象存储。
5. 非内联路径先对明文计算 SHA-256，再调用 `Encrypt`，随后以 `getStatsFileName(physicalID)` 写入对象存储，并把文件名、明文摘要、密文大小、明文大小和 IV 记入 index。
6. 所有统计追加完毕后调用 `BackupStatsDone`。残余数据以当前缓冲区第一个 block 的 physical ID 命名；返回值是完整 index 列表，供 schema 元数据引用。

恢复流程如下：

1. `RestoreStats` 创建容量为 8 的 `sync_channel`，单独启动下载线程执行 `downloadStats`，当前线程调用 `LoadStatsFromJSONConcurrently(newTableInfo, rx, 0)`。参数 `0` 保留 Go 的“由 handler 决定并发度，并跳过此处 cache 更新”的调用约定。
2. `downloadStats` 为每个 `StatsFileIndex` 向固定大小为 4 的 worker pool 派发任务。派发前若 context 已取消或已有 worker 记录错误，则停止新增任务；已经启动的任务仍通过 `WaitGroupWrapper` 等待结束。
3. `downloadOneStatsFile` 优先使用非空 `inline_data`。否则调用 `Storage::ReadFile`，用 index 中的 IV 解密，并对解密后的明文计算 SHA-256；它与写入侧“先摘要、后加密”的顺序对称。
4. 解析得到 `StatsFile` 后逐块处理：必须在 `rewriteIDMap` 中找到旧 physical ID；JSON 字节必须能解析成完整 `JSONTable`；随后清空 block 内原始 JSON 字节并发送带新 ID 的加载任务。
5. context 取消或接收端关闭时，单文件任务安静返回 `Ok(())`。所有 worker 完成且 sender 被释放后，接收端迭代结束；`downloadStats` 返回首个捕获的错误。
6. `RestoreStats` 等待下载线程并合并结果。下载线程 panic 会变成 `downloadStats worker panicked`；实现先传播下载错误，再返回加载器错误。

## 数据与状态

`StatsWriter` 的持久状态分为两层：`statsFileIndexes` 是已经完成、最终要写入 schema 的索引；`totalSize` 与 `statsFile` 是下一次刷盘前的临时批次。`flushTemporary` 无论序列化结果成功与否都会在取得结果后调用 `clearTemporary`，因此调用失败后原缓冲也已被清空，调用方不能靠重试同一个 writer 自动恢复未写数据。

`totalSize` 只累计每个 `JSONTable` 自身序列化后的字节数，不包含 `StatsFile` 容器开销；内联判断则针对整个 `StatsFile` 序列化后的长度。第一个小文件可内联，但一旦已有任何 index，后续文件即使很小也必须成为命名对象。远端 index 的 `sha256` 对应明文，`size_ori` 是明文容器长度，`size_enc` 是加密后长度；当前读取路径会使用摘要与 IV，但没有使用两个 size 字段做额外长度验证。

每个 `StatsBlock` 保存备份时的旧 physical ID。恢复时 `rewriteIDMap` 决定目标表或分区 ID；一个 `StatsFile` 可以包含多个 block，每块独立重写并产生一个任务。`JSONTable` 的 Columns、Indices、Partitions、PredicateColumns、Count、ModifyCount、Version、IsHistoricalStats 等字段由 `stubs.rs` 的 serde 形状保存，`statsfile_test.rs` 明确验证完整往返。

## 依赖与调用关系

上游与入口：

- `br/pkg/metautil/lib.rs` 挂载并再导出本模块。
- `br/pkg/metautil/metafile.rs::MetaWriter::NewStatsWriter` 是同 crate 的生产构造入口，共享 meta writer 的 storage/cipher。
- RustCodeGraph 对目标文件给出的直接使用文件为 `br/pkg/metautil/parity_test.rs` 与 `br/pkg/metautil/statsfile_test.rs`；对 `RestoreStats` 的函数查询只返回 Go/Rust 定义，没有 Rust 生产 caller。`br/pkg/backup/schema.rs` 存在同名 stats 写入流程，但该 crate 使用自己的 `stubs::metautil::StatsWriter` 边界，不能当作本文件类型的直接调用证据。
- Go 对应的生产恢复调用位于 `br/pkg/restore/snap_client/pipeline_items.go`，它从已恢复 schema 的 `StatsFileIndexes` 和 rewrite map 调用 `metautil.RestoreStats`。

主要下游：

- `astersql-objstore-storeapi::{Storage, Context}`：对象读写和取消上下文。
- `crate::metafile::{Encrypt, sha256_bytes, hex_encode}` 与 `astersql_br_pkg_utils::encryption::Decrypt`：加解密、摘要与诊断格式。
- `astersql-br-pkg-errors`：`ErrInvalidMetaFile` 和 `ErrRestoreInvalidRewrite` 两类语义错误。
- `astersql-util`：四 worker 的 `NewWorkerPool` 与任务等待用的 `WaitGroupWrapper`。
- `crate::stubs`：stats 容器、JSON 表、加载任务、加载 trait 与本地 `Message` 编解码。

RustCodeGraph 确认的文件内主调用边为 `BackupStats -> marshalStatsJSONTable / writeStatsFileAndClear`、`BackupStatsDone -> writeStatsFileAndClear`、`RestoreStats -> downloadStats`、`downloadStats -> downloadOneStatsFile`、`downloadOneStatsFile -> unmarshalStatsJSONTable`。

## 错误处理与边界

- JSON 序列化/反序列化、StatsFile 序列化、对象存储读写和加载器错误会转换为 `SharedError`；多数底层错误再经 `Trace` 包装。
- `BackupStats(None, ...)` 是成功的无操作；`BackupStatsDone` 在没有残余数据时也成功并返回此前已生成的索引。
- 内联条件和刷盘条件都是严格不等式：`content.len() < inlineSize` 才内联，`totalSize > maxStatsJsonTableSize` 才立即刷盘。修改时不可误改成 `<=` / `>=`。
- 远端内容解密后摘要不匹配返回 `ErrInvalidMetaFile`，错误文本包含 expected/got 的十六进制摘要；内联内容当前不带摘要，因此不会走完整性校验。
- 任一 block 缺失 rewrite 规则即返回 `ErrRestoreInvalidRewrite`，不会以旧 ID 或零值继续加载。
- stats 容器或 JSONTable 解析失败会终止该文件处理。因为 worker 间只共享“首个错误”槽，其他已经启动的 worker 可能短暂继续工作；函数最终只返回最先记录的错误。
- context 取消与接收端关闭被视为正常停止，不产生 channel 错误。相反，下载线程 panic 在 `RestoreStats` 中显式转为错误。
- `RestoreStats` 当前并非 Go `errgroup.WithContext` 的完全等价物：加载侧失败不会主动取消下载 context，且实现最终固定优先返回下载错误。这是阅读和扩展并发错误语义时必须保留或有意识修正的差异。
- `flushTemporary` 在序列化失败时也清空内存；写文件或加密失败发生在清空之后，同一 `StatsWriter` 不保留可重试 payload。这与 Go `defer clearTemporary` 的行为一致。

## 并发与资源生命周期

`StatsWriter` 本身要求 `&mut self` 执行追加和收尾，不提供多线程共享写入；其 `Storage` 通过 `Arc<dyn Storage + Send + Sync>` 共享。两个阈值是全局 `AtomicUsize`，生产代码按 `SeqCst` 读取，但多项配置之间不是事务更新；测试必须使用 `statsfile_test_support.rs::StatsConfigTestGuard` 的互斥锁同时保护它们，新增会改阈值的测试也应复用该 guard。

恢复侧有两层并发：`RestoreStats` 的下载线程与当前线程中的加载器并行，`downloadStats` 内部再以四 worker 并行处理多个 index。容量 8 的同步通道提供背压；若消费端不持续接收，下载 worker 会阻塞在 `send`。因此直接测试 `downloadStats` 时必须像现有测试一样并发排干 receiver，再等待下载线程。

`WaitGroupWrapper` 保证已派发 worker 全部退出，worker token 在每项任务后通过 `RecycleWorker` 归还。`downloadStats` 的本地 sender 在等待前释放，各 worker 的 sender clone 随闭包结束释放；最后一个 sender 消失后加载侧才观察到 EOF。`Arc<Mutex<Option<SharedError>>>` 只记录首错。每个 block 在反序列化 JSON 后调用 `take_json_table`，每次 flush 后替换整个 `StatsFile`，二者都是主动缩短大字节数组生命周期的内存措施。

## 与 Go 版本的对应关系

主要逻辑逐项对应 `br/pkg/metautil/statsfile.go`：默认阈值、九位补零文件名、首文件内联规则、严格大于阈值刷盘、明文摘要后加密、末批用首 block ID 命名、四 worker 下载、容量 8 通道、明文校验、缺失 rewrite 报错，以及发送前清空 JSON 字节的意图都保持一致。`br/pkg/metautil/statsfile_test.go::TestStatsWriter` 覆盖明文和 AES-128/192/256-CTR 下的写入/下载闭环；Rust 的 `statsfile_test.rs::test_stats_writer` 复刻该矩阵并比较完整 `JSONTable`。

已确认的实现差异和迁移限制：

- Go 使用真实 `backuppb.StatsFile` protobuf；Rust 当前 `stubs.rs::Message` 用 JSON 序列化容器，只保证本 crate 内 round-trip。
- Go `RestoreStats` 使用 `errgroup.WithContext`，任一 goroutine 失败会取消派生 context；Rust 下载线程拿到的是传入 context 的 clone，加载错误不会自动取消它。
- Go 的 `statsHandler` 是具体 statistics handle；Rust 用 `StatsReadWriter` trait 隔离依赖，便于精简 crate 与测试替身。
- Go 阈值是普通包变量；Rust 使用原子变量，并以测试互斥 guard 防止并行用例污染。
- Rust `RestoreStats` 与写入器 API 已实现并公开，但当前图索引没有 Rust 生产恢复 caller；不能把 Go 生产接线自动视为 Rust 已接线。

## 扩展指南

- 新增或调整 stats 容器字段时，应同时修改真实目标类型（未来若替换 stubs）、`JSONTable` serde 形状、`marshalStatsJSONTable` / `unmarshalStatsJSONTable` 以及 `statsfile_test.rs::test_stats_json_table_round_trip_preserves_all_fields`；不要只让默认值吞掉字段缺失。
- 调整分片或内联策略时，优先修改 `BackupStats` 与 `writeStatsFileAndClear`，保留严格边界、仅首 index 可内联、末批以首 block ID 命名等 Go 可观察契约，并同步 `parity_test.rs::stats_writer_inline_and_file_flush_matches_go`。
- 新增加密或完整性元数据时，必须成对修改写入和 `downloadOneStatsFile`，明确摘要覆盖明文还是密文，并覆盖错误密钥、损坏密文、错误摘要和内联数据的测试。目前 Rust 测试没有单独断言摘要损坏分支。
- 修改 ID 重写时，应在 `downloadOneStatsFile` 保持每个 block 独立映射，并扩充缺失映射、多个 block/分区和部分成功后的行为测试；绝不能静默沿用旧 ID。
- 修改并发、取消或错误优先级时，需同时审视 `RestoreStats` 与 `downloadStats`。若目标是完整复刻 Go `errgroup.WithContext`，应先补“加载器先失败时下载被取消”的独立回归测试，再改变 context 传播，避免通道阻塞或线程泄漏。
- 要把本模块接入 Rust 生产恢复链，应从 schema 中的 `StatsFileIndex` 与表 ID rewrite map 调用 `RestoreStats`，并提供真实 statistics loader 的 `StatsReadWriter` 实现；同时必须先解决 `stubs.rs` JSON 容器与真实 protobuf 的互操作边界。
- Rust 单元测试继续放在独立的 `statsfile_test.rs`、`parity_test.rs` 或新的独立 `*_test.rs` 中，不要内嵌到本源文件。

## 验证依据

- 目标源码：`br/pkg/metautil/statsfile.rs`，完整检查 383 行；关键符号为 `StatsWriter`、`BackupStats`、`BackupStatsDone`、`RestoreStats`、`downloadStats`、`downloadOneStatsFile`。
- crate 与模块：`br/pkg/metautil/Cargo.toml`、`br/pkg/metautil/lib.rs`、`br/pkg/metautil/metafile.rs::MetaWriter::NewStatsWriter`。
- 本地类型与编码边界：`br/pkg/metautil/stubs.rs::Message`、`parse_from_bytes`、`JSONTable`、`StatsFileIndex`、`StatsBlock`、`StatsFile`、`StatsReadWriter`。
- Go 对照：`br/pkg/metautil/statsfile.go`、`br/pkg/metautil/metafile.go::MetaWriter.NewStatsWriter`、`br/pkg/restore/snap_client/pipeline_items.go` 的 `metautil.RestoreStats` 调用。
- 独立测试：`br/pkg/metautil/statsfile_test.rs`、`br/pkg/metautil/parity_test.rs`、`br/pkg/metautil/statsfile_test_support.rs`、`br/pkg/metautil/statsfile_test.go`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/metautil` 找到目标及相邻 Go/Rust 测试；`node --file br/pkg/metautil/statsfile.rs` 读取全文件；`explore` 与 `callees` 确认写入和恢复主调用边；`query RestoreStats` 仅返回 Go/Rust 定义，未给出 Rust 生产 caller。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求文档存在且恰有十一个规定的二级标题。
