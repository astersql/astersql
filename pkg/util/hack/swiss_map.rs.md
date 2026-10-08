# [`pkg/util/hack/swiss_map.rs`](swiss_map.rs)

## 文件定位

本文件属于 `astersql-util-hack` crate；crate 入口 `pkg/util/hack/lib.rs` 以 `pub mod swiss_map` 暴露该模块。它是 Rust 侧专用的安全存储层，为 `pkg/util/hack/map_abi.rs`（Go 1.25 语义）和 `pkg/util/hack/map_abi_go126.rs`（Go 1.26 语义）中的 `MemAwareMap<K, V>` 提供底层 `SwissMap<K, V>`。`pkg/util/hack/Cargo.toml` 表明该 crate 对应 Go 包 `pkg/util/hack`，本文件本身不引入外部运行时依赖，仅使用标准库。

这里保存的是 Rust 拥有的键值和一份可追踪的 Go Swiss map 分配历史，不把 Rust `HashMap`、`Vec` 或枚举内存重解释成 Go runtime 对象。文件头模块注释以及 `From<HashMap<K, V>>` 的重新插入逻辑都明确了这一边界。因此，它服务于内存计费和 Go 行为迁移，不是通用高性能 map 的透明替代品，也不是 `map_abi*.rs` 中原始指针 ABI 镜像的替身。

## 核心职责

- 用 `Slot<K, V>`、`Table<K, V>` 和 `SwissMap<K, V>` 实现八槽一组、开放寻址、二次式探测的安全键值存储（`SLOTS = 8`）。
- 维持 Go Swiss map 相关的可观察状态：元素数 `used`、目录、table 局部深度、剩余增长额度、墓碑存在性、随机种子和 `clear_seq`。
- 在 table 容量低于 `MAX_TABLE = 1024` 时倍增单表；达到上限后扩展目录并拆分 table，保留 extendible-hashing 风格的目录引用关系。
- 为删除、墓碑复用与墓碑裁剪维护探测链正确性，避免把仍被后续键依赖的墓碑错误改为空槽。
- 计算 Go ABI 口径的容量和内存大小；`size` 统计 header、目录指针、table 和 group，而 `group_size` 按 Go 槽布局规则计算 group 字节数。
- 向上层提供常用 map API、借用键查询、不可变迭代、`Default`、从标准库 `HashMap` 导入和索引操作。

## 主要符号

- `SLOTS: usize = 8`：每个探测 group 的槽数；与 Go 测试中的 Swiss map group slots 一致。
- `MAX_TABLE: usize = 1024`：单 table 拆分前的最大槽容量，对应两套 `map_abi` 的 `maxTableCapacity`。
- `Slot<K, V>`：内部三态槽。`Empty` 可终止查找，`Deleted` 保持探测链，`Full { hash, key, value }` 保存完整哈希和所有权数据。
- `Table<K, V>`：包含 `slots`、局部 `depth` 和 `growth_left`。`new` 规范化容量；`find` 查找；`vacancy` 记住首个墓碑；`place` 更新增长额度；`unchecked_insert` 用于重排已有满槽；`prune_tombstones` 只回收不再维系探测链的墓碑。
- `SwissMap<K, V>`：公开容器。`tables` 和 `directory` 表示存储拓扑，`used` 表示活跃元素数，`seed` 参与哈希，`clear_seq` 记录有效清空次数，`tombstone_possible` 是快速清空判断所需的保守标志。
- `with_capacity`：构造入口。提示不超过八时使用内联小表语义；更大提示按 7/8 负载因子计算目录和 table 容量；算术溢出时回退为空小表。
- `get`、`get_mut`、`contains_key`、`insert`、`remove`、`clear`：主要 CRUD。查询支持 `Borrow<Q>`，因此例如 `String` 键可用 `&str` 查询。
- `grow`：内部扩容入口；先做单 table 倍增，容量达到 1024 后才执行目录加倍（必要时）和左右 table 拆分。
- `iter`、`keys`、`values`、`Iter`：按 `tables`、再按槽物理顺序跳过空槽和墓碑；不承诺 Go map 的迭代随机化或稳定顺序。
- `size`：按调用者给出的 Go header/table/pointer/group 常量计算分配历史对应的字节数，清空不会缩容。
- `go_field_layout<T>`、`group_size<K, V>`：内部 Go 布局换算。字符串和字节向量按两字 header 计，大于 128 字节的字段按间接指针计，并处理对齐与尾部零大小字段。

## 执行流程

