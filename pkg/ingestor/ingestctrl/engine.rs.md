# [`pkg/ingestor/ingestctrl/engine.rs`](./engine.rs)

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 根在 `pkg/ingestor/ingestctrl/lib.rs`，由其中的 `pub mod engine` 暴露。`Cargo.toml` 用 `[lib] path = "lib.rs"` 确认 crate 边界，并以 `package.metadata.porting.go-package = "pkg/ingestor/ingestctrl"` 指明 Go 对照包。

它实现本地导入控制面的核心内存 Engine：接收有序或待排序的 KV，维护导入互斥状态、键范围、Region 分裂键、重复数据与导入统计，并向 `engine_mgr.rs`、`local.rs` 和 `import_pipeline.rs` 提供具体能力。当前 Rust 实现的实际存储是 `RwLock<BTreeMap<Vec<u8>, Vec<u8>>>`，不是 Go `engine.go` 使用的 Pebble DB/SST 后台管线；因此这里是可运行的内存实现和迁移边界，而不是 Go 文件全部磁盘语义的等价复刻。

## 核心职责

- `Engine` 保存按 key 排序的 KV 和原子统计，拒绝关闭后的写入，并为导入流程提供快照、范围、分裂键及文件大小视图。
- `Writer` 在调用方给定的批大小内缓存 `KvPair`，满批自动 `Flush`，最终 `Close` 后禁止追加。
- `RangePropertiesCollector`、`encodeRangeProperties`、`decodeRangeProperties` 和 `SizeProperties::add_all` 实现 Go Pebble table property 的内存对应算法：按累计字节数/键数采样、二进制编解码并将多个累计采样序列差分合并。
- `state` 及 `tryRLock`、`lockUnless`、`unlock` 表达打开、导入、关闭和共享读取之间的状态机；`EngineManager` 依赖该状态机协调 flush、import、reset 与 cleanup。
- `nextKey` 生成半开区间上界，特别处理 TiDB 固定 19 字节整数 handle 行键，避免旧 TiKV 截断行键时选到错误边界。

## 主要符号

- `ENGINE_META_KEY = [0, 'm', 'e', 't', 'a']` 是保留元数据键；`Put` 禁止用户写入它，范围属性采集和解码也跳过它。`NORMAL_ITER_START_KEY` 保留了 Go 正常迭代起点常量，但本文件当前 `newKVIter` 没有使用它过滤数据。
- `ImportMutexState` 与四个状态常量：`IMPORT=1`、`CLOSE=2`、`READ_LOCK=4`、`OPEN=8`。`isStateLocked` 仅把 `IMPORT`/`CLOSE` 视为阻止读锁的独占态。
- `EngineMeta { ts, length, total_size }` 使用原子量保存 TSO、KV 数量和未压缩的 key+value 总字节数。
- `RangeOffsets`、`RangeProperty`、`RangeProperties` 描述某个 key 位置的累计字节/键数；字段 `Size`、`Keys`、`Key` 沿用 Go 命名。
- `RangePropertiesCollector::{new, Add, Finish}` 在第一条 KV、或距上个采样点达到任一阈值时记录累计点；`Finish` 为尚未覆盖的尾段补最后一个点。
- `SizeProperties::add_all` 把单个 SST 风格的累计 offset 转为相邻点增量，按 key 合并到 `BTreeMap`，并将末点累计大小加入 `total_size`。
- `Engine::new` 初始化空引擎；核心字段包括有序 `data`、重复数据缓冲 `duplicate_data`、分裂阈值与缓存、状态原子量、串行写锁 `operation_lock`、导入/待处理/内存统计和 `first_error`。
- `Engine::{Put, finishWrite, Close, Cleanup, Exist}` 负责写入和生命周期；`setError` 只保留第一个错误。
- `Engine::{GetFirstAndLastKey, GetKeyRange, GetRegionSplitKeys, newKVIter, snapshot}` 提供导入所需的数据读取视图。
- `Engine::{KVStatistics, ImportedStatistics, FinishImport, ConflictInfo, TotalMemorySize, getEngineFileSize}` 提供监控和导入一致性检查数据。
- `Writer::{new, Append, Flush, Close, EstimatedSize}` 是批量写接口。
- `nextKey` 和 `isStateLocked` 是模块级辅助函数。

## 执行流程

