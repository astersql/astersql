# `pkg/lightning/tikv/prop_collector.rs`

## 文件定位

本文件属于 `astersql-lightning-tikv` crate。crate 入口 `pkg/lightning/tikv/lib.rs` 以私有模块 `prop_collector` 装载它，再用 `pub use prop_collector::*` 重新导出其公开符号。`pkg/lightning/tikv/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/lightning/tikv`；本文件自身只直接依赖标准库的 `HashMap` 和 crate 内的 `TikvError`，没有条件编译项。

它位于 Lightning 本地 SST 生成链路中：`pkg/lightning/tikv/local_sst_writer.rs` 的 `write_sst` 创建 `MvccPropCollector` 与 `RangePropertiesCollector`，逐条喂入待写记录，然后把二者生成的 `tikv.*` 用户属性合入 SST property block。属性用于描述 MVCC 行/版本统计以及按键范围采样的累计大小和键数。

## 核心职责

- 用 `TablePropertyCollector` 抽象统一逐条收集（`Add`）、结束写出（`Finish`）和注册名（`Name`）三个操作；`MockCollector` 提供无副作用实现。
- `MvccPropCollector` 为固定时间戳的一批记录生成 `tikv.min_ts`、`tikv.max_ts`、行数/版本数等大端 `u64` 属性，并约每 10,000 个键生成一个 `tikv.rows_index` 锚点。
- `RangePropertiesCollector` 累计用户键与值的字节数、键数，以默认 4 MiB 或 40 Ki 个键为距离阈值生成 `tikv.range_index`。
- `put_bytes` 与 `encode` 定义属性索引的本地线格式：大端 `u64` 长度、原始键，再跟大端计数。这里不负责读取或校验已编码属性。

以上职责由 `MvccPropCollector::{Add, Finish}`、`RangePropertiesCollector::{Add, Finish}` 和 `local_sst_writer::write_sst` 的直接调用共同证明。

## 主要符号

- `InternalKey { user_key: Vec<u8> }`：只保留收集器所需用户键的轻量内部键包装；`new` 接管传入字节所有权。它不是 RocksDB 完整 internal key。
- `TablePropertyCollector`：公开 trait，所有方法都要求 `&mut self`，错误类型统一为 `TikvError`。当前三个实现是 `MockCollector`、`MvccPropCollector` 和 `RangePropertiesCollector`。
- `MockCollector::new`：保存调用方给定的名称；`Add`、`Finish` 恒返回 `Ok(())`，不会修改属性表。
- `IndexHandle { key, size, offset }`：MVCC 行索引的私有锚点。`size` 是该锚点对应区段的键数，`offset` 是全局累计键数。
- `MvccPropCollector::new` / `newMVCCPropCollector`：初始化固定 `ts` 和空计数；后者是保留 Go 命名风格的公开包装。
- `MvccPropCollector::Add`：累计一条版本记录、截去用户键末尾 8 字节的 MVCC 时间戳，并在首键或区段达到 10,000 键时保存锚点。
- `MvccPropCollector::Finish`：补齐尾区段，写入八个数值属性，按锚点键排序并编码 `tikv.rows_index`。
- `RangeOffsets`、`RangeProperty`：私有累计偏移及采样点；偏移分别记录总字节数与总键数。
- `RangePropertiesCollector::new` / `Default` / `newRangePropertiesCollector`：创建空收集器，阈值默认为 `4 * 1024 * 1024` 字节和 `40 * 1024` 键；两个阈值字段公开，允许调用方在收集前调整。
- `RangePropertiesCollector::{sizeInLastRange, keysInLastRange}`：用当前累计值减去上一锚点累计值，得到尚未封口区段的增量。
- `RangePropertiesCollector::insertNewPoint`：快照当前累计偏移并复制锚点键。
- `RangePropertiesCollector::{Add, Finish}`：累计数据并按阈值采样；结束时补齐非空尾区段并写入 `tikv.range_index`。
- `put_bytes`、`encode`：分别编码一个长度前缀字节串和完整 Range 采样点列表。

## 执行流程

1. `local_sst_writer::write_sst(path, ts, records)` 分别调用 `MvccPropCollector::new(ts)` 与 `RangePropertiesCollector::new()`。
2. 对每个 `(key, value)`，入口先构造 `InternalKey`，再依次调用两个收集器的 `Add`。因此两个索引看到相同顺序和相同记录集合。
3. MVCC `Add` 将 `rows`、当前区段大小和全局偏移各加一，取 `user_key[..len-8]` 作为行键；第一条记录以及之后每满 10,000 条记录形成锚点，形成锚点后当前区段计数归零。
4. Range `Add` 把 `user_key.len() + value.len()` 加到累计大小，把键数加一。第一条记录必成锚点；以后只要距上一锚点的大小或键数达到任一阈值，就在当前键处成锚点，随后保存当前键为尾键。
5. 入口创建共享属性字典，先调用 MVCC `Finish`，再调用 Range `Finish`。二者分别补齐未落盘的尾区段并写入不同属性名，不会互相覆盖。
6. `write_sst` 之后补充标准 RocksDB 属性，按属性名排序并生成 SST property block；因此本文件只产生用户属性内容，不负责文件 I/O 或 SST 块布局。

