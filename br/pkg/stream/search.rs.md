# `br/pkg/stream/search.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-stream`（`br/pkg/stream/Cargo.toml`），由 `br/pkg/stream/lib.rs` 以 `pub mod search` 装入并通过 `pub use search::*` 扁平导出。它实现对日志备份外部存储的按键检索：从 `v1/backupmeta` 找出候选元数据，定位可能覆盖目标键和时间窗的数据文件，读取并校验文件，再解析 Default CF 与 Write CF 记录。

当前接线必须区分“库实现”和“命令实现”。Rust 库及其独立测试直接使用本文件；但 `br/cmd/br/debug.rs` 的搜索命令导入的是 `br/cmd/br/stubs.rs::stream_search`，该桩只回显搜索键，并未调用 `astersql-br-pkg-stream`。因此不能把本文件描述成当前 Rust BR CLI 已经实际执行的搜索后端。Go 的 `br/cmd/br/debug.go` 则直接调用 `br/pkg/stream/search.go` 的真实实现。

## 核心职责

- `Comparator` 与 `NewStartWithComparator` 提供可替换的键匹配策略；默认策略用字节前缀判断候选编码键是否以搜索键开头。
- `NewStreamBackupSearch` 将调用者提供的原始键转换为 TiKV memcomparable 编码，保存外部存储、比较器和可选时间上下界。
- `StreamBackupSearch::Search` 负责完整编排：列举并解析元数据、文件级粗过滤、逐文件搜索、跨文件汇集两个 CF，最后合并与排序。
- `searchFromDataFile` 负责单文件 SHA-256 校验、事件迭代、键/时间戳解码和 CF 特定值解析。
- `mergeCFEntries` 以业务键和 `start_ts` 把没有 short value 的 Write CF 记录与 Default CF 大值配对，同时保留未配对记录。
- `EncodeSearchKey` 暴露与构造器相同的搜索键编码规则，供测试和需要构造文件键的调用者复用。

它不负责创建备份、写入元数据、存储重试、取消传播或 CLI 参数解析；这些属于上游命令/存储层。`startTs`、`endTs` 只用于 `DataFileInfo` 的文件级范围裁剪，不会在迭代时再次逐条过滤时间戳。

## 主要符号

- `pub trait Comparator { fn Compare(&self, src: &[u8], dst: &[u8]) -> bool }`：搜索匹配扩展点。`src` 是文件内包含降序时间戳后缀的完整编码键，`dst` 是已 memcomparable 编码的搜索键。
- `startWithComparator`：私有、无状态的默认比较器；`Compare` 调用 `src.starts_with(dst)`。
- `NewStartWithComparator() -> Box<dyn Comparator>`：公开工厂，用动态分派返回默认比较器。
- `StreamKVInfo`：公开结果 DTO。`Key` 为解码业务键的大写十六进制，`EncodedKey` 为完整文件键的小写十六进制；值和 short value 使用 Base64；Default CF 独立记录的 `CommitTs` 为零。
- `StreamBackupSearch`：保存 `Arc<dyn Storage>`、比较器、编码后的 `searchKey` 以及两个时间边界。字段私有，时间窗通过 setter 设置。
- `NewStreamBackupSearch(...) -> StreamBackupSearch`：编码原始搜索键；默认 `startTs == 0`、`endTs == 0`，分别表示没有下界、没有上界。
- `SetStartTS` / `SetEndTs`：设置包含式文件级时间边界。
- `resolveMetaData`：私有候选文件筛选器，跳过 `IsMeta` 文件，并用键区间及 `MinTs`/`MaxTs` 判定是否可能命中。
- `Search`：公开同步入口，返回合并后的 `Vec<StreamKVInfo>` 或第一个错误。
- `searchFromDataFile`：`pub(crate)` 单文件入口；测试通过 `br/pkg/stream/export_test.rs::SearchFromDataFileForTest` 包装访问。
- `mergeCFEntries`：公开 CF 合并函数；测试包装 `MergeCFEntriesForTest` 直接验证合并数量。
- `EncodeSearchKey`：对原始字节调用 `codec::EncodeBytes`。

