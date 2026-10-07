# `br/pkg/restore/data/recover.rs`

## 文件定位

该文件属于 `astersql-br-pkg-restore-data` library crate。crate 入口 [`lib.rs`](lib.rs) 以公开模块 `recover` 装配它，并通过 `pub use recover::*` 再导出其公开类型和函数；[`Cargo.toml`](Cargo.toml) 将对应的 Go package 标为 `br/pkg/restore/data`，且当前没有外部 Rust 依赖。本文件本身不访问网络或磁盘，而是 `Recovery::MakeRecoveryPlan` 的纯内存决策层：把各 store 汇报的 Region peer 元数据归并为一致的有效 Region 集合，再为每个有效 Region 选择 leader。

生产调用位于 [`data.rs`](data.rs) 的 `Recovery::MakeRecoveryPlan`：它先按 `RegionId` 汇集 `RecoverRegion`，调用 `SortRecoverRegions` 和 `CheckConsistencyAndValidPeer`，随后对有效 Region 调用 `LeaderCandidates` 与 `SelectRegionLeader`，最终写入 `Recovery::RecoveryPlan`。因此，本文件处在“读取 store 元数据”与“向各 store 下发恢复请求”之间，不负责元数据采集、RPC 下发、ID 分配或 flashback。

## 核心职责

1. 用 `RecoverRegion` 把 protobuf 风格的 `RegionMeta` 与来源 `StoreId` 绑定，使后续计划既能比较 Raft 状态，也能定位请求目标 store。
2. `SortRecoverRegions` 对同一 Region 的副本按 `LastLogTerm`、`LastIndex`、`CommitIndex` 三级降序排列，取最领先副本的版本、范围和 tombstone 状态作为该 Region 的代表；再把所有代表按 `RegionVersion` 降序排列。
3. `CheckConsistencyAndValidPeer` 让高版本 Region 优先占据有序区间集合，跳过与已选区间重叠的旧候选，然后验证剩余区间从空起点开始首尾相接且代表不是 tombstone，返回有效 `RegionId` 集合。
4. `LeaderCandidates` 从已经按 Raft 进度排序的副本中，保留与首副本的 term/index/commit 完全相同者；`SelectRegionLeader` 再从这些候选中选择当前 leader 计数最低的 store，以便 `MakeRecoveryPlan` 分散 leader。

这里的“有效”是恢复计划层面的区间与代表 peer 有效性，不等同于在线探活或 Raft quorum 校验；这些函数只消费调用者已经收集到的元数据。

## 主要符号

- `RecoverRegion { RegionMeta, StoreId }`：公开输入记录。`RegionMeta` 来自 `crate::stubs::recovpb`，包含 Region/Peer ID、Raft 日志进度、epoch version、tombstone 和键范围；`StoreId` 标记元数据来源。其 `Deref<Target = RegionMeta>` 模拟 Go 匿名嵌入字段，允许 `peer.RegionId`、`peer.LastIndex` 等字段访问写法。
- `RecoverRegionInfo { RegionId, RegionVersion, StartKey, EndKey, TombStone }`：公开的区间裁决记录。它只保留每个 Region 最领先副本中与冲突消解有关的数据；键已经经过 `PrefixStartKey` / `PrefixEndKey` 规范化。
- `SortRecoverRegions(&mut HashMap<u64, Vec<RecoverRegion>>) -> Vec<RecoverRegionInfo>`：原地排序每个 Region 的 peer 向量，并返回按 version 降序的代表信息。后续 `LeaderCandidates` 依赖这里保留下来的 peer 顺序。
- `CheckConsistencyAndValidPeer(Vec<RecoverRegionInfo>) -> Result<HashSet<u64>>`：以 `BTreeMap<StartKey, RecoverRegionInfo>` 模拟 Go treemap，执行重叠淘汰、tombstone 检查和相邻性检查。
- `LeaderCandidates(&[RecoverRegion]) -> Result<Vec<RecoverRegion>>`：以 `peers[0]` 为 Raft 状态基线，返回相同 `LastLogTerm`、`LastIndex`、`CommitIndex` 的候选副本克隆。
- `SelectRegionLeader(&HashMap<u64, i32>, &[RecoverRegion]) -> RecoverRegion`：选择 `StoreId` 对应分数最小的候选；缺失分数按 `0`，同分保持较早候选。

