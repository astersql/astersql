# `pkg/util/extsort/external_sorter.rs`

## 文件定位

本文件是 `astersql-util-extsort` crate 的抽象边界，定义外部键值排序所需的三个公开 trait：`ExternalSorter`、`Writer` 和 `Iterator`，以及它们共用的错误类型。crate 入口 `pkg/util/extsort/lib.rs` 同时公开 `external_sorter` 与 `disk_sorter`，并重新导出两者的公开项；因此调用方通常从 extsort crate 使用这些契约，而具体的落盘、压缩和恢复行为位于 `pkg/util/extsort/disk_sorter.rs`。

`pkg/util/extsort/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/util/extsort`，运行时依赖只有 `serde`、`serde_json` 和 `tokio-util`。本文件自身只直接使用标准库错误 trait 与 `tokio_util::sync::CancellationToken`，不实现存储格式，也不拥有后台任务。

在当前应用调用链中，一个明确的生产使用者是 `pkg/lightning/duplicate/detector.rs`：`Detector` 持有 `Arc<dyn ExternalSorter>`，经 `key_adder` 创建写入器，在 `detect_inner` 中排序，再在 `get_range_bounds` 和 `pkg/lightning/duplicate/worker.rs::scan_task` 中创建独立迭代器扫描结果。因此该文件是 Lightning 重复键检测与具体磁盘外排实现之间的依赖倒置边界。

## 核心职责

- `ExternalSorter` 规定外排器的阶段协议：先由一个或多个 `Writer` 写入，关闭所有写入器后调用 `sort`，排序完成后才可创建一个或多个 `Iterator`，最后选择保留数据的 `close` 或删除数据的 `close_and_cleanup`。
- `Writer` 规定写入端的所有权和刷新语义。`put` 必须在返回前复制 `key`/`value`，因此调用方可以立即复用输入缓冲；`flush` 后写入器仍可复用，`close` 则完成刷新并释放资源。
- `Iterator` 规定类似 RocksDB 的定位和顺序扫描接口。`seek` 定位第一个 `>= key` 的条目，`next` 成功后的键必须严格大于此前键，这一约束把跨文件重复键消除纳入实现责任。
- `Error`、`Result<T>`、`JoinedError` 和 `join_errors` 统一动态错误传递，并允许迭代操作失败与清理失败同时被保留。

这些职责是接口契约，不等同于所有性质已由 trait 的类型系统强制保证。例如排序的幂等性、原子性、崩溃恢复和重复键消除均依赖具体实现；当前 `DiskSorter` 在 `pkg/util/extsort/disk_sorter.rs` 中承担这些实现责任。

## 主要符号

- `pub type Error = Box<dyn StdError + Send + Sync + 'static>`：可跨线程传递、拥有 `'static` 生命周期的动态错误。调用方可通过 `downcast_ref` 检查原始具体类型。
- `pub type Result<T>`：本 crate 外排接口的统一返回类型。
- `pub trait ExternalSorter: Send + Sync`：可在线程间共享的排序器对象。`new_writer` 和 `new_iterator` 返回装箱 trait object，使实现可隐藏具体写入器和迭代器类型；两个创建入口接收 `CancellationToken`。
- `pub trait Writer: Send`：可转移到工作线程、但不要求共享引用并发调用的写入句柄。方法为 `put`、`flush`、`close`。
- `pub trait Iterator: Send`：可转移到工作线程的游标。定位方法返回 `bool` 表示当前位置是否有效，错误通过 `error` 或 `take_error` 另行取得。
- `Iterator::take_error`：把原始错误移出迭代器；第二次调用返回 `None`。转移错误后，在再次读取前必须关闭或重新定位迭代器。该 API 是 Rust 相对 Go 接口的重要所有权补充。
- `Iterator::unsafe_key` / `unsafe_value`：借用当前内部缓冲，不进行复制；只能在 `valid()` 为真时读取，其内容可能在下一次改变位置的调用后失效。
- `pub struct JoinedError`：固定保存两个错误，顺序为主要操作错误、清理错误。`Display` 逐行格式化二者，`source()` 返回主要错误，`errors()` 暴露两项供精确检查。
- `pub(crate) fn join_errors`：包内错误合并器。两边都失败时构造 `JoinedError`；只有一边失败时原样返回该错误；都成功时返回 `None`。

本文件没有常量、条件编译项或具体排序状态字段。具体状态常量、原子状态机和文件元数据属于 `disk_sorter.rs`。

## 执行流程

