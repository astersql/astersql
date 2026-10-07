# `pkg/ingestor/ingestctrl/iterator.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate。`pkg/ingestor/ingestctrl/Cargo.toml` 将 `lib.rs` 指定为库入口，并用 `package.metadata.porting.go-package = "pkg/ingestor/ingestctrl"` 标明 Go 对照包；`pkg/ingestor/ingestctrl/lib.rs` 通过 `pub mod iterator` 公开本模块。本文件自身只使用标准库的 `Arc`、`Mutex` 和 crate 根的 `Error`、`KvPair`、`Result`，没有条件编译项，也不直接依赖 Cargo 中声明的外部 crate。

它位于本地 ingest Engine 的有序 KV 读取边界：`PebbleIter` 把拥有型 `Vec<KvPair>` 变成支持半开区间扫描的 `IngestLocalEngineIter`；`KeyAdapter` 在业务键与可排序存储键之间转换；`DupDetectIter` 在扫描时按解码后的业务键合并重复组并收集所有冲突项；`DupDBIter` 读取已编码的重复结果并对外暴露业务键。

当前生产接线并不完全等同于 Go：`pkg/ingestor/ingestctrl/engine.rs::Engine::newKVIter` 确实构造 `PebbleIter`；`pkg/ingestor/ingestctrl/engine_mgr.rs::newEngineManager` 确实选择 `DupDetectKeyAdapter` 或 `NoopKeyAdapter`，该适配器也进入 `duplicate.rs` 的重复流。但仓库搜索显示 `DupDetectIter::new` 和 `DupDBIter::new` 的直接 Rust 使用点仅在 `iterator_test.rs`，生产重复检测当前主要走 `pkg/ingestor/ingestctrl/duplicate.rs` 的 `DupKVStreamImpl`/`DupeController`，不能把 Go 的 Pebble/dupDB 接线描述成 Rust 已接线事实。

## 核心职责

1. 定义统一的前向迭代协议 `Iter`，以及增加首条、末条定位和缓冲释放能力的 `IngestLocalEngineIter`。
2. 用 `PebbleIter` 对内存 KV 快照排序、按 `[lower, upper)` 过滤并维护游标，模拟本地 Engine 所需的 Pebble 迭代行为。
3. 用 `DupDetectKeyAdapter` 生成保持字典序的存储键：先 mem-comparable 编码业务键，再追加经符号位翻转的 8 字节 row ID 和 2 字节长度；解码时剥离后缀并校验主体编码。
4. 用 `DupDetectIter` 每个业务键只向调用方产出一条，同时把出现至少两次的整组键值对写入共享 `duplicates`。
5. 用 `DupDBIter` 顺序读取编码键，解码后返回业务键并保留底层值。

本文件不负责持久化 Pebble 数据库、提交 batch、把重复项写入真实 dup DB、记录日志或决定重复冲突策略；这些是 Go 实现或 Rust 其他模块的职责。

## 主要符号

- `pub trait KeyAdapter: Send + Sync`：并发可共享的键转换边界。`Encode(&self, key, row_id)` 生成拥有型存储键，`Decode(&self, key)` 返回业务键或 `Error`。
- `NoopKeyAdapter`：原样复制键，忽略 row ID；用于未开启重复检测的路径。
- `DupDetectKeyAdapter`：重复检测键格式实现。私有辅助函数 `encode_memcomparable_bytes` 和 `decode_memcomparable_bytes` 负责每 8 字节一组、追加 marker 的可排序编码与严格校验。
- `pub trait Iter`：声明 `Valid`、`Next`、`Key`、`Value`、`Close`、`Error`。`Key`/`Value` 要求调用方先保证当前位置有效。
- `pub trait IngestLocalEngineIter: Iter`：补充 `First`、`Last`、`ReleaseBuf`，是 `Engine::newKVIter` 的动态分派返回接口。
- `PebbleIter`：拥有排序和范围过滤后的 `pairs`、可选游标 `current` 与关闭标志 `closed`。
- `DupDetectIter`：组合 `PebbleIter`、`Arc<dyn KeyAdapter>`、当前键值缓存、`Arc<Mutex<Vec<KvPair>>>` 重复收集器和首个迭代错误。
- `DupDBIter`：组合 `PebbleIter`、适配器、当前解码键缓存和错误；其 `First`/`Last` 是固有公开方法，前向方法通过 `Iter` 实现。

所有公开类型和 trait 都在模块层可见。两种 mem-comparable 辅助函数以及 `DupDetectIter::fill`、`DupDBIter::decode` 是内部实现。文件中没有常量、枚举、宏或 `cfg` 分支。