本文件没有模块级常量、trait、异步函数或条件编译项，所有业务符号都是公开 API；唯一内部实现是 `RecoverRegion` 的 `Deref`。

## 执行流程

`Recovery::MakeRecoveryPlan` 驱动的完整局部流程如下：

1. 调用者遍历 `Recovery::StoreMetas`，按 `RegionId` 把不同 store 的 `RegionMeta` 包装成 `RecoverRegion`；同时更新恢复所需的 `MaxAllocID`。
2. `SortRecoverRegions` 遍历 Region 分组，按 term → last index → commit index 的优先级对 peer 降序排序。每组首项成为代表，其原始键范围被转换到恢复内部键空间：普通键前置字节 `z`，空结束键由 `PrefixEndKey` 转成 `z + 1`，表示正无穷边界。代表记录随后按 `RegionVersion` 降序排列。
3. `CheckConsistencyAndValidPeer` 按上述版本顺序逐项尝试插入 `BTreeMap`。若 ceiling 的起点落在当前区间内，或 floor 区间的终点越过当前起点，则当前候选与已选候选重叠并被跳过；由于高版本先处理，这实现了 split/merge 期间的版本冲突优先级。
4. 函数按起点顺序扫描保留区间，以 `PrefixStartKey([])` 为首个期望起点。每个区间必须非 tombstone，且当前起点等于前一区间终点；通过者的 `RegionId` 被加入 `HashSet`。
5. `MakeRecoveryPlan` 对不在有效集合中的 Region，为其各副本生成 tombstone 请求；对有效 Region，调用 `LeaderCandidates` 收窄到 Raft 状态并列最优的副本。
6. `SelectRegionLeader` 使用 `storeBalanceScore` 选择分数最低的候选。调用者把所选 peer 写成 `AsLeader=true` 的恢复请求，并将该 store 分数加一，使后续 Region 倾向其他低分 store。

## 数据与状态

- `regions` 的键是 Region ID，值是该 Region 在各 store 上的副本快照。`SortRecoverRegions` 会原地改变每个向量的顺序；这是后续以 `peers[0]` 为领先副本的关键不变量。
- peer 优先级只由三项 Raft 进度决定，顺序为 `LastLogTerm`、`LastIndex`、`CommitIndex`；`Version` 不参与同一 Region 内的副本排序，而用于不同 Region 区间发生重叠时的全局优先级。
- `RecoverRegionInfo` 的 `StartKey` / `EndKey` 不是原始用户键。`key.rs` 定义普通边界为 `b'z' + key`，空 EndKey 为单字节 `b'{'`，使无限结束边界排在所有 `b'z'` 前缀键之后。
- `treeMap` 是函数内局部 `BTreeMap`，键为规范化起点；其有序迭代同时承担区间顺序恢复与相邻性验证。
- `validPeers` 只记录 Region ID，不记录具体 peer。具体 leader 仍从 `SortRecoverRegions` 已排序的原始 `regions` 中产生。
- `storeBalanceScore` 由 `MakeRecoveryPlan` 跨 Region 维护；本文件只读取它。未知 store 的分数视为 `0`，选中后由调用者递增。

## 依赖与调用关系

上游生产链为 `doRecoveryData` → `Recovery::MakeRecoveryPlan` → 本文件四个函数；`MakeRecoveryPlan` 的结果随后由 `Recovery::RecoverRegions` / `RecoverRegionOfStore` 通过恢复流发送。本文件也由 crate 根再导出，所以测试可从 `crate::recover` 或 crate 公共表面访问。

直接下游依赖包括：

- 标准库 `HashMap`、`HashSet`、`BTreeMap`：分别承载 Region 分组、有效 ID 集合和有序区间集合。
- `crate::key::{PrefixStartKey, PrefixEndKey, keyCmp, keyEq}`：统一内部键空间，并沿用 Go 的字节序比较语义。
- `crate::stubs::recovpb::RegionMeta`：当前 crate 内 protobuf 消息的本地 Rust 结构；`RecoverRegion` 通过 `Deref` 暴露其字段。
- `crate::stubs::{Error, Result}` 与 `berrors`：构造可供上层 `Error::Trace` 传播的恢复错误。
- `crate::stubs::log`：在 tombstone、区间不相邻和候选检查路径记录消息；没有改变控制流之外的持久状态。