1. `SwissMap::with_capacity(hint)` 总是先建一个八槽 table。`hint <= 8` 保持 `directory` 为空；更大提示先把目标容量换算为满足 7/8 负载的槽数，再选择二次幂目录长度和 table 容量。目录项初始各指向一个独立 table。
2. 查询先由 `hash` 把每个 map 的 `seed` 写入 `DefaultHasher`，再哈希键。`table_index` 对无目录或单项目录返回零，否则取哈希高位选目录项。`Table::find` 用 `hash >> 7` 选择首组，逐组扫描八槽；遇到匹配的完整哈希和相等键即返回，某组出现 `Empty` 则确认探测链结束。
3. `insert` 先查找现有键；命中时只替换 value 并返回旧值，不改变 `used`。未命中时，`vacancy` 优先记住第一个 `Deleted`，但继续到 `Empty` 才确认可插入位置。增长额度耗尽时先尝试 `prune_tombstones`；仍无安全位置则调用 `grow` 后重试。
4. 小于 1024 槽时，`grow` 将目标 table 替换为双倍容量 table，并重新插入原有 `Full` 槽；首次离开小表时创建单项目录。达到 1024 槽后，以局部深度判断是否先把目录每项复制一份，再创建右 table，使用下一哈希高位把原元素分流，并改写应指向右 table 的目录项。
5. `remove` 找到槽后检查该槽所在 group 是否已有 `Empty`。小表或已有空槽时可直接置 `Empty` 并返还增长额度；否则置 `Deleted` 以保持探测链，并设置 `tombstone_possible`。删除最后一个元素会更换哈希种子，但保留 tables 和目录容量。
6. `clear` 在“无活跃元素且不可能有墓碑”时直接返回。否则逐槽设为 `Empty`，按小表或 7/8 负载规则重置每个 table 的 `growth_left`，清除墓碑标志，使 `clear_seq` 环绕加一并更换种子；已分配拓扑不收缩，可供重新填充和稳定计费。
7. 上层 `MemAwareMap::Init` 调用 `group_size` 得到 Go group 大小，`RealBytes` 最终以 `SwissMap::size` 计算真实模拟分配量；`Set`/`SetExt` 则通过本容器的 CRUD 和元素计数维护近似计费。

## 数据与状态

核心不变量由 `pkg/util/hack/swiss_map_test.rs::invariants` 直接检查：`iter().count() == len()`；空目录时恰有一个八槽 table；非空目录长度为二次幂；每个 table 被目录连续引用 `directory.len() >> depth` 次；每个 table 的“非空槽数 + growth_left”恰等于小表的 8 或普通表的 `capacity * 7 / 8`。每个 `Full` 槽还必须能经公开 `get` 找回同一个 value 引用。

`used` 只计算 `Full`，而 `growth_left` 把墓碑也视为已消耗容量，除非复用墓碑或安全裁剪墓碑。`directory` 保存 table 下标而非指针，因此 `tables` 扩容后不会产生悬垂引用。多个目录项可以指向同一 table；`depth` 决定应有引用次数。`tombstone_possible` 只保证“false 表示没有墓碑”，不是墓碑计数。

`seed` 是每个 map 的哈希扰动值；`set_seed` 仅供空 map 测试固定结果。`clear_seq` 只在发生实际槽清理时递增；对已经干净的空 map 再次 `clear` 不递增。当前 `Iter` 不保存或检查 `clear_seq`，因为 Rust 借用规则已阻止持有不可变迭代器时通过安全 API 可变清空。

内存计费有意描述 Go ABI，不描述 Rust 对象的实际堆占用。`capacity` 汇总所有 table 槽数；`size` 在小表路径计一次 group，在目录路径对每个实际 table 计 table header 和按八槽折算的 groups，同时为每个目录项计指针。

## 依赖与调用关系

下游仅依赖 Rust 标准库：`Borrow` 支持异型借用查询，`Hash`/`DefaultHasher` 提供键哈希，`RandomState` 只用于取得新随机种子，`Vec` 保存目录、tables 和 slots，`Index` 提供 `map[&key]` 语法。

模块装配边为 `pkg/util/hack/lib.rs -> swiss_map`。直接生产调用者集中在两个 ABI 包装：`pkg/util/hack/map_abi.rs` 和 `pkg/util/hack/map_abi_go126.rs` 都把 `SwissMap<K, V>` 存入 `MemAwareMap::M`，在 `Init` 调用 `group_size`，在 `NewMemAwareMap` 调用 `with_capacity`，并通过 `insert`、`contains_key`、`get`、`remove`、`clear`、`len`、`iter` 和 `size` 实现 Go 风格包装 API。RustCodeGraph 对该文件给出大量“used by”文件，主要是常见符号名造成的保守文件级关联；精确上游应以上述 `crate::swiss_map` 导入和调用点为准。