典型流程如下：

1. 调用方持有 `ExternalSorter` 实例，并把取消令牌交给 `new_writer`。可建立多个互相独立的写入器并发生产键值。
2. 每个写入器通过 `put` 写入；需要时调用 `flush`，最终调用 `close`。输入切片在 `put` 返回后不再被实现持有。
3. 所有写入器关闭后，调用 `ExternalSorter::sort`。契约要求该步骤幂等、原子，并允许错误或进程退出后恢复；排序结束后禁止继续写入。
4. `is_sorted` 为真后，调用方通过 `new_iterator` 创建独立游标。排序前创建迭代器必须报错。
5. 游标以 `first`、`seek` 或 `last` 定位，先检查返回值/`valid`，再读取 `unsafe_key` 和 `unsafe_value`；通过 `next` 前进。操作返回 `false` 时，应检查 `take_error` 以区分正常耗尽与失败。
6. 调用 `Iterator::close` 释放读资源；排序器使用完后调用 `close` 保留外部数据以便恢复，或调用 `close_and_cleanup` 删除其创建的资源。

当前 `DiskSorter` 对该流程的实现证据见 `disk_sorter.rs`：`new_writer` 只允许 `WRITING` 状态；`sort` 用原子 compare-exchange 进入 `SORTING`，失败时回退为 `WRITING`；`do_sort` 原子重命名 `sorted.tmp` 为排序标记后发布 `SORTED`；`new_iterator` 只接受已排序状态。`MergingIter::next` 会同时推进所有等于当前键的子迭代器，从而保证下一键严格增大并跨文件去重。

Lightning 的实际顺序为 `Detector::key_adder` → `Writer::put/flush/close` → `Detector::detect_inner` 调用 `sort` → `get_range_bounds` 和各 `Worker::scan_task` 调用 `new_iterator`。这说明接口既服务于写入阶段，也允许检测 worker 各自拥有读游标并行扫描区间。

## 数据与状态

本文件不保存键值或排序状态；它通过 trait 将状态约束暴露给实现：

- 排序器具有“可写、排序中、已排序”的逻辑阶段。trait 文档规定排序开始后不能再创建写入器，未排序时不能创建迭代器；`DiskSorterInner::state` 以 `AtomicI32` 落实该状态机。
- `Writer` 拥有待写键值的实现侧缓冲。`DiskSorterWriter::put` 用 `to_vec` 复制两段切片；`flush_inner` 先按键排序并在单个缓冲中去重，再写出 pending 文件。
- `Iterator` 拥有当前位置和潜在错误。`valid` 只说明当前位置可读；失败信息由 `error` 借用查看，或由 `take_error` 一次性转移所有权。
- `unsafe_key`/`unsafe_value` 返回的数据属于迭代器内部。调用方若需要跨定位操作保留数据，必须自行复制；`pkg/lightning/duplicate/worker.rs::scan_iterator` 在需要保存内部键时会进行解码或复制，而不是长期保存该借用。
- `JoinedError` 保存恰好两个拥有所有权的错误，不会把错误提前字符串化。`errors()[0]` 是主要错误，`errors()[1]` 是清理错误。

接口不规定相同 key 对应哪个 value 被保留，只要求重复 key 被移除以及 `next` 后 key 严格增大。任何依赖重复项 value 选择规则的扩展，都必须先在 Go/Rust 契约和测试中明确该规则。

## 依赖与调用关系

上游直接关系：

- `pkg/lightning/duplicate/detector.rs::Detector` 持有 `Arc<dyn ExternalSorter>`；`key_adder` 调用 `new_writer`，`detect_inner` 调用 `sort`，`get_range_bounds` 调用 `new_iterator`、`first`、`last`、`take_error` 和 `close`。
- `pkg/lightning/duplicate/worker.rs::Worker::scan_task` 为每个扫描任务创建迭代器，`scan_iterator` 使用 `seek`、`valid`、`unsafe_key`、`next` 和 `take_error`，然后关闭迭代器。
- `lightning/pkg/importer/dup_detect.rs::dupDetector::run` 的 Rust 迁移层以 `Arc<dyn ExternalSorter>` 接受 ignore-rows 排序器，并另行打开磁盘排序器交给重复检测器；该证据表明该抽象位于导入重复检测路径，而非 SQL 排序执行器的通用替代品。

下游实现关系：