RustCodeGraph 的 `node` 显示 `recover.rs` 直接被 `parity_test.rs` 和 `recover_test.rs` 使用；精确源码检索还确认生产调用在 `data.rs:696-720`。测试调用覆盖 `parity_test.rs`、`key_test.rs` 和 `recover_test.rs`。

## 错误处理与边界

- `SortRecoverRegions` 直接访问每组 `peers[0]`。空 peer 向量违反收集阶段不变量并触发 panic；独立测试 `recover_test.rs::sort_recover_regions_rejects_region_without_peers` 用 `#[should_panic]` 固定该行为。函数不会静默忽略坏分组。
- `CheckConsistencyAndValidPeer` 遇到最终保留的 tombstone 代表时返回 `ErrRestoreInvalidPeer` 的注解错误；遇到首区间不从空起点开始、区间缺口或乱序时返回 `ErrInvalidRange` 的注解错误。重叠候选不是错误，而是按 version 优先规则被淘汰。
- 相邻性扫描验证首个起点及所有相邻边界，但当前实现没有在循环结束后显式断言最后一个 `EndKey` 等于无限结束边界；扩展验证时不能把“完整覆盖至正无穷”写成已经由此函数单独保证的事实。
- `LeaderCandidates` 对空切片返回 `ErrRestoreRegionWithoutPeer`；这比 Go 代码仅判断 `nil` 更统一地拒绝无 peer 输入。非空输入要求调用方已经按 `SortRecoverRegions` 排序，否则首项基线可能不是最领先副本。
- `SelectRegionLeader` 同样直接访问 `peers[0]`，因此要求非空；正常生产路径由成功的 `LeaderCandidates` 保证。分数缺失等价于 `0`，负分也会按普通整数比较成为更低负载。
- 所有错误均同步返回；本文件不捕获 panic、不重试，也不负责恢复失败后的阶段回滚。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、RPC 连接或文件句柄。四个函数均同步执行，临时 `Vec`、`HashSet` 和 `BTreeMap` 在调用结束时由所有权规则释放；返回值或克隆的 `RecoverRegion` 由调用者接管。

`SortRecoverRegions` 需要独占可变借用，因为它原地重排 peer 向量；其余函数读取借用或取得输入向量所有权。`LeaderCandidates` 和 `SelectRegionLeader` 会克隆所选记录，因此结果不借用调用者切片。并发安全主要由上层保证：若多个线程需要共享 `regions` 或负载分数，必须在本文件之外协调；当前 `MakeRecoveryPlan(&mut self)` 以独占 `Recovery` 借用串行构建计划。

复杂度方面，设 Region 数为 R、第 i 个 Region 的副本数为 Pᵢ：副本排序约为 Σ O(Pᵢ log Pᵢ)，代表排序为 O(R log R)，区间插入与扫描为 O(R log R) 与 O(R)，候选筛选及选主对单 Region 为 O(Pᵢ)。主要内存开销来自代表向量、有序映射、有效 ID 集合和候选克隆。

## 与 Go 版本的对应关系

直接对照文件是 [`recover.go`](recover.go)，主链对照位于 [`data.go`](data.go) 的 `MakeRecoveryPlan`：

- Rust `RecoverRegionInfo` 与 Go 同名结构字段一致；Go 的 `RecoverRegion` 定义在 `data.go`，Rust 将其放在 `recover.rs` 并以 `Deref` 模拟匿名嵌入的字段提升。
- 两版 `SortRecoverRegions` 都按 term/index/commit 降序选择代表，再按 version 降序排列 Region 信息。Rust 使用比较器链，避免 Go 比较函数中无符号相减后转 `int` 的表达形式，但目标顺序一致。
- Go 用 `github.com/emirpasic/gods/maps/treemap` 配合 `keyCmpInterface`，Rust 用标准库 `BTreeMap<Vec<u8>, _>`；ceiling、floor、重叠跳过和后续相邻检查的分支对应。
- 两版均把 tombstone 代表和不连续区间包装为 BR 恢复错误；Rust 日志桩不携带 Go `zap` 字段，因此诊断字段丰富度不同，但控制流与错误类别保持对应。
- Go `LeaderCandidates` 只显式拒绝 `nil` slice，非 nil 的空 slice 随后会在 `[0]` 处 panic；Rust 明确拒绝所有空切片并返回恢复错误。正常生产输入的语义相同，边界行为更严格。
- 两版 `SelectRegionLeader` 都把缺失分数当零、只在严格更低时替换，因此同分保持首候选；两版都要求非空候选。
- Rust crate 使用本地 `stubs::recovpb::RegionMeta` 而非外部 kvproto crate；这与 `Cargo.toml` 当前“无外部依赖、本地 traits/stubs”的迁移边界一致，不应误写为已直接接入真实 gRPC protobuf。

