# `pkg/executor/join/concurrent_map.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-join`，由包入口 `pkg/executor/join/lib.rs:17-18` 以公开模块 `concurrent_map` 装配。它位于 Hash Join v1 的构建侧哈希表下层：`pkg/executor/join/hash_table_v1.rs:936-990` 用 `ConcurrentMap<RowPointer>` 保存哈希值到构建侧行指针链的映射，`HashRowContainer::new` 在 `concurrent` 为 `true` 时选择这一实现（`hash_table_v1.rs:1007-1012`）。

`pkg/executor/join/Cargo.toml` 将该目录定义为独立 join crate，库入口为 `lib.rs`，并用 `[package.metadata.porting] go-package = "pkg/executor/join"` 声明 Go 对照包。当前实现只使用 Rust 标准库的 `HashMap`、`Arc` 和 `RwLock`，不直接依赖 Cargo 中列出的其他 crate。

## 核心职责

- 将 `u64` 哈希键固定分散到 320 个 shard，每个 shard 独立加锁，以减少多个 Hash Join 构建 worker 插入时对单锁的争用（`SHARD_COUNT`、`ConcurrentMap::shard`）。
- 为同一哈希键保留全部构建侧值：每次插入都创建新链头，并通过 `Entry::next` 指向旧链头，而不是覆盖旧值（`ConcurrentMap::insert`）。这里保存的是“同一哈希值的候选行链”，真正的 join key 相等性仍由上层匹配逻辑判断。
- 在构建完成后按键读取链头，或逐 shard 遍历所有链头，供 `ConcurrentMapHashTable` 展开为所有 `(hash, RowPointer)`（`ConcurrentMap::get`、`ConcurrentMap::for_each`；调用接线见 `hash_table_v1.rs:954-985`）。
- 以与插入返回值相同的近似口径报告 `HashMap` 桶容量增长，供上层累计构建侧内存变化（`map_slot_bytes`、`insert`、`real_memory_bytes`）。

## 主要符号

- `SHARD_COUNT: usize = 320`（`concurrent_map.rs:111-112`）：固定 shard 数；键到 shard 的映射依赖此值，修改会改变锁粒度和键分布。
- `Entry<V>`（`concurrent_map.rs:114-121`）：公开、泛型的不可变链节点。`value` 保存一项构建侧数据，`next: Option<Arc<Entry<V>>>` 持有同键的旧链头。
- `ConcurrentMap<V>`（`concurrent_map.rs:123-127`）：公开容器，内部 `shards` 私有；每个元素为 `RwLock<HashMap<u64, Arc<Entry<V>>>>`。它实现 `Default`，语义等同于 `new`。
- `ConcurrentMap::map_slot_bytes()`（`concurrent_map.rs:136-139`）：私有常量函数，以 `size_of::<u64>() + size_of::<Arc<Entry<V>>>()` 作为每个桶槽位的估算字节数。
- `ConcurrentMap::new()`（`concurrent_map.rs:141-148`）：公开构造器，立即创建 320 个空 `HashMap` 及其锁。
- `ConcurrentMap::shard(key)`（`concurrent_map.rs:150-153`）：私有定位函数，以 `key % 320` 选择 shard。
- `ConcurrentMap::insert(key, value) -> i64`（`concurrent_map.rs:155-165`）：公开写入口，头插碰撞链，并返回本次 shard 容量扩张的估算字节数。
- `ConcurrentMap::get(key) -> Option<Arc<Entry<V>>>`（`concurrent_map.rs:167-174`）：公开读入口，克隆链头的 `Arc`，不存在时返回 `None`。
- `ConcurrentMap::for_each(visitor)`（`concurrent_map.rs:176-183`）：公开遍历入口，每个键只向回调交付链头；调用方如需所有值必须自行沿 `next` 展开。
- `ConcurrentMap::real_memory_bytes() -> i64`（`concurrent_map.rs:185-197`）：公开观测入口，汇总所有 shard 的当前 capacity 乘槽位估算值。

文件没有 trait、枚举、模块级可变状态或条件编译项。顶部 `concurrent_map.rs:22-107` 的整段注释是早期 Go 形状的迁移记录，不参与编译；真实 API 是 `concurrent_map.rs:108-198` 的泛型实现。

## 执行流程

1. `ConcurrentMap::new` 创建固定长度为 320 的 shard 数组；每个 shard 从空 `HashMap` 开始，并由独立 `RwLock` 包裹。
2. 上层 `ConcurrentMapHashTable::put` 把行位置 `RowPointer` 和已计算的哈希传给 `insert`（`hash_table_v1.rs:954-960`）。
3. `insert` 通过取模定位 shard，取得写锁，读取该键的旧链头并克隆其 `Arc`，再构造 `Entry { value, next: old }` 作为新头写回。相同键的插入因此形成后进先出的链，但不丢失旧值。
4. `insert` 比较写入前后的 shard capacity；只有 `HashMap` 扩容时才返回正的 map 槽位增量。同键替换链头通常不改变 capacity，链节点和值的内存由上层另行计入；`ConcurrentMapHashTable::put` 会额外加上一个 `RowPointer` 的大小（`hash_table_v1.rs:956-960`）。
5. 探测时，`ConcurrentMapHashTable::get` 先调用 `ConcurrentMap::get` 获取稳定的 `Arc` 链头，再逐节点克隆 `next` 并收集所有候选行（`hash_table_v1.rs:962-970`）。
6. 全表访问时，`ConcurrentMap::for_each` 按 shard 顺序持读锁，对每个键调用一次 visitor；上层 `ConcurrentMapHashTable::for_each` 再展开整条冲突链（`hash_table_v1.rs:977-985`）。