`SwissMap` 不位于 SQL 请求、规划或存储执行主链上；它是 `pkg/util/hack` 的基础工具，通过 `MemAwareMap` 被需要 Go map 内存计费语义的上层间接使用。Cargo 清单没有普通依赖或 feature 条件，只有测试依赖 `astersql-testkit-testsetup`；本文件唯一条件编译项是尾部 `#[cfg(test)]`，把独立的 `swiss_map_test.rs` 挂为测试模块。

## 错误处理与边界

该 API 不返回业务错误类型。正常缺失用 `Option` 表示：`get`/`get_mut`/`remove` 返回 `None`，首次 `insert` 返回 `None`，覆盖插入返回旧值。`Index<&Q>` 对缺失键以 `expect("no entry found for key")` panic，语义与标准 map 索引一致；需要处理缺失时应使用 `get`。

`set_seed` 对非空 map 断言失败，测试固定了 panic 文本。`with_capacity` 对 `hint * 8` 或目录二次幂计算溢出不 panic，而是返回初始空小表。内部 `unchecked_insert` 的 `expect("rehash has room")`、匹配槽后的 `unreachable!()` 依赖结构不变量；若触发，说明扩容或槽状态维护已损坏，而非可恢复输入错误。

删除不能一律生成空槽：当同组没有空槽时，后续键可能依赖当前探测链，必须留下 `Deleted`。`prune_tombstones` 既要求墓碑至少占总槽数 10%，又要求可安全回收的墓碑至少占 10%，并且不移动活元素。修改这些阈值或探测次序可能导致查找遗漏或偏离 Go 分配历史。

哈希行为仅要求 Rust 内部自洽，不保证与 Go runtime 产生相同哈希位序；固定 `mockSeedForTest` 主要用于可重复的 Rust 拆分/计费测试。`DefaultHasher` 和手动混入种子也不是 Go hasher 的 ABI 实现。

## 并发与资源生命周期

`SwissMap` 没有内部锁、原子或后台任务。读方法接收 `&self`，结构修改接收 `&mut self`，Rust 借用规则阻止安全代码中的并发写、边迭代边清空以及迭代引用失效。类型是否可跨线程由 `K`、`V` 和其标准容器的自动 `Send`/`Sync` 约束决定；文件不额外承诺并发 map 语义。

键和值由 `Slot::Full` 持有所有权。覆盖插入把旧值交给调用者；删除返回被移除的值；`clear` 逐槽替换并立即析构键值；map 或 clone 的最终析构由 Rust 自动完成。`borrowed_lookup_clone_and_drop_release_values_once` 使用 `Arc` 引用计数验证 clone、clear 和 drop 各只释放自己的所有权一次。

扩容通过 `std::mem::replace` 取走旧 table，再把活槽移动到新 table；墓碑和空槽在移动时被丢弃。目录与 table 的分配在 `clear` 和删空后继续保留，这既支持复用，也使 `size` 在清空前后保持已分配字节数。没有手写 `unsafe`、裸指针资源或需要调用者显式关闭的生命周期。

## 与 Go 版本的对应关系

Go 目录没有 `swiss_map.go`；Go 1.25/1.26 的真实存储就是内建 `map[K]V`，`pkg/util/hack/map_abi.go` 与 `map_abi_go126.go` 通过 `unsafe` 读取 runtime map/type/table/group 布局。Rust 无法把 `std::collections::HashMap` 安全重解释为这些对象，所以本文件是迁移时新增的安全模型，并由两个 Rust `map_abi` 版本共享。

对应关系如下：`SwissMap.used` 对应 Go runtime map 的 `Used`；`seed`、目录、局部/全局深度效果、`tombstone_possible` 和 `clear_seq` 对应 Go map 状态；`Table::growth_left` 对应 Go table 的剩余增长额度；八槽 group、7/8 负载、1024 槽拆表以及清空不释放容量与 Go 测试意图对齐。`pkg/util/hack/map_abi_test.go::TestSwissTable` 提供 group 布局、八到九元素扩容、目录大小、删空换种子、clear 序号和内存值的 Go 基准。

差异必须保留在理解中：Rust 槽是安全枚举且保存完整 `u64` 哈希，不是 Go control bytes；目录保存下标而非 table 指针；Rust `Iter` 不实现 Go 的随机迭代状态或并发修改检测；哈希采用 Rust `Hash`/`Eq`；`From<HashMap>` 会逐项重建 Go 风格存储，不能继承标准库 map 的不透明分配。`group_size` 只复现计费所需布局规则，并针对 `String`、`&str`、`Vec<u8>`、大于 128 字节字段和零大小尾字段做显式换算。