文件没有条件编译项、模块常量或异步函数；测试模块在 `br/pkg/stream/lib.rs` 中以独立文件、`#[cfg(test)]` 方式挂载，没有把测试内嵌到生产源文件。

## 执行流程

1. 调用者用 `NewStreamBackupSearch` 传入共享存储、比较器和原始键。构造器先执行 `codec::EncodeBytes`，使搜索键与备份文件中的键编码一致；可随后用 `SetStartTS`、`SetEndTs` 设置窗口。
2. `Search` 调用 `Storage::ListFiles("v1/backupmeta")`，只处理路径以 `.meta` 结尾的对象。每个对象经 `ReadFile` 读取后，以 `serde_json::from_slice::<Metadata>` 解析。
3. `resolveMetaData` 遍历 `Metadata.Files`。它排除元文件、搜索键落在 `[StartKey, EndKey]` 之外的文件、`MaxTs < startTs` 的过旧文件，以及 `MinTs > endTs` 的过新文件。边界相等时保留候选。
4. `Search` 串行调用 `searchFromDataFile`。后者读取 `DataFileInfo.Path`，计算 SHA-256，并要求与 `DataFileInfo::GetSha256()` 完全相等。
5. `NewEventIterator` 遍历文件事件。每轮先 `Next`，再检查迭代器错误；比较器未命中的键被跳过。命中键的最后 8 字节经 `DecodeUintDesc` 得到时间戳，剩余部分经 `DecodeBytes` 得到业务键。
6. Write CF 值由 `RawWriteCFValue::ParseFrom` 解析：键尾时间戳是 `CommitTs`，值内时间戳是 `StartTs`；存在 short value 时将其 Base64 编码。Default CF 使用键尾时间戳作为 `StartTs`，原值直接 Base64 编码。
7. 单文件先按完整编码键放入两个 `HashMap`，随后把值追加到全局原始结果；`Search` 再按 CF 重建全局两个 map。这保证位于不同文件的 Write/Default 记录能够配对，但同一完整编码键的重复记录会被后写值覆盖。
8. `mergeCFEntries` 遍历全部 Write 记录。若 `ShortValue` 为空，就从大写十六进制业务键还原原始字节，执行 `EncodeBytes(raw) || EncodeUintDesc(startTs)`，以其十六进制串查找 Default 记录；命中后复制 `Value` 并标记对应 Default 已消费。
9. 所有 Write 记录都会输出；未消费的 Default 记录也会输出。最后按 `CommitTs` 升序排序，因此 `CommitTs == 0` 的独立 Default 记录位于前部。

## 数据与状态

`StreamBackupSearch` 在构造后持有共享存储句柄与独占比较器。搜索过程不修改外部存储；除 setter 外，`Search(&self)` 只创建局部容器，因而同一实例的只读调用不共享可变搜索中间态。`Arc<dyn Storage>` 允许存储句柄跨所有者共享，但本文件没有为单次搜索创建线程或任务。

键有三种表示，需要保持一致：原始业务键是构造器输入；`searchKey` 是 `EncodeBytes` 后的比较目标；文件事件键是该编码键再拼接 `EncodeUintDesc(ts)`。结果中的 `Key` 是原始业务键的大写 hex，`EncodedKey` 是完整事件键的小写 hex。Write CF 的 `CommitTs` 来自键，`StartTs` 来自写记录值；Default CF 的 `StartTs` 来自键，`CommitTs` 留为零。

搜索的主要临时状态是候选 `Vec<DataFileInfo>`、单文件 CF map、跨文件 `raw_entries` 和全局 CF map。内存用量随候选文件数与命中记录数增长，且 `ReadFile` 会把每个正在处理的完整文件读入内存。最终合并容量预留为两类记录总数，已合并 Default 的键另存于 `HashSet`。

## 依赖与调用关系

上游方面，`br/pkg/stream/lib.rs` 导出全部公开符号；`br/pkg/stream/search_test.rs` 和 `br/pkg/stream/parity_test.rs` 构造真实搜索器并调用 `Search`，`br/pkg/stream/export_test.rs` 调用 crate 内单文件搜索与合并入口。RustCodeGraph 将本文件标为被 `export_test.rs`、`parity_test.rs` 使用；精确 `callers` 查询没有返回本文件方法的生产调用边。文本接线核验进一步确认 `br/cmd/br/debug.rs` 使用的是命令 crate 自己的 `stubs::stream_search`，所以真实库实现目前没有经该 CLI 入口接线。