- `pkg/util/extsort/disk_sorter.rs::DiskSorter` 实现 `ExternalSorter`。
- `DiskSorterWriter` 实现 `Writer`；`VecIterator`、`SstIter` 和 `MergingIter` 实现本文件的 `Iterator`。
- `MergingIter` 在子迭代器定位失败时调用 `join_errors(iter.take_error(), iter.close())`，避免清理覆盖原始读取错误。

crate 边界由 `pkg/util/extsort/lib.rs` 建立；`external_sorter_test.rs` 作为独立测试模块被 `#[cfg(test)]` 接入。`Cargo.toml` 的 `tokio-util` 提供取消令牌，`serde`/`serde_json` 主要供相邻磁盘实现的元数据使用，不是本接口文件的直接依赖。

## 错误处理与边界

- 所有可能失败的创建、写入、排序和关闭操作返回 `Result`；游标移动沿用 Go 风格返回 `bool`，因此调用者必须在 `false` 后检查 `error`/`take_error`，不能把所有 `false` 都解释为正常 EOF。
- `take_error` 保留原始错误对象及其具体类型，且只可取走一次。`pkg/util/extsort/disk_sorter_test.rs::iterator_error_ownership_survives_merge_and_close` 验证错误穿过合并与关闭后仍能按具体类型向下转型，并验证第二次提取为空。
- `join_errors` 的四种组合都有确定结果：双失败合并、单失败原样传递、双成功无错误。`merging_iter_keeps_both_read_and_close_errors` 验证双失败时的顺序、格式、具体类型和 `source`。
- `unsafe_key` 与 `unsafe_value` 在无效位置调用没有安全兜底；接口明确要求先验证 `valid`。以 `DiskSorter` 的向量迭代器为例，无效位置会触发 `expect` 或索引失败，因此调用顺序属于调用者责任。
- 取消是协作式的：token 由 `new_writer`、`sort`、`new_iterator` 接收，但 trait 无法强制实现检查频率。当前磁盘实现会在创建入口、排序开始和压缩批次/记录处理中检查取消。
- `close` 与 `close_and_cleanup` 的差异是恢复语义边界。前者不应清理外部存储；后者允许删除排序器创建的目录和文件。调用方不能把两者互换。
- trait 说明要求先关闭所有 writer 再排序；接口本身没有 writer 计数或借用关系来静态保证这一点，具体实现和调用方需要共同维护该前置条件。

## 并发与资源生命周期

`ExternalSorter: Send + Sync` 允许排序器通过 `Arc` 在线程间共享；`Writer: Send` 和 `Iterator: Send` 允许每个独立句柄移动到某一工作线程。接口承诺多个 writer 和多个 iterator 可分别并发使用，但没有承诺同一个 `&mut Writer` 或 `&mut Iterator` 可被并发调用。

资源生命周期分为三层：

1. writer 生命周期结束于 `Writer::close`，其语义通常包含最后一次 flush。`external_sorter_test.rs::run_common_parallel_test` 用 10 个线程分别创建、写入并关闭 writer，随后才排序。
2. iterator 生命周期结束于 `Iterator::close`。具体 `SstIter` 在关闭底层迭代器后执行 reader-pool 的 `unref` 回调；`MergingIter` 会关闭所有已打开的子迭代器。若操作和关闭均失败，`join_errors` 避免丢失任一错误。
3. sorter 生命周期结束于 `close` 或 `close_and_cleanup`。当前 `DiskSorter::close` 是无操作，用于保留文件以便 reopen；`close_and_cleanup` 删除整个工作目录，并把目录不存在视为成功。

`DiskSorter` 的 clone 共享同一个 `Arc<DiskSorterInner>`；原子状态控制阶段切换，`Mutex` 保护 pending 文件集合，`RwLock` 保护有序文件集合。上述同步细节不在本 trait 中固定，因此新实现可以采用不同机制，但必须维持相同的可观察并发契约。

## 与 Go 版本的对应关系

`pkg/util/extsort/external_sorter.go` 是直接语义对照：Go 的 `ExternalSorter`、`Writer`、`Iterator` 方法在 Rust 中均有蛇形命名对应项，写入→排序→迭代→关闭的阶段契约、并发 writer/iterator、去重和严格递增语义一致。`external_sorter_test.go` 与 `external_sorter_test.rs` 都使用固定随机种子，覆盖单 writer、10 个并发 writer、排序前拒绝迭代、按 key 字典序输出及结果条数。

需要注意的 Rust 差异：