1. `EngineManager::openEngine`（`engine_mgr.rs`）创建目录和 `Engine::new`，以 `IMPORT_MUTEX_STATE_OPEN` 暂时独占引擎，分配 TSO 后 `unlock` 并注册 `Arc<Engine>`。
2. `EngineManager::localWriter` 创建 `Writer`。`Writer::Append` 取得 key/value 所有权并压入 `batch`；达到 `batch_size.max(1)` 后调用 `Flush`。`Flush` 对每个元素调用 `Engine::Put`。
3. `Put` 先以 Acquire 读取 `closed`，拒绝关闭状态及空键/保留 meta 键，再通过 `operation_lock` 串行化复合更新，通过 `data.write()` 插入或覆盖。新 key 增加 `length`；新旧记录的 `key.len()+value.len()` 差值同时更新 `total_size` 和 `memory_size`。
4. `LocalBackend::ImportEngine`（`local.rs`）通过 `EngineManager::lockEngine(...IMPORT...)` 获得独占状态，调用 `finishWrite`。后者幂等地置 `closed` 和 `CLOSE`，然后返回此前 `setError` 保存的首错。
5. 导入路径调用 `GetRegionSplitKeys`：按 `BTreeMap` 顺序累积每条 KV 的字节和数量，首 key 成为第一个边界；达到任一阈值时加入 `nextKey(current_key)` 并清零当前段计数；最后确保 `nextKey(last_key)` 是终点，同时更新缓存。相邻边界在 `local.rs` 中转换成半开区间任务。
6. `GetKeyRange` 用 `GetFirstAndLastKey([], [])` 取得全局首末 key，并以 `nextKey(last)` 生成 `[start,end)`；`LocalEngineSource`（`import_pipeline.rs`）通过 `snapshot` 克隆全部 KV，连同 `engine_meta.ts` 交给导入管线。
7. 导入完成后 `LocalBackend::ImportEngine` 调用 `FinishImport(bytes,count)`，再比较 KV 与导入统计；无论成功失败，最后都 `unlock`。
8. 关闭/清理由 `EngineManager::closeEngine`、`resetEngine`、`cleanupEngine` 驱动；`Cleanup` 删除 `<id>`、`<id>.dupdetect` 和 `<id>.dupresult` 三个目录。

## 数据与状态

`data` 的 `BTreeMap` 决定快照、迭代器和范围扫描均按字节序排列。`Put` 覆盖已有 key 时不增加 `length`，只按旧值与新值的总记录大小之差调整字节统计；因此 `KVStatistics` 与 `snapshot` 应保持同一逻辑视图。`newKVIter` 和 `snapshot` 都先完整克隆当前 map，返回的是调用时快照，后续写入不会改变既有迭代结果，但大 Engine 会产生与数据量成正比的额外内存和复制成本。

互斥状态编码为一个 `AtomicU32`：共享读持有者每次增加 4，`IMPORT`/`CLOSE`/`OPEN` 使用低位独占值。`tryRLock` 只做一次 CAS，状态竞争时允许返回失败；`lockUnless` 先检查 ignore mask，随后自旋/yield，直到能把状态从 0 改成目标状态。调用者必须严格配对 `tryRLock`/`rUnlock` 或 `lockUnless`/`unlock`，否则可能下溢、永久自旋或错误清除他人的状态。

`first_error` 由 `Mutex<Option<Error>>` 保护且只写首错；`finishWrite` 首次调用检查并返回它，之后因为 `closed.swap(true)` 已为真而直接成功。`region_split_keys_cache` 每次计算后覆盖，但当前没有公开读取者。`pending_file_size` 当前文件内无累加入口，只参与 `getEngineFileSize().DiskSize`；这也是与 Go SST 实现的迁移差异。

## 依赖与调用关系

直接内部依赖是 `iterator.rs` 的 `IngestLocalEngineIter`/`PebbleIter`，以及 crate 根定义的 `EngineId`、`KvPair`、`KeyRange`、`ConflictInfo`、`EngineFileSize`、`Error` 和 `Result`。标准库依赖涵盖 `BTreeMap`、文件系统、路径、原子量、`Arc`、`Mutex` 与 `RwLock`；本文件没有直接使用 `Cargo.toml` 中的外部 crate。

主要上游调用边（经 RustCodeGraph 文件使用关系和源码搜索核对）如下：

- `engine_mgr.rs` → `Engine::new`、锁方法、`finishWrite`/`Close`/`Cleanup`、`Writer::new`、统计与大小接口；这是对象生命周期的所有者。
- `local.rs::ImportEngine` → `finishWrite`、`GetRegionSplitKeys`、`GetKeyRange`、`FinishImport` 和统计接口；这是本地 Engine 进入 Region 导入主链的入口。
- `import_pipeline.rs::{LocalEngineAdapter, LocalEngineSource}` → API 统计、范围、分裂键、关闭及 `snapshot`；它把具体 Engine 适配为 `astersql-ingestor-engineapi` 抽象。
- `engine_test.rs` 直接验证互斥、关闭写入、范围、迭代快照、Writer 批处理和 key 所有权；`local_test.rs` 另有 `nextKey` 整数行键的可执行测试。