## 执行流程

键编码流程如下：

1. `DupDetectKeyAdapter::Encode` 把业务键按 8 字节分组。每组补零到 8 字节，再写入 `0xff - padding` marker；长度恰为 8 的倍数（包括空键）时，额外写一组 8 个零和 `0xf7` 终止组。
2. 在编码主体后追加 `(row_id ^ i64::MIN).to_be_bytes()`。翻转符号位后，大端字节顺序与有符号 row ID 顺序一致。
3. 最后追加大端 `8_u16`，记录 row ID 后缀长度。相同业务键因此先按 row ID 排序，不同业务键仍按 mem-comparable 主体排序。
4. `Decode` 从末尾读长度，检查键足以容纳该后缀，然后只解码前面的 mem-comparable 主体。当前实现不解析或返回 row ID。

`PebbleIter::new` 先按 `pair.key` 排序，再保留 `key >= lower && (upper 为空 || key < upper)` 的元素。构造后尚未定位；`First`/`Last` 建立游标，`Next` 只在已有游标时递增。`Close` 标记关闭、清空数据并撤销游标，此后 `Valid` 恒为假。

`DupDetectIter::new` 对非空上下界用 `adapter.Encode(bound, i64::MIN)` 编码，再构造底层 `PebbleIter`。`First` 或 `Last` 先定位底层，再由 `fill` 解码当前键并复制当前值。前向 `Next` 的步骤是：

1. 若已有错误，立即返回 `false`。
2. 保存当前对外键值作为重复组首项，然后持续推进底层迭代器。
3. 每项都解码业务键并复制值；若业务键仍等于前一个业务键，则在第一次重复时补入首项，再追加当前项。
4. 遇到下一个不同业务键时，先更新对外缓存；若形成了重复组，则一次取得 `duplicates` 锁并整组追加，然后返回 `true`。
5. 底层耗尽时，同样提交尾部重复组，再返回 `false`。唯一键从不写入 `duplicates`。

`DupDBIter` 使用同样的编码范围构造底层迭代器。`First`、`Last`、`Next` 每次定位成功后调用 `decode` 更新 `decoded_key`；`Value` 直接借用当前底层值。

## 数据与状态

`KvPair` 的键和值都由迭代器拥有。`PebbleIter::new` 会排序并过滤传入向量，代价是 `O(n log n)` 排序和原地保留；它不是数据库上的惰性游标。`Key`/`Value` 返回对内部向量或缓存的借用，`Close` 后不可再访问。未先调用 `First`/`Last` 就访问 `PebbleIter::Key`/`Value` 会因 `expect("iterator is invalid")` panic；trait 契约把有效性检查责任交给调用方。

`DupDetectIter` 的 `decoded_key` 与 `current_value` 是当前对外记录的拥有型缓存。因为 `Next` 可能一次越过多个相同业务键，必须保留首项值，直到完成该组。`duplicates` 是共享、追加式集合；同一实例反复从头扫描会再次追加重复项，代码不去重也不清空。锁只在一个重复组完成时短暂持有，不跨越解码和底层推进。

`DupDBIter` 只缓存解码键，值仍在 `inner` 中。三个实现的 `Error` 均返回借用：`PebbleIter` 永远为 `None`；两个包装器优先返回自己的解码/锁错误，否则查询底层错误。

## 依赖与调用关系

已确认的生产调用链包括：

- `pkg/ingestor/ingestctrl/engine.rs::Engine::newKVIter` → `PebbleIter::new` → `IngestLocalEngineIter` 动态分派；该链用于本地 Engine 的 `[lower, upper)` 快照扫描。
- `pkg/ingestor/ingestctrl/engine_mgr.rs::newEngineManager` → 根据 `BackendConfig::duplicate_detection` 构造 `DupDetectKeyAdapter` 或 `NoopKeyAdapter`；`getKeyAdapter` 以 `Arc<dyn KeyAdapter>` 返回共享实例。
- `pkg/ingestor/ingestctrl/local.rs` → `EngineManager::getKeyAdapter`；`pkg/ingestor/ingestctrl/duplicate.rs::NewLocalDupKVStream` 和 `DupeController` 持有该适配器并在重复数据流中调用 `Decode`。

已确认的测试链为 `iterator_test.rs::test_dup_detect_iterator` → `DupDetectIter::new`/`DupDBIter::new` → `First`/`Next`/`Key`/`Value`/`Close`，以及 `test_key_adapter_encoding` → `DupDetectKeyAdapter::{Encode, Decode}`。RustCodeGraph 的 `node` 调用轨迹还确认：`DupDetectIter::fill` 由其 `First`、`Last` 调用并下调 `Decode`、`Key`、`Value`；`DupDBIter::decode` 由其 `First`、`Last`、`Next` 调用；`decode_memcomparable_bytes` 只由 `DupDetectKeyAdapter::Decode` 调用。