空输入时，MVCC `Finish` 仍写出值为零（时间戳属性除外）的数值属性和空 `tikv.rows_index`；Range `Finish` 写出空 `tikv.range_index`。

## 数据与状态

`MvccPropCollector` 的 `ts` 在构造后不变；`rows`、`current_size`、`current_offset` 单调累计（代码使用 `wrapping_add`）；`last_row` 保存最近用户键去掉 8 字节后缀后的副本；`handles` 持有待编码锚点。`Finish` 会原地排序 `handles`，所以输出按行键字节序排列，而不是盲目依赖输入顺序。

MVCC 数值属性均为 8 字节大端整数：`min_ts` 与 `max_ts` 等于构造参数，`num_rows`、`num_puts`、`num_versions` 等于 `rows`，`num_deletes` 与 `num_errors` 为 0，`max_row_versions` 为 1。每个 `rows_index` 项为 `key_len:u64 + key + size:u64 + offset:u64`。

`RangePropertiesCollector` 的 `current_offsets` 是全局累计值，`last_offsets` 是最近锚点的全局累计值，二者之差才是当前区段距离；`last_key` 用于 `Finish` 封口。每个 `range_index` 项同样编码为 `key_len:u64 + key + cumulative_size:u64 + cumulative_keys:u64`，其中后两个字段是全局累计偏移，不是区段增量。

## 依赖与调用关系

上游直接调用边由 RustCodeGraph 与源码共同确认：

- `pkg/lightning/tikv/lib.rs` 声明并重新导出本模块，且用 `#[path = "prop_collector_test.rs"]` 挂载独立测试。
- `pkg/lightning/tikv/local_sst_writer.rs::write_sst` 直接调用 `MvccPropCollector::{new, Add, Finish}`、`RangePropertiesCollector::{new, Add, Finish}` 和 `InternalKey::new`；这是当前 Rust 生产路径。
- `pkg/lightning/tikv/local_sst_writer_test.rs::TestIntegrationTest` 通过写入并读回 SST，间接确认 `tikv.num_rows` 与 `tikv.range_index` 进入属性块。

下游依赖只有 `HashMap<String, Vec<u8>>`、`Vec` 的所有权/排序/编码操作和 `TikvError` 返回契约。trait 适配实现显式转发到同名固有方法，避免 trait 方法内部递归调用。当前生产入口使用具体类型，没有通过 `dyn TablePropertyCollector` 调度；`MockCollector` 也未出现在该入口中。

## 错误处理与边界

- 所有 `Add`/`Finish` 签名返回 `Result<(), TikvError>`，但本文件当前实现路径没有主动构造 `Err`；错误类型主要保持与写入链路及 trait 的统一接口。
- `MvccPropCollector::Add` 无长度检查就执行 `user_key[..len-8]`。短于 8 字节的键会因切片边界 panic，而不是返回 `TikvError`；`prop_collector_test.rs::mvcc_collector_panics_for_key_without_timestamp_suffix` 固化了这一 Go 对齐行为。调用方必须传入含 8 字节 MVCC 后缀的编码键。
- 计数和字节累计使用 `wrapping_add`，区段差值使用 `wrapping_sub`。极端超过 `u64` 的输入不会报错而会回绕；正常 SST 尺度依赖“不溢出且累计值不倒退”的隐含前提。
- 收集器不检查键序。MVCC 锚点会在 `Finish` 排序，但 Range 锚点保持添加顺序；生产入口应继续保证记录有序。
- `Finish` 不是幂等终结操作：它没有清空尾区段计数。MVCC 对有尾区段的实例重复调用会再次追加尾锚点，测试 `mvcc_collector_repeated_finish_reappends_tail_anchor` 明确验证了这一点；Range 收集器也会在仍有区段差值时追加当前尾点。正常写入链只应调用一次。
- 编码函数仅写出数据，不实施长度上限、反序列化或格式校验；消费者必须按大端 `u64` 布局解析。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或网络资源。收集器以拥有型 `Vec`/`HashMap` 管理内存，并通过 `&mut self` 串行变更状态；类型没有内部同步，不能在多个写入线程间并发共享可变实例。生产路径在 `write_sst` 的局部作用域内创建两个收集器，遍历记录后各 `Finish` 一次，属性字节转移/复制进属性字典，随后收集器随函数作用域释放。