- Go 使用 `context.Context`，Rust 使用 `CancellationToken`；后者只表达取消信号，不携带 Go context 的任意值或 deadline API。
- Go 的 `Put` 约束是不修改也不保留传入切片；Rust 文档用“返回前复制切片”表达同一可复用缓冲目的。
- Go 的 `Error() error` 可直接返回接口值；Rust 同时提供借用的 `error()` 和取得所有权的 `take_error()`，后者解决动态错误在迭代器销毁后的生命周期与向下转型需求。
- `JoinedError`/`join_errors` 对齐 Go `errors.Join` 在“主操作失败且 close 也失败”时保留两项错误的语义，但它是固定两项的本地实现，不是通用任意数量错误组。
- Go 接口没有在类型签名中标注并发约束；Rust 通过 `Send + Sync`/`Send` 显式表达可跨线程边界。

当前 Rust 公共测试生成唯一 key，因此验证了全局排序和不丢记录，但不单独覆盖接口注释所称的重复 key 去重；跨文件去重可从 `MergingIter::next` 的实现和磁盘排序器测试获得直接证据。扩展重复值选择语义时仍应新增专门对照测试。

## 扩展指南

- 新增 `ExternalSorter` 实现时，应实现完整阶段状态机，而不是只让接口编译通过：并发 writer、排序前拒绝 iterator、排序幂等/原子、失败可重试或恢复、排序后拒绝写入、并发独立 iterator、去重、关闭与清理语义都需要落实。
- 若扩展排序或迭代语义，优先修改本文件对应 trait 和文档，再调整 `disk_sorter.rs` 实现；同时更新独立的 `external_sorter_test.rs` 公共契约测试以及实现专属的 `disk_sorter_test.rs`，不要把测试内嵌到生产源文件。
- 若增加错误 API，必须保持原始类型可检查，并明确 `error` 与 `take_error` 的状态转换。涉及操作失败加清理失败时，应复用或有意扩展 `JoinedError`，并同步双错误测试。
- 若增加取消点，应在接口说明中定义可观察行为，并覆盖写入创建、排序、迭代器创建及长扫描路径；不要假定接收 token 就自动具备取消响应。
- 若改变 `unsafe_key`/`unsafe_value` 的有效期、复制策略或移动后行为，需要同步所有基于借用缓冲的调用者，尤其是 Lightning 的 `get_range_bounds` 和 `scan_iterator`。
- 若改变重复 key 的 value 选择规则或排序比较规则，必须先核对 `external_sorter.go` 与 Go 测试，评估 Lightning 重复检测内部键编码的兼容性，并新增包含跨 writer、跨文件重复 key 的 Rust/Go 对照用例。
- 性能敏感扩展应避免把具体缓冲、文件格式或锁策略塞进 trait；抽象只固定可观察契约，批量大小、压缩并发、reader pool 等策略应继续留在实现层。

## 验证依据

- 目标源码：`pkg/util/extsort/external_sorter.rs`，RustCodeGraph `node --file ... --offset 1 --limit 260` 核对了 126 行全貌，包括三个 trait、错误别名、`JoinedError` 和 `join_errors`。
- 图查询：RustCodeGraph `status` 显示目标已在索引中；`query ExternalSorter`、`query Writer`、`query Iterator`、`query JoinedError`、`query join_errors` 用于消歧；聚焦 `explore` 与 `node` 核对了 `DiskSorter` 实现、`MergingIter` 的 `join_errors` 调用、Lightning `Detector::key_adder/detect_inner/get_range_bounds` 和 `Worker::scan_task` 调用链。常见方法名的独立 `callers/callees` 查询未产生稳定消歧结果，因此本文只采用精确文件/符号查询确认的调用关系。
- crate 与模块：`pkg/util/extsort/Cargo.toml`、`pkg/util/extsort/lib.rs`。
- Rust 实现与测试：`pkg/util/extsort/disk_sorter.rs`、`pkg/util/extsort/external_sorter_test.rs`、`pkg/util/extsort/disk_sorter_test.rs`。
- Go 对照与测试：`pkg/util/extsort/external_sorter.go`、`pkg/util/extsort/external_sorter_test.go`；另外核对了调用链中的 `lightning/pkg/importer/dup_detect.rs`。
- 人工复核结论：本文区分了接口保证、具体 `DiskSorter` 事实和尚未由公共测试单独覆盖的性质；未把接口注释误写成类型系统自动强制的能力，也未复制整段源码。
- 本任务仅新增说明文档，不修改运行时代码；按计划不运行 Cargo。结构验证命令与结果应在交付检查中记录。