`pkg/util/hack/swiss_map_test.rs` 是本文件的直接白盒测试；`map_abi_test.rs`、`migration_aster_unit_test.rs` 和 Go `map_abi_test.go` 则从包装层交叉验证计费和 CRUD。Go 1.26 包装文件与 1.25 文件接口类似，因此本安全存储不是由 Cargo feature 选择其中一套，而是两套 Rust 模块均可直接引用。

## 扩展指南

- 新增 CRUD 或入口 API 时，优先在 `SwissMap` impl 中实现，并让两套 `MemAwareMap` 包装保持 Go 1.25/1.26 的共同语义；不要开放对 `tables`、`slots` 的可变解引用，否则会绕过 `used`、`growth_left`、墓碑和内存计费。
- 修改探测策略时必须同步审查 `Table::find`、`vacancy`、`prune_tombstones`、`unchecked_insert` 和 `grow`。首空槽终止、墓碑优先复用以及跨组探测顺序是同一不变量，不能只改一处。
- 修改扩容或负载因子时必须同步 `with_capacity`、`Table::new`、`place`、`grow`、`remove`、`clear` 和 `size`，并核对 `MAX_TABLE`、目录连续引用及清空后容量保留。性能风险包括过早拆表、墓碑累积、重复 rehash 和目录膨胀。
- 修改 Go 布局计费时从 `go_field_layout` 和 `group_size` 接入，并同时对照 Go 1.25/1.26 runtime 布局。兼容风险集中在字符串/切片 header、超过 128 字节的间接字段、混合对齐以及零大小尾字段。
- 新行为测试应继续放在独立的 `pkg/util/hack/swiss_map_test.rs`，不要内嵌到生产文件；包装层行为同步更新 `map_abi_test.rs`、`migration_aster_unit_test.rs`，必要时对照 `map_abi_test.go`。至少覆盖结构不变量、覆盖插入、借用查询、删空换种子、墓碑探测链、table 拆分、clear 重用与精确计费。
- 若需要并发访问，应在调用层增加锁或并发容器，不应在本文件暗中加入内部同步，因为这会改变泛型 trait、性能和资源生命周期。若需要 Go 式迭代清空检测，则应先明确 Rust API 如何允许这种变更；当前安全借用模型下 `clear_seq` 不参与 `Iter`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/hack` 确认 `swiss_map.rs`、独立测试及两套 ABI 文件；`node --file pkg/util/hack/swiss_map.rs` 读取完整 504 行实现；对 `SwissMap`、`group_size` 和文件路径的 `query`/`explore` 用于核对符号与调用面。宽泛 `explore` 对 `insert`、`remove` 等通用名产生大量歧义，因此调用关系另由精确模块导入核验。
- 源码：`pkg/util/hack/swiss_map.rs`，重点符号为 `Slot`、`Table::{find,vacancy,prune_tombstones}`、`SwissMap::{with_capacity,insert,grow,remove,clear,size}`、`Iter` 和 `group_size`。
- crate 与模块：`pkg/util/hack/Cargo.toml`、`pkg/util/hack/lib.rs`；前者确认 crate 边界和无普通外部依赖，后者确认公开模块及独立测试装配。
- 直接调用者：`pkg/util/hack/map_abi.rs`、`pkg/util/hack/map_abi_go126.rs`；两者均导入 `crate::swiss_map::SwissMap`，由 `MemAwareMap` 和 `NewMemAwareMap` 使用，并在初始化时调用 `group_size`。
- Rust 测试：`pkg/util/hack/swiss_map_test.rs` 覆盖容量提示、目录/table 不变量、6000 元素拆分、删除与重填、墓碑裁剪、删空换种子、非空改种子 panic、借用查询/clone/drop 和 Go 槽布局；`pkg/util/hack/map_abi_test.rs`、`pkg/util/hack/migration_aster_unit_test.rs` 覆盖包装层内存与 CRUD。
- Go 对照：`pkg/util/hack/map_abi.go`、`pkg/util/hack/map_abi_go126.go`、`pkg/util/hack/map_abi_test.go`；它们验证真实 Go runtime ABI 包装、`MemAwareMap`、八到九元素扩容以及清空和精确内存值。本目录没有同名 `swiss_map.go`，因此文档没有把 Rust 安全模型误述为 Go 同名源码复刻。
- 按任务约束，本次是纯文档分析，没有运行 Cargo；验收使用任务指定的 11 章节结构命令，并人工核对上述符号、调用边、边界和扩展位置。
