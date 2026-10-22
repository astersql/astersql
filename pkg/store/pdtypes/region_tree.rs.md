# `pkg/store/pdtypes/region_tree.rs`

## 文件定位

私有函数 `overlap` 集中定义区间相交规则：相邻但不相交的半开区间不算重叠，空 `end_key` 表示延伸到键空间正无穷。

## 核心职责

- `Region` 把一份 `kvproto::metapb::Region` 元数据和可选 Leader 组合成便于测试构造、克隆和比较的值对象。
- `RegionTree::SetRegion` 用新区间替换所有与它相交的旧 Region。这里的“替换”是批量删除全部重叠项后追加新区间，不按 Region ID 或 epoch 做冲突裁决。
- `RegionTree::ScanRange` 将内部集合按 `Meta.start_key` 原地排序，然后返回与查询半开区间相交的 Region 副本，并实施 Go 实现的实际 limit 语义。
- 私有函数 `overlap` 集中定义区间相交规则：相邻但不相交的半开区间不算重叠，空 `end_key` 表示延伸到键空间正无穷。

## 主要符号

- `pub struct Region { pub Meta: metapb::Region, pub Leader: Option<metapb::Peer> }`：拥有而非借用 protobuf 值；派生 `Clone`、`Debug`、`Default` 和 `PartialEq`。`Meta` 在 Rust 中不可为 `None`，`Leader` 可以缺失。
- `pub fn NewRegionInfo(meta, leader) -> Region`：无校验的值构造器，只把两个参数装入 `Region`。
- `pub struct RegionTree { pub Regions: Vec<Region> }`：线性容器，公开字段允许调用者直接读取或改写；派生的 `Default` 产生空集合。
- `RegionTree::SetRegion(&mut self, region)`：以 `Vec::retain` 保留所有不重叠项，再把新区间追加到末尾。复杂度为 O(n)，且调用后不保证按 key 排序。
- `RegionTree::ScanRange(&mut self, startKey, endKey, limit) -> Vec<Region>`：先原地排序，再构造查询枢轴，经 `overlap` 过滤、`take` 截断并克隆结果。排序为 O(n log n)，扫描与克隆为 O(n) 加返回数据成本。
- `fn overlap(a, b) -> bool`：模块私有的对称区间判定。若 `b.end <= a.start` 或 `a.end <= b.start`（且对应 end 非空）则不重叠，否则重叠。

文件没有常量、trait、条件编译项或异步函数。

## 执行流程

写入流程从 `SetRegion` 开始：遍历已有 `Regions`；每项与新区间调用 `overlap`；所有相交项被 `retain` 删除；最后把新区间追加。因而一次写入可同时覆盖多个旧分片，测试中的 `[b,f)` 会同时替换 `[a,c)` 和 `[e,g)`。边界刚好相接（例如 `[a,c)` 与 `[c,e)`）时，`end <= start` 分支判定为不重叠，两者都保留。

扫描流程从 `ScanRange` 开始：首先按字节序比较 `Meta.start_key` 并原地排序；随后用 `NewRegionInfo` 构造 `[startKey,endKey)` 查询枢轴；遍历排序后的集合并调用 `overlap`；对正 limit 最多取对应条数，对零 limit 使用 `usize::MAX` 表示不限，对负 limit 取零条；最后克隆命中的 `Region` 形成独立返回值。空查询 `endKey` 通过 `overlap` 被解释为扫描到键空间末尾。

## 数据与状态

唯一可变持久状态是 `RegionTree::Regions`。`SetRegion` 会改变成员集合但不排序；`ScanRange` 即使只做逻辑读取，也会为了稳定输出顺序原地重排集合，所以签名要求 `&mut self`。返回值是深层 `Clone` 后的新 `Vec<Region>`，之后修改树不会回写已返回结果，修改结果也不会影响树。