图索引对多个同名 trait 方法存在误连边，例如把本文件不同实现的 `Next` 相互关联，因此本文只采用能由具体符号源码或路径搜索复核的边。当前没有生产源码直接构造 `DupDetectIter` 或 `DupDBIter` 的证据。

## 错误处理与边界

`DupDetectKeyAdapter::Decode` 对短于 2 字节、声明后缀长度大于实际长度的输入返回 `Error::InvalidData("insufficient bytes to decode duplicate-detect key")`。主体解码还拒绝 marker 表示的 padding 大于 8、padding 区含非零字节、缺少完整终止组或终止组后仍有主体字节的情况；非法 padding 使用 `"invalid mem-comparable key encoding"`。

编码边界包括空业务键、长度恰为 8 的倍数、负 row ID 和 `i64::MIN`/`i64::MAX`。额外终止组避免前缀键与完整 8 字节键排序混淆；row ID 符号位翻转保证 `-1` 的编码排在 `0` 之前。构造范围时用 `i64::MIN` 生成每个业务键的最小后缀，因此下界包含该业务键的所有 row ID，非空上界排除上界业务键。

`DupDetectIter` 首次解码失败或推进中解码失败时保存错误并停止；重复集合的 `Mutex` poisoned 时保存 `Error::Poisoned`。`Close` 只关闭内层内存迭代器，不清空共享重复集合。`DupDBIter::Next` 当前没有先检查自身既有 `error`，但一旦错误存在，`Valid` 为假；遵守 `while Valid { ...; Next(); }` 契约的调用方会停止。直接在错误后继续调用 `Next` 仍可能推进底层，这是与 Go 实现显式短路的差异。

`PebbleIter::Next` 在尚未定位时不移动并返回 `false`；空输入的 `First`/`Last` 返回 `false`。范围采用字节字典序和半开语义，空 `upper` 表示无上界，空 `lower` 是最小下界。

## 并发与资源生命周期

`KeyAdapter` 要求 `Send + Sync`，实例通常通过 `Arc<dyn KeyAdapter>` 在 EngineManager、重复控制器和迭代器间共享。适配器本身无可变状态。`DupDetectIter` 的重复输出通过 `Arc<Mutex<Vec<KvPair>>>` 共享，支持多个检测器向同一集合追加；组级加锁避免逐条锁开销，但不同迭代器之间的组追加顺序取决于调度，不构成全局排序保证。

迭代器本身没有声明 `Send`/`Sync` 上界，也不创建线程、任务或通道；可变推进需要 `&mut self`。`Close` 对 `PebbleIter` 是幂等的内存清理，包装器委托该操作。`ReleaseBuf` 在三个 Rust 路径中实际只有 `PebbleIter` 和 `DupDetectIter` 实现，且均为空操作，因为数据由 `Vec` 缓存拥有；这不同于 Go 的 `membuf.Buffer::Reset/Destroy` 生命周期。Rust 迭代器不持有文件、Pebble snapshot、batch 或日志器。

共享重复集合不会随迭代器关闭释放；其生命周期由外部 `Arc` 所有者控制。解码后的键和值在下一次成功定位时被替换，借用不能跨越需要可变借用的推进调用。

## 与 Go 版本的对应关系

直接对照为 `pkg/ingestor/ingestctrl/iterator.go`，测试对照为 `iterator_test.go`。接口层面，Rust `Iter` 对应 Go `Iter`，Rust `IngestLocalEngineIter` 对应 Go 同名接口；Rust 额外把 `First` 纳入扩展 trait，而 Go 从嵌入的 Pebble/forward iterator 获得该能力。

主要语义保持为：编码后的键按业务键和 row ID 排序；上下界先经 key adapter 转换；重复扫描每个业务键只暴露一次，却收集重复组的全部成员；重复库读取时再解码业务键。Rust 测试用 20 个唯一键、20 个二重组、10 个三重组验证最终只有 50 个对外键且收集 70 个重复记录，并验证 100,000 项规模下的唯一键与重复数。

关键实现差异如下：

