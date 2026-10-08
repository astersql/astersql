# `pkg/util/mvmap/mvmap.rs`

## 文件定位

`mvmap.rs` 是 `astersql-util-mvmap` crate 的核心实现文件，由 [`pkg/util/mvmap/lib.rs`](lib.rs) 通过 `include!("mvmap.rs")` 内联到 crate 根。crate 边界和 Go 来源由 [`pkg/util/mvmap/Cargo.toml`](Cargo.toml) 声明：package 名是 `astersql-util-mvmap`，`package.metadata.porting.go-package` 指向 `pkg/util/mvmap`，且没有第三方依赖。

它提供一个“同一 key 可以保存多个 value”的字节级哈希表，优化目标是将 key/value 和元数据分片连续存储，减少每条记录的独立分配。当前 Rust 生产接线是 [`pkg/expression/aggregation/util.rs`](../../expression/aggregation/util.rs) 中 `distinctChecker`：它以编码后的聚合参数为 key，先用 `MVMap::Get` 判重，再用 `MVMap::Put` 记录首次出现。`pkg/executor/join/Cargo.toml` 也声明了该 crate，但 `pkg/executor/join/index_lookup_join.rs` 里的 `MVMap` 用法目前只存在于注释化的 Go 对照代码，不是已运行的 Rust 调用链。

## 核心职责

- 用 `HashMap<u64, entryAddr>` 把 FNV-1 64 位哈希值映射到桶链表头，一个哈希桶可以包含同 key 多 value，也可以包含哈希冲突的不同 key。
- 用 `dataStore` 连续存放 `key || value`，用 `entryStore` 存放长度、数据地址和下一节点地址，避免为每个 key/value 对建立独立堆对象。
- `Put` 只追加不覆盖；`Get` 过滤哈希冲突后，按写入时间顺序返回目标 key 的所有 value；`Len` 统计 value 数而非不同 key 数。
- `Iterator` 跳过第 0 个哨兵 entry，按 `entryStore` 的物理追加顺序遍历所有 key/value 对，不按哈希桶或 key 分组。

## 主要符号

- `entry { addr, keyLen, valLen, next }`：桶链表节点。`addr` 指向 key 的起始字节，value 紧跟 key；`next` 指向同哈希桶的旧链头。
- `entryAddr { sliceIdx, offset }` 与 `dataAddr { sliceIdx, offset }`：分别定位 entry 分片中的元素和 data 分片中的字节。`nullEntryAddr == (0, 0)` 是链尾哨兵。
- `entryStore`：以 `Vec<Vec<entry>>` 分片追加元数据。`put` 每到 `maxEntrySliceLen`（8192）条就创建新片，`get` 按地址返回 `entry` 副本。
- `dataStore`：以 `Vec<Vec<u8>>` 分片保存原始字节。`put` 返回写入起点；`get` 先比较 key 字节再切出 value；`getEntryData` 直接按 entry 切出 key/value，供迭代器使用。
- `maxDataSliceLen` 与 `maxEntrySliceLen`：分别是 64 KiB 的 data 分片目标上限和 8192 条的 entry 分片上限。单个 `key + value` 超过 64 KiB 时，新 data 分片会以该记录的实际长度为容量。
- `MVMap { hashTable, entryStore, dataStore, length }`：公开容器类型，字段不公开。
- `NewMVMap() -> MVMap`：初始化哈希表、首个 data/entry 分片，并在 entry 的 `(0, 0)` 写入空占位，使零地址可作为 null。
- `MVMap::Put(&mut self, key, value)`：追加一条记录并将它插入对应哈希链头。
- `MVMap::Get(&self, key, values) -> Vec<&[u8]>`：将命中 value 追加到调用者给定的 `values`，然后反转整个 vector。
- `MVMap::Len(&self) -> usize`：返回累计追加的 value 数。
- `MVMap::NewIterator(&self) -> Iterator<'_>` 与 `Iterator::Next`：创建借用 map 的物理顺序迭代器；每次返回 `(Some(key), Some(value))`，耗尽后持续返回 `(None, None)`。

## 执行流程

1. `NewMVMap` 建立空 `HashMap`，为 entry/data 各建立首片，再通过 `entryStore::put(entry::default())` 占用 entry 地址 `(0, 0)`。RustCodeGraph 的 `NewMVMap` 节点报告了同名 `put` 边；结合该节点的完整函数体可确认实际构造调用是 `entryStore::put`。
2. `Put` 用 `fnv_hash64(key)` 计算桶号，查出旧链头（无桶则为 `nullEntryAddr`），再由 `dataStore::put` 一次连续写入 key 和 value。
3. `Put` 把数据地址、两段长度和旧链头组成新 `entry`，由 `entryStore::put` 追加，然后用新 entry 地址替换桶头并递增 `length`。因此同桶链是“新到旧”顺序。
4. `Get` 从桶头开始沿 `entry.next` 遍历。`dataStore::get` 会比较 entry 对应的完整 key，因此相同 FNV 哈希但字节不同的节点被跳过。
5. 命中 value 先以“新到旧”顺序追加，最后 `values.reverse()` 反转整个结果，对空 seed 即得到原始 `Put` 顺序。若输入 vector 已有 seed，seed 也会参与反转并移到末尾；这是 Go 实现的现有语义，不是普通的“保留 seed 顺序后追加”。
6. `Iterator::Next` 从首片的 entry 1 开始，当前片耗尽则转到下一片的 entry 0。它不访问 `hashTable`，所以自然保留全局物理写入顺序，到达分片尾部后返回耗尽哨兵。

