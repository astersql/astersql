# `pkg/store/pdtypes/region_tree.rs`

## 文件定位

本文件属于 Cargo crate `astersql-store-pdtypes`，由 [`lib.rs`](./lib.rs) 以公开模块 `region_tree` 暴露；crate 的 Go 包映射是 `pkg/store/pdtypes`（[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package`）。它实现的是供测试和迁移对照使用的简化 PD Region 集合，而不是真正的平衡区间树，也不负责访问 PD、持久化元数据或执行调度。

直接的 Rust 使用证据位于 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)：该独立测试导入 `NewRegionInfo` 与 `RegionTree`，验证重叠替换、按起始键扫描、条数限制和负限制行为。仓库根 `Cargo.toml` 还以 `facade_store_pdtypes` 暴露整个 crate，`pkg/ingestor/ingestctrl/Cargo.toml` 与 `pkg/ddl/placement/Cargo.toml` 依赖此 crate；不过代码搜索没有发现这些生产 crate 直接调用本文件的 API，因此不能把 crate 级依赖误写成 `RegionTree` 的生产调用链。

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

直接对照文件是 [`region_tree.go`](./region_tree.go)。Rust 保留了 Go 的 `Region`、`NewRegionInfo`、`RegionTree`、`SetRegion`、`ScanRange` 和私有 `overlap` 结构，以及“删除所有重叠项后追加”“扫描前按 start key 排序”“空 end 表示正无穷”的实际行为。

所有权表达有所不同：Go 的 `Meta`、`Leader`、`Regions` 元素和构造器返回值均为指针，Rust 的 `Meta` 与集合元素为拥有的值、仅 `Leader` 用 `Option` 表示可缺失，扫描也返回克隆值。这样避免了 `Meta == nil` 的状态，但增加了扫描克隆成本。

Go 注释写着 `limit <= 0 means no limit`，实际循环条件却是 `limit == 0 || len(res) < limit`，所以负数不会加入任何结果。Rust 注释与实现明确记录了这一实际语义，并由 `migration_region_tree_replaces_overlaps_and_scans_in_key_order` 的负数断言锁定；因此不应仅按 Go 注释把负数改为不限，否则会破坏现有移植一致性。

## 扩展指南

若扩展区间判定，应首先修改唯一规则点 `overlap`，并在独立测试文件 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加相邻边界、空上界、空区间和倒置区间用例；不要把测试内嵌回生产源文件。若增加 ID/epoch 冲突处理，应接入 `SetRegion` 并先明确 Go 对照，因为当前契约只认 key 重叠。若要提供只读扫描，应避免 `ScanRange` 原地排序，例如维护写入有序不变量或排序索引，但需评估公开 `Regions` 字段会绕过不变量的问题。

若数据规模从小型测试集合扩大，应重新评估每次写入 O(n)、每次扫描 O(n log n) 和全量克隆成本，可能需要真正的区间索引；这种改变也会涉及顺序稳定性和公开字段兼容。任何 limit 语义调整都必须同步 Go 实现/注释与负数测试，不能只修改 Rust。新增 API 应继续放在本模块，测试保持在同目录独立 `*_test.rs` 文件中。

## 验证依据

- RustCodeGraph：运行 `status`，索引包含 11,467 个文件且目标文件可用；运行 `files --filter pkg/store/pdtypes` 确认模块文件集合；运行 `node --file pkg/store/pdtypes/region_tree.rs --offset 1 --limit 260` 读取 107 行完整源码并确认符号；运行 `query RegionTree --kind struct`、`query NewRegionInfo --kind function` 区分 Rust、Go 及 BR 中的同名实现。
- Rust 源与装配：[`region_tree.rs`](./region_tree.rs)（全部实现）、[`lib.rs`](./lib.rs)（公开模块声明）、[`Cargo.toml`](./Cargo.toml)（crate 名称、`kvproto` 依赖和 Go 包映射）。
- Go 对照：[`region_tree.go`](./region_tree.go)，逐项核对构造、重叠替换、排序扫描、空上界及实际 limit 条件。
- 独立测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的 `migration_region_tree_replaces_overlaps_and_scans_in_key_order`，覆盖多旧区间替换、全范围有序扫描、正 limit 截断和负 limit 空结果。
- 调用搜索：对 `pkg`、`br`、`tests` 中的 Rust 文件搜索 `pdtypes::region_tree`、`RegionTree`、`NewRegionInfo`、`SetRegion` 与 `ScanRange`，确认本模块直接调用集中在上述迁移测试，并排除 BR 的同名独立类型。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文档存在且固定二级标题恰好为 11 个，并人工复核唯一新增生产物为本说明文件。