## 数据与状态

容器的持久状态只有 `shards`。键是调用方已经算出的 `u64` 哈希值，值链由 `Arc<Entry<V>>` 构成；节点创建后字段不再修改，因此读者拿到链头后可在 shard 锁释放后安全遍历。插入新头只修改对应 shard 的 `HashMap` 槽位，不修改既有节点。

同键链保留重复值，也保留不同 join key 产生相同哈希时的候选值。`concurrent_map_test.rs:26-55` 用 111 个键承载 1000 个并发插入值并逐链确认无遗漏；`hash_table_v1_test.rs:136-161` 进一步验证 6656 个行指针在并发表中保持桶内重复项、遍历总数和 memory delta 清零契约。

内存指标是有意限定的估算：`real_memory_bytes` 只计算各 `HashMap` 当前 capacity 对应的 `u64` 键和链头 `Arc` 槽位，不包含 320 个锁/HashMap 对象本身、哈希表实现额外控制字节、`Entry` 分配、`V` 的深层内存或 `Arc` 控制块。它用于和本实现历次 `insert` 返回的容量增量对账，不应被解释为进程实际占用。

## 依赖与调用关系

上游装配与调用关系为：

`lib.rs::concurrent_map` → `hash_table_v1.rs::ConcurrentMapHashTable` → `ConcurrentMap::{new, insert, get, for_each}`。

更具体地说，`ConcurrentMapHashTable::default` 构造 map；`BaseHashTable::put/get/for_each` 分别调用其插入、查找和遍历 API；`HashRowContainer::new` 的 `concurrent` 分支把这个包装器放入 `Box<dyn BaseHashTable + Send>`。RustCodeGraph 的 `node concurrent_map.rs::ConcurrentMap` 也给出 `hash_table_v1.rs` 与 `concurrent_map_test.rs` 两条导入使用边。

下游依赖均来自标准库：`HashMap` 提供分片内存储和 capacity，`RwLock` 提供 shard 级互斥，`Arc` 让链头及后继在锁外仍存活，`size_of` 提供内存估算。目标文件不执行行哈希、不比较 join key、不拥有 chunk，也不负责 spill；这些职责位于 `hash_table_v1.rs` 及更上层 Hash Join 组件。

## 错误处理与边界

- API 没有 `Result` 返回值。`RwLock::read/write` 若发现锁中毒，会通过 `expect("concurrent map poisoned")` panic（`insert`、`get`、`for_each`、`real_memory_bytes`）；当前实现没有恢复或跳过中毒 shard 的路径。
- `get` 用 `Option` 表达键是否存在；空表或未命中均为 `None`。独立测试检查 `get(111)` 未命中（`concurrent_map_test.rs:45-55`）。
- `SHARD_COUNT` 当前为非零常量，因此取模安全；若未来允许配置 shard 数，必须显式保证非零，并同步检查内存与锁竞争基准。
- `insert` 的字节计算采用 `usize` 差值后转换到 `i64`。在正常 `HashMap` 扩容中 capacity 不会因插入下降；极端大容量下的整数转换没有单独错误处理。
- visitor 在读锁内执行。若 visitor 阻塞，会延长该 shard 的读锁持有时间；若 visitor 试图向同一 map 的同一 shard 插入，可能形成无法推进的锁等待，因此回调应保持短小且不得重入写路径。
- 本层仅按哈希分桶，不校验真实 join key。哈希碰撞不是错误，上层必须对候选行继续做等值条件判断。

## 并发与资源生命周期

锁粒度是单 shard：不同 `key % 320` 的写入可并行，同一 shard 的写入串行。`insert` 在读取旧头、创建新头、替换映射和读取新 capacity 的整个阶段持写锁，确保并发插入不会丢链。`get` 虽然 Go 基线依赖“全部写完再读”而省略读锁，Rust 当前实现仍取得读锁，因此也避免了读写 `HashMap` 的数据竞争。

`for_each` 一次只持一个 shard 的读锁：回调看到该 shard 在持锁期间的一致视图，但遍历 320 个 shard 的全过程不是全局快照，先前释放的 shard 之后仍可能被写入。`real_memory_bytes` 同样逐 shard 分别读锁，其汇总值在并发写入期间不是单一时刻的全局快照。Hash Join 的预期生命周期是多 worker 完成构建后再由 probe worker 读取；若扩展为构建/探测并行，必须重新审视跨 shard 快照与业务可见性，而不能只依赖单次 `get` 的线程安全。