## 数据与状态

`MVMap` 有三组相互约束的状态：`hashTable` 只保存每个哈希桶的链头，`entryStore` 保存不会被删除的链节点，`dataStore` 保存这些节点引用的不可变字节。当前 API 没有删除、更新或清空操作，所以地址在 map 生命期内保持逻辑稳定；外层 `Vec` 扩容只移动内层 `Vec` 控制块，不改变保存于内层分片中的字节语义。

`sliceIdx`/`sliceLen` 指向当前追加分片。data 只在“当前片非空且加入整条 key/value 会超过 64 KiB”时换片，因此一对 key/value 永远位于同一 data 分片。entry 则在当前片恰好达到 8192 条时换片。`length` 每次 `Put` 增加 1，与用户记录 entry 数相同，不包含构造时的空哨兵。

key 和 value 都在 `Put` 时复制进内部 data 分片；`Get`/`Next` 返回的是指向这些内部字节的借用切片，其生命期不超过 `MVMap` 的不可变借用。

## 依赖与调用关系

- 下游：`std::collections::HashMap` 管理哈希值到链头的映射；同目录 [`fnv.rs`](fnv.rs) 提供 `fnv_hash64`，由 `Put` 和 `Get` 使用。其余存储只依赖标准库 `Vec`。
- 已接线的 Rust 上游：`pkg/expression/aggregation/util.rs::createDistinctChecker` 调用 `NewMVMap`；`distinctChecker::Check` 调用 `Get` 和 `Put`，为 `DISTINCT` 聚合实现“首次出现”判定。RustCodeGraph 将该文件连到聚合实现和 typed hash aggregation 等上层文件。
- 尚未实体接线的 Rust 路径：`pkg/executor/join/Cargo.toml` 声明 `astersql-util-mvmap`，但 `pkg/executor/join/index_lookup_join.rs` 中 `lookup_map: mvmap::MVMap` 与 `mvmap::NewMVMap()` 均在注释化代码里。不应将这个 Cargo 依赖单独视为已运行的调用证据。
- Go 上游：`pkg/expression/aggregation/util.go` 以相同方式用于 DISTINCT 判重；`pkg/executor/join/index_lookup_join.go` 在构建 lookup join task 时创建 map，在内表键编码后 `Put`，再按外表键 `Get` 找到匹配行指针编码。

## 错误处理与边界

该 API 不返回 `Result`：缺失 key 通过空 vector 表示，迭代耗尽通过 `(None, None)` 表示，哈希冲突由 `dataStore::get` 的 key 字节比较正常过滤，而不是错误。内存分配失败、索引越界和整数溢出均没有自定义恢复路径，会遵循 Rust 标准容器和当前编译配置的 panic/分配失败行为。

需特别保留的边界语义有：

- 空 key、空 value 可按普通字节切片写入；查找命中与“未命中”由返回 vector 是否有元素区分，而不是由 value 长度区分。
- `Get` 反转整个传入 vector。调用者若不想要 seed 被反转，应像当前 Rust `distinctChecker` 一样传入空 `Vec`。
- 长度元数据是 `u32`，而入参长度是 `usize`；`dataStore::put` 与 `MVMap::Put` 使用 `as u32` 转换，没有拒绝超过 `u32::MAX` 的 key/value 或总长度。这与 Go 版本的 `uint32` 元数据形状一致，但不应将超大输入宣称为安全支持。
- `Len` 只增不减，相同 key 重复写入也会增加。容器没有去重或覆盖语义。

## 并发与资源生命周期

实现内部没有锁、原子计数、通道、后台任务或显式资源释放逻辑。`Put` 需要 `&mut self`，`Get`/`NewIterator` 需要 `&self`；安全 Rust 的借用规则会阻止在持有 `Get` 结果或 `Iterator` 对 map 的不可变借用时直接调用 `Put`。如果要跨线程共享并修改，调用者必须自行建立互斥等外部同步边界；本类型不提供并发操作的复合原子性。

所有 entry 和 data 都随 `MVMap` 所有，且只追加。map drop 时由 `Vec`/`HashMap` 自动释放全部内存；在此之前没有单条回收。`Iterator<'a>` 和返回的 `&'a [u8]` 都绑定 map 借用，不能安全地逃逸到 map drop 之后。这种设计适合任务或聚合执行期内持续积累、任务结束时整体释放的场景。

## 与 Go 版本的对应关系