主要下游边是 `Engine::newKVIter` → `PebbleIter::new`，`Writer::Flush` → `Engine::Put`，`GetKeyRange`/`GetRegionSplitKeys` → `nextKey`，以及 `Cleanup` → `fs::remove_dir_all`。

## 错误处理与边界

- `decodeRangeProperties` 对不足 4 字节的长度头、长度加法溢出、或不足 `key_len+16` 的 payload 返回 `Error::InvalidData`；整数读取前均已完成边界检查。编码格式是大端 `u32 key_len + key + u64 size + u64 keys`，没有版本号或校验和。
- `Put` 对关闭引擎返回 `Error::Closed`，对空键和 meta 键返回 `Error::InvalidArgument`；锁中毒统一映射为 `Error::Poisoned`。
- `Cleanup` 仅删除存在的路径，首个 IO 错误立即返回，后续目录不会继续清理；`Exist` 只检查主 `<data_dir>/<id>` 路径是否存在，不验证其类型或内容。
- 空范围的 `GetFirstAndLastKey` 返回两个空向量；空 Engine 的 `GetKeyRange` 也因此返回空边界。`GetRegionSplitKeys` 对空 Engine 返回空列表。调用方不得假设至少存在两个分裂边界。
- 分裂阈值以 `max(1)` 归一化，避免零或负配置导致无效阈值。累计采用 `u64` 普通加法；极端超过 `u64::MAX` 的数据量会受 Rust 构建模式的溢出行为影响。
- `Writer::Flush` 使用 `drain(..)`；如果中途某个 `Put` 失败，该错误会返回，但尚未处理的 drain 元素随迭代器销毁而丢弃。扩展重试语义时必须先明确是否应保留失败项和尾项。
- `Writer::Close` 只在 `Flush` 成功后置 `closed=true`；失败时仍可再次操作。`Engine::Close` 则无条件置位且本身幂等。
- `nextKey([])` 返回空；普通 key 通过追加 `0` 获得最小前缀后继。固定格式整数行键将 handle 加一；handle 为 `i64::MAX` 时返回下一 table 的前缀。

## 并发与资源生命周期

`Engine` 预期由 `Arc` 共享。单个 `Put` 同时受 `operation_lock` 和 `data` 写锁保护，使 map 与两个统计原子量作为一个复合更新发生；读取方法只拿 `data` 读锁或原子快照，因此不同统计字段之间不保证跨字段的瞬时一致读，但写入复合更新不会互相交错。

导入状态机独立于 `data` 的 `RwLock`：它协调高层操作，而不自动阻止已有调用者直接读取 `data`。`lockUnless` 是无超时的协作式自旋，等待读锁释放期间不断 `yield_now`；高竞争或读锁泄漏会造成饥饿/永久等待。`finishWrite` 与 `Close` 通过 Release/AcqRel 发布关闭状态，`Put` 用 Acquire 观察；然而 `Put` 的关闭检查发生在取得 `operation_lock` 前，已经越过检查的并发写可能在关闭置位后完成，调用方应先通过高层状态锁排空写入者。

迭代器和 `snapshot` 拥有 KV 克隆，不借用 Engine 锁；释放迭代器不会影响先前复制出的 key/value。`duplicate_store` 返回同一个 `Arc<Mutex<Vec<KvPair>>>`，所有使用者共享其生命周期和锁毒化风险。目录资源没有 RAII 自动删除；只有显式 `Cleanup` 才删除三个目录。

## 与 Go 版本的对应关系

`engine.go` 是语义来源，但两者当前并非完整等价：