链节点由 `Arc` 管理：map 持有每个键的头节点，头节点递归持有后继；`get` 返回的克隆可延长整条后继链生命周期。当前没有删除或清空 API，资源在 `ConcurrentMap` 及所有外部 `Arc<Entry<V>>` 引用释放后回收。`ConcurrentMap<V>` 能跨线程共享还取决于泛型 `V` 的自动 trait 边界；编译器会要求实际值满足相应的 `Send`/`Sync` 条件。

## 与 Go 版本的对应关系

直接基线是 `pkg/executor/join/concurrent_map.go`：两边均固定 320 个 shard，以 `hashKey % 320` 定位，插入时把新节点链接到旧头，并逐 shard 加读锁遍历。`pkg/executor/join/concurrent_map_test.go` 的核心意图也已迁移：并发写入 1000 项、对 111 取模制造碰撞、逐链找回所有项，以及核对插入内存增量。

主要差异如下：

- Go 的节点是外部 `*entry`，`Insert` 原地写 `value.Next`；Rust 用泛型 `Entry<V>` 和 `Arc` 在 map 内创建不可变节点，避免裸指针所有权与锁外悬垂风险。
- Go shard 使用 `hack.MemAwareMap`，能报告其自身 `Bytes/RealBytes`，测试还固定哈希 seed；Rust 使用标准 `HashMap`，没有 seed 固定逻辑，内存只按 capacity × `(u64 + Arc)` 估算。因此 Rust 测试验证“累计 delta 等于同口径实时估算”（`concurrent_map_test.rs:58-85`），不复刻 Go 测试中的绝对字节常量 283840/376320（`concurrent_map_test.go:70-106`）。
- Go `Get` 根据构建后只读约定直接访问 map、不加 `RLock`；Rust `get` 始终取读锁。Rust 的行为更保守，但仍保持返回当前链头的功能语义。
- Go `IterCb` 使用函数类型并交付裸 `*entry`；Rust `for_each` 接受 `FnMut`，借用 `&Arc<Entry<V>>`，上层若需跨回调持有必须显式克隆。
- Rust 新增 `real_memory_bytes` 作为同口径验证接口；Go 通过逐 shard 直接读取 `MemAwareMap.RealBytes()` 完成测试，未提供同名 map 方法。

## 扩展指南

- 新增 map 操作应先决定其一致性范围。单键更新放在 `ConcurrentMap` 对应 shard 的一次锁区间内；需要全局快照的操作不能照搬 `for_each`，应设计固定锁顺序并评估同时持有 320 把锁的成本与死锁风险。
- 修改分片策略或 `SHARD_COUNT` 时，同步检查 `shard`、并发分布、构造成本和 `concurrent_map_test.rs` 中常量断言；不要改成依赖 `HashMap` 内部 hasher 的分片，否则会改变调用方传入哈希值的稳定映射。
- 修改碰撞链表示时，必须同步 `ConcurrentMapHashTable::{put,get,for_each}` 的展开逻辑，并保留重复 hash 下所有 `RowPointer`。回归测试应放在独立的 `pkg/executor/join/concurrent_map_test.rs` 或 `hash_table_v1_test.rs`，不要把测试嵌入生产源文件。
- 修改内存统计时，必须让 `insert` 返回值与 `real_memory_bytes` 使用同一口径，并同步审视上层 `ConcurrentMapHashTable::put` 额外计入的 `RowPointer`，避免重复或漏计。若目标是实际分配字节，应另行实现分配器/容器级统计，不应把当前槽位估算悄然改名为精确值。
- 增加删除/替换操作时，要处理外部 `Arc` 仍可持有旧链的语义，并定义长度与 memory delta 如何变化；当前 API 和测试只覆盖追加构建后只读的 Hash Join 生命周期。
- 若允许 visitor 重入、长时间运行或并行执行，应先把回调移出锁区或建立快照，同时评估克隆链头的内存与一致性代价。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11467 个文件、7032 个 Rust 文件；目标 `concurrent_map.rs` 被识别为 15 个符号。使用了 `status`、`files --filter pkg/executor/join`、`explore`、`query`、`node`，并对 `ConcurrentMap` 执行精确节点查询。精确 `callers/callees` 命令未返回额外明细，因此没有据此臆造调用关系。
- 生产源码：`pkg/executor/join/concurrent_map.rs:108-198`（当前实现），以及 `:22-107`（仅注释的迁移草稿）。
- crate 与模块边界：`pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs:17-18,92-94`。
- 直接 Rust 调用边：`pkg/executor/join/hash_table_v1.rs:726,936-990,1007-1012`。
- 独立 Rust 测试：`pkg/executor/join/concurrent_map_test.rs:26-85`、`pkg/executor/join/hash_table_v1_test.rs:136-161`。
- Go 对照：`pkg/executor/join/concurrent_map.go:23-95`、`pkg/executor/join/concurrent_map_test.go:26-106`。
- 本任务是纯文档分析，按任务约束未运行 Cargo，也未修改 Rust、Go、Cargo 或只读的 `plan.md`。交付前使用任务给定命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核文件定位、执行链、锁与资源生命周期、Go 差异和安全扩展点均有上述路径/符号依据。