键和锚点都保存为自有 `Vec<u8>`，不会借用调用方缓冲区；代价是每条记录更新 `last_row`/`last_key` 时发生复制，形成锚点时还会再次复制。修改该生命周期或复用策略时必须防止后续输入覆盖已保存锚点。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/tikv/prop_collector.go`。Rust 的 `MockCollector`、`MvccPropCollector`、`IndexHandle`、`RangePropertiesCollector` 分别对应 Go 的 `mockCollector`、`mvccPropCollector`、`indexHandleKV`、`rangePropertiesCollector`；默认阈值、10,000 键 MVCC 锚点、属性名、收集器名称与大端编码布局保持一致。

主要表示差异是：Go 直接实现 Pebble 的 `sstable.TablePropertyCollector`，属性字典值为 `string`；Rust 定义本地同形 trait，使用 `InternalKey` 与 `HashMap<String, Vec<u8>>`。Go 构造器返回指针，Rust 构造器返回拥有型值。Go MVCC props 结构预留 TTL 的最大/最小过期时间，且只在存在时输出对应属性；Rust 当前没有 TTL 字段，因此不会生成 `tikv.max_expire_ts` 或 `tikv.min_expire_ts`。这属于当前移植差异，不能将 TTL 描述为已支持。

Go 与 Rust 都会在短 MVCC 键切片时 panic，也都保留 `Finish` 后的尾区段状态；独立 Rust 回归测试专门固定了这两个容易被“安全化”或“幂等化”改写的语义。Go 的 `local_sst_writer_test.go` 原计划逐属性比较 TiKV 样例 SST，但相关属性比较代码当前被注释且测试被跳过；Rust 的 `local_sst_writer_test.rs` 只对关键属性做聚焦断言，不能据此宣称所有属性与 TiKV 样例逐字节一致。

## 扩展指南

- 新增或修改 MVCC 属性时，优先改 `MvccPropCollector` 的状态、`Add` 累计规则和 `Finish` 属性映射，并同步独立文件 `pkg/lightning/tikv/prop_collector_test.rs`；若属性要进入真实 SST，还应扩展 `local_sst_writer_test.rs` 的读回断言。
- 新增 TTL 属性需要先明确与 Go 的可选指针语义、时间戳来源和“不存在时不输出”规则，不能简单写零值属性。
- 调整 Range 阈值或格式时，修改 `RangePropertiesCollector::{new, Add, Finish}` 与 `encode`，同时覆盖首键、恰达阈值、双阈值取或、空输入、尾区段和重复 `Finish`。测试仍应放在独立 `*_test.rs`，不要嵌入生产文件。
- 改动索引线格式必须同步所有消费者并验证大端 `u64` 长度及累计偏移语义；这是兼容性风险，旧 TiKV/工具可能无法解析不兼容格式。
- 优化键复制可降低大量记录下的分配成本，但必须保持锚点拥有稳定字节；改为借用或缓冲复用前要证明生命周期安全。
- 若要把 trait 接到其他 SST writer，应确认其传入的是用户键而非包含 RocksDB sequence trailer 的完整 internal key，并继续满足 MVCC 末尾 8 字节的前置条件。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件；`files --filter pkg/lightning/tikv` 确认目标 Rust/Go 源与独立测试均被索引。
- RustCodeGraph `node --file pkg/lightning/tikv/prop_collector.rs --offset 1 --limit 400`：读取目标文件完整 312 行，核对全部类型、trait、函数、impl、可见性与编码逻辑。
- RustCodeGraph `query RangePropertiesCollector --kind struct`、`query newRangePropertiesCollector --kind function`：区分 Lightning、ingestor 及 Go/Rust 同名实现，避免把别处 collector 当成本文件调用关系。
- RustCodeGraph 对 `pkg/lightning/tikv/local_sst_writer.rs` 第 200 行起的节点读取：确认 `write_sst` 的 `new -> Add -> Finish -> property block` 生产调用链。
- 已读 crate/模块边界：`pkg/lightning/tikv/Cargo.toml`、`pkg/lightning/tikv/lib.rs`。
- 已读 Go 对照：`pkg/lightning/tikv/prop_collector.go`；已读独立 Rust 测试：`pkg/lightning/tikv/prop_collector_test.rs`、`pkg/lightning/tikv/local_sst_writer_test.rs`；已读 Go 相关测试：`pkg/lightning/tikv/local_sst_writer_test.go`。
- 人工复核结论：本文件存在是为了让本地生成的 SST 携带 TiKV 可识别的 MVCC 与范围索引属性；运行时由 `write_sst` 对同一记录流串行驱动；安全扩展必须保持 MVCC 键前提、大端线格式、累计偏移、单次 `Finish` 生命周期和独立测试约束。