- 名称与状态位、`engineMeta` 统计、范围属性编码/差分、首末 key、`nextKey`、导入统计和清理后缀基本一一对应。
- Go `Engine` 持有 Pebble DB、SST 目录、writer 注册表、context/cancel、channel、wait group、后台 compaction/ingest、key adapter、重复检测 DB 和 logger；Rust `Engine` 以进程内 `BTreeMap` 和同步克隆替代这些组件，没有 WAL、SST table properties、后台 goroutine/channel 或崩溃恢复。
- Go `finishWrite(ctx)` 会 flush 所有 writer、关闭 SST channel、等待后台任务并返回 `OnceError`；Rust `finishWrite()` 只切换关闭状态并返回 `first_error`。
- Go `GetRegionSplitKeys` 从 Pebble SST property 聚合 `sizeProperties` 后调用范围切分逻辑；Rust 直接遍历内存 KV 并按阈值切段。Rust 中范围属性工具保留了 Go 算法，但当前没有接入 `GetRegionSplitKeys`。
- Go `TotalMemorySize`/`getEngineFileSize` 遍历活跃 writer、Pebble metrics 与 pending SST；Rust 维护写入后数据大小，并将其同时作为磁盘/内存近似值，因此监控口径不同。
- Go `Writer` 支持 sorted/unsorted rows、membuf、SST writer、flush 状态与本地 writer 注册；Rust `Writer` 只是拥有 `Vec<KvPair>` 的同步批处理器。Rust 取得 `Vec` 所有权，避免 Go 可复用 key buffer 的别名问题，`engine_test.rs::sorted_batches_own_keys_after_source_buffer_reuse` 固化了这一点。
- Rust `ConflictInfo` 实际统计 `duplicate_data`；Go 当前 `ConflictInfo` 返回空值。迁移时应先确认上层期望，而不是机械覆盖任一侧。

## 扩展指南

- 新增存储行为时优先修改 `Engine::{Put, snapshot, newKVIter, GetFirstAndLastKey}` 并同步 `engine_test.rs`；必须维持 map、`length`、`total_size`、`memory_size` 的覆盖写不变量。
- 改变关闭/导入互斥时同时审查 `tryRLock`、`rUnlock`、`lockUnless`、`unlock`、`finishWrite`，以及 `engine_mgr.rs` 的所有配对调用和 `local.rs::ImportEngine` 的失败解锁路径。至少扩展 `engine_test.rs::lock_unless_waits_for_a_non_ignored_read_lock`，覆盖竞争、ignore mask 与异常返回。
- 改变 Region 分裂策略时修改 `GetRegionSplitKeys`/`nextKey`，并同步 `local_test.rs::next_key_advances_integer_record_keys_and_handles_overflow`；还应补充空 Engine、阈值边界、尾段、整数 handle 最大值及普通前缀 key 的独立测试。
- 若接入 `RangePropertiesCollector`，必须保持大端 wire 格式与 meta-key 过滤兼容，并为截断输入、多个 property 集合的相同 key 合并、阈值为零和累计溢出增加独立 Rust 测试；不要把测试嵌入生产文件。
- 若追求 Go Pebble/SST 等价，应在任务范围内逐项移植磁盘、flush、后台错误传播和恢复语义，不能把现有 `BTreeMap` 行为描述成已具备持久化能力。新增外部 Rust 依赖还必须遵守仓库关于独立上游仓库、tag 和可复现 Git 依赖的规则。
- 性能风险集中在全量克隆 (`snapshot`/`newKVIter`)、自旋独占锁和按 KV 逐次 `Put`；兼容风险集中在范围边界、统计口径、关闭竞态和 Go/Rust 错误语义。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ingestor/ingestctrl` 列出目标及同目录 Go/Rust 文件。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/engine.rs --offset 1 --limit 500` 与 `--offset 500 --limit 200`：读取目标全部 639 行，并报告直接使用者包括 `engine_mgr.rs`、`engine_test.rs`、`import_pipeline.rs`、`local.rs`、`local_test.rs` 等 6 个文件。
- RustCodeGraph `query GetRegionSplitKeys --json` 定位本文件第 476 行实现及 `engine_mgr.rs`/`import_pipeline.rs` 的同名接口；`query RangePropertiesCollector --json` 对齐 Go `engine.go` 第 406 行与 Rust 第 129 行。对精确 Rust 方法运行 `callers`/`callees` 未返回边，因此调用关系又由上述直接使用文件的源码搜索核实，未把缺失图边当作不存在调用。
- 已读源码/配置：`pkg/ingestor/ingestctrl/{engine.rs,Cargo.toml,lib.rs,engine_mgr.rs,local.rs,import_pipeline.rs,iterator.rs}`；Go 对照为 `engine.go`；独立测试为 `engine_test.rs`、`engine_test.go` 和 `local_test.rs` 中 `nextKey`/范围属性相关段落。
- `engine_test.rs` 的可执行测试覆盖 import 状态大小视图、非忽略读锁等待、关闭后写入错误、范围扫描、迭代数据独立性、默认最小批大小和复用源 buffer 后 key 所有权；`local_test.rs` 的可执行 `next_key_advances_integer_record_keys_and_handles_overflow` 覆盖整数 handle 与溢出。其余仅保留 Go 步骤的注释占位不能作为 Rust 行为通过证据，本文未据此声称已验证。
- 本任务只新增说明文档，按计划不运行 Cargo；交付时执行任务规定的 11 章节结构命令，并人工复核唯一生产物、源码链接、事实边界和 Go/Rust 差异。