- Go `pebbleIter` 包装真实 `*pebble.Iterator` 与 `*membuf.Buffer`；Rust `PebbleIter` 是排序后的内存 `Vec<KvPair>`，`ReleaseBuf` 为空，`Close` 不可能返回存储错误。
- Go `dupDetectIter` 委托 `common.DupDetector`，把重复项写入 Pebble batch，并在 `Close` 时先关闭 detector 再关闭 iterator；Rust 在 `Next` 中直接分组并追加到 `Arc<Mutex<Vec<KvPair>>>`，没有 batch、logger 或 `DupDetectOpt`。
- Go `KeyAdapter` 来自 Lightning common 且 row ID 是可变长字节；本文件 Rust trait 固定接收 `i64`，后缀固定 8 字节。因此 Go 测试中的 `mock_common_handle` 往返能力不在当前 Rust API 表达范围内。
- Go `dupDBIter` 构造真实 dup DB iterator；Rust `DupDBIter` 接收调用方已提供的 `Vec<KvPair>`。
- Go `Error` 合并底层和 detector 错误；Rust 只保存首个包装层错误并回退到底层 `Error`，而内存底层当前永不报错。

因此本文件是行为核心的内存化移植，并非 Go I/O 与资源模型的完整替换。扩展时应维持测试意图，但不能假设未实现的 Pebble、batch、通用 handle 或日志能力已经存在。

## 扩展指南

- 修改编码格式时，从 `DupDetectKeyAdapter::{Encode, Decode}` 及两个 mem-comparable 辅助函数入手，同时更新 `pkg/ingestor/ingestctrl/iterator_test.rs::test_key_adapter_encoding`，覆盖空键、8 字节边界、非法 marker/补零、截断后缀和 row ID 极值。格式会影响排序与范围，属于持久化兼容风险。
- 若需要支持 Go 的任意 row ID/common handle，不能只把 `i64` 换成字节切片；必须同时定义后缀长度、排序语义、上下界最小后缀和旧编码兼容策略，并核对所有 `KeyAdapter` 调用者。
- 修改重复组规则时，集中调整 `DupDetectIter::Next`，并同步 `test_dup_detect_iterator` 与大规模 `benchmark_dup_detect_iter`。必须保留“仅重复组写出全部成员、唯一组不写出、尾组也会 flush”的不变量。
- 若把 `DupDetectIter`/`DupDBIter` 接入生产，应先决定是否继续内存收集，还是恢复 Go 的 dup DB/batch 生命周期；同时接线 `EngineManager::getDuplicateData`、关闭错误传播和并发排序要求，不能仅新增构造调用。
- 修改 `Iter`/`IngestLocalEngineIter` 契约时，要同步 `engine.rs::Engine::newKVIter` 及其调用方；尤其应明确无效位置访问是 panic 还是可恢复错误，以及 `ReleaseBuf` 的真实保证。
- 新测试继续放在独立的 `iterator_test.rs`，不要内嵌到生产源文件；若行为要求 Go parity，也同步核对 `iterator.go` 和 `iterator_test.go`。

正确性风险集中在编码排序、范围端点、尾部重复组和 poisoned 锁；兼容风险集中在修改已编码键格式或扩大 row ID 类型；性能风险来自构造时复制/排序全部 KV、扫描时逐项解码复制，以及每个重复组临时分配 `Vec<KvPair>`。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ingestor/ingestctrl/iterator.rs` 确认目标文件已索引且有 59 个符号。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/iterator.rs --offset 1 --limit 500`：核对完整 438 行源文件、全部 trait/结构体/实现与条件编译情况。
- RustCodeGraph `query/node`：核对 `KeyAdapter`、`PebbleIter`、`DupDetectIter`、`DupDBIter`、`fill`、`decode`、`encode_memcomparable_bytes`、`decode_memcomparable_bytes` 和 `Next` 的定义及调用轨迹。精确 `callers/callees` 对同名 trait 方法部分无输出或产生歧义，本文未把歧义边作为事实。
- crate 与模块边界：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/lib.rs`。
- 直接生产证据：`pkg/ingestor/ingestctrl/engine.rs`、`pkg/ingestor/ingestctrl/engine_mgr.rs`、`pkg/ingestor/ingestctrl/local.rs`、`pkg/ingestor/ingestctrl/duplicate.rs`。
- Go 对照：`pkg/ingestor/ingestctrl/iterator.go`；测试证据：`pkg/ingestor/ingestctrl/iterator_test.rs` 与 `pkg/ingestor/ingestctrl/iterator_test.go`。
- 人工复核结论：本文件存在是为了统一本地有序读取和重复键编码/分组语义；当前实际运行链中 `PebbleIter` 与适配器已接线，而两个重复包装迭代器只由独立测试直接覆盖；安全扩展必须同步编码、范围、错误/资源契约及独立 parity 测试。