Go 测试 [`key_test.go`](key_test.go) 验证排序、连续区间、并列候选和最低分选主；Rust 的 [`key_test.rs`](key_test.rs) 镜像这些用例，[`parity_test.rs`](parity_test.rs) 还把这些辅助函数放进 `MakeRecoveryPlan` 与恢复流程契约中，[`recover_test.rs`](recover_test.rs) 额外固定空 peer 分组 panic。

## 扩展指南

- 修改副本优先级时，应集中调整 `SortRecoverRegions` 的比较器，并同步检查 `LeaderCandidates` 的“并列”字段集合；两处定义不一致会造成代表排序与候选资格脱节。至少同步更新 `key_test.rs`、`parity_test.rs::contract_normal_keys_and_sort` 及 Go 对照测试意图。
- 修改区间冲突规则时，应在 `CheckConsistencyAndValidPeer` 的 ceiling/floor 两个分支成对处理，并保持输入按 `RegionVersion` 降序的不变量。需要新增覆盖同起点、包含、相交、相邻、tombstone、首部缺口和尾部边界的独立 Rust 测试，不要把测试内嵌到生产文件。
- 若要强制完整覆盖到正无穷，应在相邻扫描结束后显式核对最终 `prevEndKey == PrefixEndKey([])`，并先确认 Go 行为与恢复协议兼容性；这是可见行为变化，不应只当文档或重构处理。
- 修改负载均衡策略时，入口是 `SelectRegionLeader`，状态更新则在 `data.rs::Recovery::MakeRecoveryPlan` 的 `storeBalanceScore` 递增处。必须共同验证缺失分数、同分稳定性、单候选和跨多个 Region 的分布；额外的全局状态可能引入并发与可复现性风险。
- 替换本地 `stubs::recovpb` 为真实依赖时，应先处理 crate 依赖边界、protobuf getter/所有权差异与错误类型，再保持本文件的纯决策职责；根据仓库约束，外部 Rust 依赖必须来自带 tag 的上游 Git 依赖，不能复制到本仓库本地覆盖。
- 任何逻辑变更都应继续把测试放在同目录的独立 `*_test.rs` / `parity_test.rs`，并对照 `recover.go` 和 `key_test.go`，避免 Rust 与 Go 恢复决策漂移。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/data` 确认目标、模块、Go 对照和测试文件均在索引中。
- RustCodeGraph `node --file br/pkg/restore/data/recover.rs --offset 1 --limit 500`：核对本文件 210 行全貌、两个公开结构、四个公开函数、`Deref`、错误分支和直接测试使用者。
- RustCodeGraph `query`：分别定位 Rust/Go 的 `SortRecoverRegions`、`CheckConsistencyAndValidPeer`、`LeaderCandidates`、`SelectRegionLeader` 及 Go 测试同名符号。
- RustCodeGraph `explore` 与 `node --file br/pkg/restore/data/data.rs --offset 660 --limit 90`：确认 `Recovery::MakeRecoveryPlan` 是生产入口，并核对收集、有效集、tombstone 请求、选主和 store 分数递增的调用顺序。
- RustCodeGraph `node`：读取 `key.rs` 的键规范化/比较实现，以及 `stubs.rs` 的 `RegionMeta` 字段与 getter。
- 直接读取的边界与对照文件：`br/pkg/restore/data/Cargo.toml`、`lib.rs`、`recover.go`、`data.go`、`key_test.go`、`key_test.rs`、`recover_test.rs`、`parity_test.rs`；精确 `rg` 检索交叉确认所有 Rust 生产与测试调用点。
- 结构验证要求：目标文档存在，且固定的十一个二级标题各出现一次。该任务是纯文档分析，按计划不运行 Cargo。