Rust [`mvmap.rs`](mvmap.rs) 与 Go [`mvmap.go`](mvmap.go) 基本是结构和控制流的逐项移植：`entry`/`entryStore`/`dataStore`/两种地址类型、两个分片上限、零地址哨兵、链头插入、`Get` 最后的整体反转、value 计数和物理顺序迭代都保持一致。FNV 实现被拆在 Rust [`fnv.rs`](fnv.rs) 与 Go [`fnv.go`](fnv.go) 中。

语言层面的主要差异是：

- Go `NewMVMap` 返回 `*MVMap`，Rust 返回按值所有的 `MVMap`，由调用者决定是否再装入 `Box`/`Arc` 等容器。
- Go 用 `nil` 表示查找节点不匹配或迭代耗尽；Rust 内部 `dataStore::get` 用 `Option<&[u8]>`，迭代对外用 `(Option<&[u8]>, Option<&[u8]>)`。
- Go `Get` 可复用调用者 slice 的 backing array；Rust `Get` 接收并返回 `Vec<&[u8]>`，value 本身不复制，但 vector 的所有权会移入再移出。
- Go 注释要求只在一个 goroutine 中使用；Rust 依赖借用检查器约束普通安全访问，但仍没有内部锁或面向并发操作的 API。

Go 原测试 [`mvmap_test.go`](mvmap_test.go) 的同 key 多 value、`Len`、遍历顺序和耗尽哨兵已在独立 Rust 测试 [`mvmap_test.rs`](mvmap_test.rs) 中对齐。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步固定了缺失 key、seed 反转、重复耗尽、64 KiB data 换片和 8192 entry 换片行为。

## 扩展指南

- 修改写入布局时，应同时审查 `dataStore::put`、`entryStore::put`、`MVMap::Put` 与 `Iterator::Next`，保持“单条 key/value 不跨 data 分片”和“地址在生命期内可回读”两个不变量。同步扩展独立测试 `migration_aster_unit_test.rs`，不要把测试内联到生产文件。
- 更换哈希算法或桶结构时，必须保留 `dataStore::get` 的完整 key 校验，否则哈希冲突会被误认为相同 key。还需同步 `fnv.rs`/`fnv.go` 对照和两边哈希向量测试。
- 添加删除、覆盖或内存回收时，不能只改 `hashTable`：必须重新定义 entry/data 地址稳定性、`length` 语义、现存借用切片的有效期和迭代顺序，并评估 DISTINCT 与 lookup join 的兼容性。
- 调整 `Get` 的 seed 处理或返回顺序会改变 Go 兼容语义。先在 `migration_aster_unit_test.rs::get_matches_go_seed_reversal_semantics` 和 Go 对照测试中明确新契约，再修改 `MVMap::Get`。
- 对超大输入增加安全保护时，优先在 `dataStore::put`/`MVMap::Put` 入口使用可检查长度转换并设计可观察的错误 API；这会影响现有无 `Result` 签名，需同时检查 `distinctChecker` 和未来 lookup join 接线。
- 性能调整应使用独立 [`bench_test.rs`](bench_test.rs) 与 Go [`bench_test.go`](bench_test.go) 比较 Put/Get，并用跨 data/entry 分片的功能测试防止只优化小数据路径。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11467 个文件，其中 7032 个 Rust 文件；`files --filter pkg/util/mvmap` 列出 11 个 Rust/Go 源文件与测试文件。
- RustCodeGraph 源码节点：`node --file pkg/util/mvmap/mvmap.rs --offset 1 --limit 400` 读取全部 302 行实现；`node pkg/util/mvmap/mvmap.rs::NewMVMap` 确认构造签名和哨兵初始化，并将图中同名 `put` 边与函数体交叉核对为 `entryStore::put`；`node pkg/util/mvmap/mvmap.rs::MVMap` 确认四个状态字段。
- 上游与下游依据：RustCodeGraph 读取 `pkg/expression/aggregation/util.rs`，确认 `createDistinctChecker -> NewMVMap` 和 `distinctChecker::Check -> Get/Put`；文本搜索确认 `pkg/executor/join/index_lookup_join.rs` 中只有注释化用法，并在 Go `index_lookup_join.go` 中找到实际 `NewMVMap`/`Put`/`Get` 接线。
- crate 与模块证据：读取 `pkg/util/mvmap/Cargo.toml` 和 `lib.rs`，确认 crate 名、Go 对照路径、`include!` 装配和三个独立 Rust 测试模块；该目录没有 `doc.go`。
- 语义与边界证据：逐项对照 `mvmap.go`、`mvmap_test.go`、`mvmap_test.rs` 和 `migration_aster_unit_test.rs`，确认多 value 顺序、value 计数、seed 整体反转、迭代耗尽以及 data/entry 跨片边界。
- RustCodeGraph 的自然语言 `explore` 和批量 `callers/callees` 在 30 秒窗口内未返回结果；因此调用关系用可成功返回的精确 `node` 轨迹、已索引上游文件和 `rg` 精确引用搜索交叉核验，未将超时查询解读为“无调用者”。