下游方面：

- `crate::stubs::Storage` 提供同步 `ListFiles` 与 `ReadFile`；`Arc` 提供共享所有权。
- `crate::stubs::backuppb::{Metadata, DataFileInfo}` 描述元数据和文件范围；当前 Rust 搜索用 Serde JSON 解析 `Metadata`。
- `crate::decode_kv::{NewEventIterator, Iterator}` 解析数据文件事件流。
- `crate::meta_kv::RawWriteCFValue` 解析 Write CF 的写类型、start TS 与 short value。
- `crate::stubs::codec` 提供 memcomparable 字节编码、降序时间戳编解码。
- `astersql-br-pkg-utils-consts` 提供 `DefaultCF`、`WriteCF`；`sha2`、`base64`、`hex` 分别用于完整性、展示值和展示键编码。上述外部依赖均由 `br/pkg/stream/Cargo.toml` 声明。

## 错误处理与边界

`Search` 使用 `Result<_, Error>` 逐层快速返回：列文件、读 meta、JSON 反序列化、读数据文件、校验和、事件迭代、时间戳解码、业务键解码和 Write CF 值解析任一失败都会终止整个搜索。数据文件读取和关键解码错误包含文件路径；元数据 JSON 解析只保留底层错误文本。校验和不一致返回明确的 `validate checksum failed`，不会继续处理损坏文件。

键区间与时间窗都是候选文件的粗过滤：键的 `StartKey`、`EndKey` 使用包含边界，时间也在 `MaxTs == startTs` 或 `MinTs == endTs` 时保留。设置 `startTs > endTs` 没有显式参数校验，通常会筛掉无重叠文件，但该组合不是一个单独报错。未知 CF 会完成校验和迭代，却不产生结果。

实现假设比较器命中的事件键至少有 8 字节时间戳后缀；`&k[k.len() - 8..]` 对更短键会发生 Rust 越界 panic，而不是返回 `Error`。合并阶段若结果 `Key` 的 hex 解码失败，会静默跳过 Default 回填并仍输出 Write 条目；这与 Go 版本记录 warning 后继续的可观测性不同。空 short value 被用作“需要查 Default”的信号，因此真正的空 short value 与“不含 short value”在结果层不可区分。

## 并发与资源生命周期

当前 Rust 实现是全同步、串行流程：先全部读取/解析 meta 并积累候选文件，再逐个完整读取数据文件，最后集中合并。没有 Tokio 任务、线程池、通道、锁、超时或取消令牌；调用线程承担全部 I/O 与 CPU 工作。`Arc<dyn Storage>` 的生命周期覆盖搜索器，比较器由 `Box` 独占，局部文件缓冲和 map 在 `Search` 返回时释放。

该资源模型与 Go 不同。Go 的 `readDataFiles` 用 64 worker 处理 meta，`Search` 用 16 worker 搜索数据文件并通过 channel 汇集结果，还接受 `context.Context` 传播取消；Rust 没有这些并发和取消语义。扩展并发时必须保证所有文件搜索完成后再做全局 CF 合并，并为并发错误、通道关闭、内存峰值和确定性输出制定清晰策略。

## 与 Go 版本的对应关系

主要结构逐项对应 `br/pkg/stream/search.go`：`Comparator`、前缀比较器、`StreamKVInfo`、搜索器构造与时间 setter、元数据筛选、单文件搜索、CF 合并和按 commit TS 排序都保留了相同意图。`br/pkg/stream/search_test.rs` 对照 `search_test.go` 覆盖前缀比较、文件搜索和 Write/Default 合并；Rust 额外通过完整 `Search` 路径验证跨文件大值回填，而测试辅助仍位于独立的 `export_test.rs`。

已验证的迁移差异包括：