关键不变量来自实现而不是类型系统：正常通过 `SetRegion` 写入后，集合中不应保留与最后写入项重叠的旧项；但 `Regions` 是公开字段，调用者可绕过 `SetRegion` 填入重叠或乱序数据。实现不检查 Region ID、epoch、peer、区间连续性，也不拒绝空区间或起止键倒置，因此它只能模拟测试所需的区间替换与扫描语义。

## 依赖与调用关系

下游依赖只有 `kvproto::metapb::{Region, Peer}` 和 Rust 标准库的 `Vec`/排序/迭代器能力。`ScanRange -> NewRegionInfo -> metapb::Region::default` 构造查询枢轴；`SetRegion -> overlap` 与 `ScanRange -> overlap` 共享区间规则。没有网络、磁盘、时钟、随机数或其他 crate 内模块调用。

RustCodeGraph 的文件节点确认目标文件共 107 行，并标出 `NewRegionInfo`、`RegionTree`、`SetRegion`、`ScanRange`、`overlap` 等 7 个符号；通用名称的全局 callers 查询会混入其他模块的同名 mock，故最终直接调用关系以精确路径代码搜索为准。该搜索只找到 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接导入和调用本模块符号；`br/pkg/restore/split/mock_pd_client.rs` 有另一个独立的 `RegionTree`，类型与实现均不同，不是本文件调用者。

## 错误处理与边界

所有 API 都是无 `Result` 的内存操作，没有业务错误传播或恢复路径。空树扫描返回空集合。半开区间约定使相邻边界不重叠；非空 `end_key` 参与 `<=` 判断，空 `end_key` 则代表无上界。空 `start_key` 按普通字节序最小值处理。

`limit > 0` 时限制返回数量，`limit == 0` 时不限，`limit < 0` 时返回空集合。过滤条件中 `(limit as usize) > 0` 在 `limit > 0` 前提下恒真，真正的截断由后续 `take` 完成；修改这段逻辑时应以现有 Go 代码和负数回归测试为准。实现不会验证畸形区间、重复 ID 或 epoch 新旧关系；这些输入不会返回错误，而是机械套用区间比较规则。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。树和 Region 都拥有其数据，生命周期不依赖外部借用；删除的 Region 随 `retain` 结束被释放，扫描结果通过克隆获得独立所有权。两个修改方法都要求独占的 `&mut RegionTree`，同一实例不能在安全 Rust 中被多个调用者同时修改；若上层需要共享并发访问，必须自行提供 `Mutex`、`RwLock` 等同步机制。本模块没有声明或保证并发一致性。

## 与 Go 版本的对应关系

所有权表达有所不同：Go 的 `Meta`、`Leader`、`Regions` 元素和构造器返回值均为指针，Rust 的 `Meta` 与集合元素为拥有的值、仅 `Leader` 用 `Option` 表示可缺失，扫描也返回克隆值。

Rust 注释与实现明确记录了这一实际语义，并由 `migration_region_tree_replaces_overlaps_and_scans_in_key_order` 的负数断言锁定；因此不应仅按 Go 注释把负数改为不限，否则会破坏现有移植一致性。

## 扩展指南

若扩展区间判定，应首先修改唯一规则点 `overlap`，并在独立测试文件 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加相邻边界、空上界、空区间和倒置区间用例；不要把测试内嵌回生产源文件。若增加 ID/epoch 冲突处理，应接入 `SetRegion` 并先明确 Go 对照，因为当前契约只认 key 重叠。若要提供只读扫描，应避免 `ScanRange` 原地排序，例如维护写入有序不变量或排序索引，但需评估公开 `Regions` 字段会绕过不变量的问题。

若数据规模从小型测试集合扩大，应重新评估每次写入 O(n)、每次扫描 O(n log n) 和全量克隆成本，可能需要真正的区间索引；这种改变也会涉及顺序稳定性和公开字段兼容。任何 limit 语义调整都必须同步 Go 实现/注释与负数测试，不能只修改 Rust。新增 API 应继续放在本模块，测试保持在同目录独立 `*_test.rs` 文件中。