- Go 通过 `WalkDir(GetStreamBackupMetaPrefix())` 发现对象并用 protobuf `Metadata.Unmarshal` 解析；Rust 调用 `ListFiles("v1/backupmeta")`、严格限定 `.meta` 后缀并以 JSON 反序列化。这要求 Rust 当前存储中的 meta 确实采用其桩模型支持的 JSON 格式，不能直接推断可读取 Go 生产备份元数据。
- Go 并发读取 meta/数据文件并接受 context；Rust 串行同步执行且不可取消。
- Go `searchFromDataFile` 将每条结果直接发 channel；Rust 先放入单文件 map，因此同一文件内重复完整键会折叠。Rust 随后又在全局 map 中按完整键折叠重复项。
- Go hex 解码失败会写 warning；Rust 不记录日志。Go 单文件完成会写 info；Rust 不记录日志。
- Go CLI 已接真实 `stream.NewStreamBackupSearch`；Rust CLI 仍接命令 crate 的桩。故本文件当前是具备测试证据的库级移植，不是完整的端到端生产替代。

## 扩展指南

- 新增键匹配方式应实现 `Comparator`，并在独立测试中覆盖编码后键、时间戳后缀及边界；不要绕过构造器对原始键的 `EncodeBytes`。
- 调整时间过滤时应先决定它仍是文件级粗过滤，还是需要逐事件精确过滤；若后者，应分别依据 Write 的 commit TS、Default 的 start TS 定义语义，并补齐等于上下界及无效窗口测试。
- 接入真实 BR CLI 时，应替换 `br/cmd/br/stubs.rs::stream_search`，统一两个 crate 的 `Storage`、错误与序列化类型，并用真实 Go 兼容元数据验证；仅修改 `debug.rs` 的导入不足以证明格式兼容。
- 支持生产 protobuf meta 时，修改点在 `Search` 的 meta 解码和真实 `backuppb`/存储依赖，而不是放宽 JSON 错误。需要用 Go 生成的备份元数据做兼容测试。
- 引入并发或流式处理时，必须保留“所有文件结果汇集后再跨文件合并”的不变量；同时限制候选/结果内存并保证错误发生时工作单元能够停止。
- 强化健壮性时，应在截取时间戳前验证键长，在构造或搜索前校验 `startTs <= endTs`，并决定未知 CF、重复完整键及合并键 hex 失败应报错、告警还是保留现状。
- 任何行为修改都应同步 `br/pkg/stream/search_test.rs`；crate 级接线/编码约束可在 `br/pkg/stream/parity_test.rs` 补充。Go 对照变化则同时核验 `br/pkg/stream/search.go`、`search_test.go` 与 `export_test.go`，测试逻辑不要移入 `search.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`node --file br/pkg/stream/search.rs --offset 1/221` 读取了完整 331 行并报告测试使用点；`query StreamBackupSearch`、`query NewStreamBackupSearch` 定位 Rust/Go/命令桩同名符号。对 `NewStreamBackupSearch`、`StreamBackupSearch`、`EncodeSearchKey` 和 `Search` 的精确 `callers/callees` 未返回可用生产边，因此调用接线另以仓库文本核验。
- 生产实现：`br/pkg/stream/search.rs`、`br/pkg/stream/lib.rs`、`br/pkg/stream/Cargo.toml`、`br/pkg/stream/decode_kv.rs`、`br/pkg/stream/meta_kv.rs`、`br/pkg/stream/stubs.rs`。
- Rust 调用与测试：`br/pkg/stream/search_test.rs`、`br/pkg/stream/export_test.rs`、`br/pkg/stream/parity_test.rs`；它们验证前缀比较、SHA-256、事件解码、跨文件 CF 合并、Base64 值和未配对 Default 保留。
- Go 语义对照：`br/pkg/stream/search.go`、`br/pkg/stream/search_test.go`、`br/pkg/stream/export_test.go`；生产入口对照为 `br/cmd/br/debug.go`。
- Rust CLI 接线限制：`br/cmd/br/debug.rs` 与 `br/cmd/br/stubs.rs::stream_search`。
- 包目录没有 `br/pkg/stream/doc.go`；本次未发现需要额外读取的包级 Go contract 文件。
- 本任务只新增说明文档，不运行 Cargo。结构验证应确认文件存在且恰有十一个规定的二级标题。
