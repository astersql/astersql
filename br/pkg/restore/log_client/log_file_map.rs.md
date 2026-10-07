# `br/pkg/restore/log_client/log_file_map.rs`

## 文件定位

该文件属于 `astersql-br-pkg-restore-log-client` library crate。crate 入口 `br/pkg/restore/log_client/lib.rs` 通过 `#[path = "log_file_map.rs"] pub mod log_file_map` 挂载模块，并用 `pub use log_file_map::*` 把其中的公开类型和函数提升到 crate 根。`br/pkg/restore/log_client/Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/restore/log_client` 的 Rust 移植；本文件自身只依赖标准库 `std::collections::HashMap`，没有 feature 条件、外部 crate 或 I/O 依赖。

它位于日志恢复的检查点恢复链路中：`br/pkg/restore/log_client/log_split_strategy.rs::NewLogSplitStrategy` 读取 `LogMetaManagerT::LoadCheckpointData` 返回的 `(groupKey, Goff, Foffs)`，把符合当前下游表 rewrite 规则的文件偏移插入 `LogFilesSkipMap`；随后 `LogSplitStrategy::ShouldSkip` 用日志数据文件的 `MetaDataGroupName`、`OffsetInMetaGroup` 和 `OffsetInMergedGroup` 查询该表，避免重新处理检查点已完成的文件，并在跳过时补报文件条目数和字节数。

## 核心职责

本文件用稀疏的三级索引表达“哪些日志文件已经完成，可以跳过”：

1. `metaKey: String` 定位一个 metadata group；
2. `groupOff: i32` 定位该 metadata group 内的文件组；
3. `fileOff: i32` 通过 64 位位图定位组内文件。

基础类型 `LogFilesSkipMap` 只支持精确到单文件的插入与查询，服务于当前 Rust 日志分裂策略的生产路径。扩展类型 `LogFilesSkipMapExt` 还可以把整个 meta 或整个 group 标记为跳过；RustCodeGraph 与仓库搜索显示它当前只由 `br/pkg/restore/log_client/parity_test.rs::go_rust_public_contract_matches` 使用，尚未接入 Rust 生产调用链，因此不能把 Ext 描述为当前恢复流程已使用的优化。

稀疏 `HashMap<i32, u64>` 只为出现过的 64 文件块分配存储。重复插入通过按位或保持幂等；未出现的 meta、group 或位图块统一解释为“不可跳过”，使缺失检查点记录采取安全的继续处理策略。

## 主要符号

- `pub type BitMap = HashMap<i32, u64>`：位图的稀疏块表。块键为 `off >> 6`，值的第 `off & 63` 位表示对应文件偏移是否命中。
- `newBitMap() -> BitMap`：建立空位图。虽然公开，但主要由本文件的懒初始化路径调用。
- `bit_pos(off: i32) -> (i32, u64)`：唯一私有帮助函数，将偏移拆成块号和单比特掩码；`bitMapSet` 与 `bitMapHit` 共用它，保证读写寻址一致。
- `bitMapSet(m: &mut BitMap, off: i32)`：缺失块以零初始化后执行按位或；同一偏移多次插入不会改变最终结果。
- `bitMapHit(m: &BitMap, off: i32) -> bool`：缺失块按零读取，只有对应位非零才返回 `true`。
- `BitMapExt { bitMap, skip }`：group 层扩展节点。`skip == true` 表示整个 group 跳过，此时逐文件位图不再参与判定。
- `FileMap { pos }`：基础版的 group 偏移到 `BitMap` 的映射。
- `FileMapExt { pos, skip }`：扩展版的 meta 节点。`skip == true` 表示该 meta 下所有 group 都跳过；否则 `pos` 保存各 group 的 `BitMapExt`。
- `LogFilesSkipMap { skipMap }`：基础跳过表，公开方法为 `Insert` 和 `NeedSkip`。
- `NewLogFilesSkipMap() -> LogFilesSkipMap`：基础表构造器；Rust 返回所有权值，而 Go 对照函数返回指针。
- `LogFilesSkipMapExt { skipMap }`：支持 `Insert`、`SkipMeta`、`SkipGroup`、`NeedSkip` 的扩展跳过表。
- `NewLogFilesSkipMapExt() -> LogFilesSkipMapExt`：扩展表构造器。

所有结构体字段均为 `pub`，crate 外调用者理论上可以直接改写内部映射；不过本文件定义的不变量由上述构造器和方法维护，安全扩展应优先使用这些方法而非直接拼装内部状态。

## 执行流程

基础版插入流程 `LogFilesSkipMap::Insert(metaKey, groupOff, fileOff)` 为：

1. 以 `metaKey.to_string()` 查找 `skipMap`，不存在时用 `newFileMap` 建立 meta 节点；
2. 在该节点的 `pos` 中查找 `groupOff`，不存在时用 `newBitMap` 建立位图；
3. `bitMapSet` 计算 `(fileOff >> 6, 1 << (fileOff & 63))`，再把掩码或入对应块。

基础版查询 `LogFilesSkipMap::NeedSkip` 按 meta、group、位图块逐层查找。meta 或 group 缺失时立即返回 `false`；两层都存在时由 `bitMapHit` 判断目标位。`br/pkg/restore/log_client/log_split_strategy.rs::ShouldSkip` 正是以文件元数据中的三个定位字段调用该方法。

扩展版插入 `LogFilesSkipMapExt::Insert` 同样懒创建 meta 与 group，但先后检查 `FileMapExt::skip` 和 `BitMapExt::skip`。任一整层跳过标志已经成立时，单文件标记没有额外信息，方法直接返回；否则才在 group 位图中置位。

`SkipMeta` 用新的 `FileMapExt { skip: true, pos: empty }` 覆盖同名 meta，因而会主动丢弃此前的逐 group/逐文件明细。`SkipGroup` 在 meta 未整层跳过时，用新的 `BitMapExt { skip: true, bitMap: empty }` 覆盖目标 group，也会丢弃该 group 旧的逐文件明细。由于整层标记是更强的跳过条件，这种覆盖不改变当前查询结果。

扩展版 `NeedSkip` 的优先级为：meta 不存在则 `false`；meta 的 `skip` 为真则 `true`；group 不存在则 `false`；group 的 `skip` 为真则 `true`；最后查询单文件位图。该顺序与 `log_file_map.go` 完全对应。

## 数据与状态

该模块保存的全部状态都在内存中，不序列化、不访问存储，也不自行加载检查点。检查点数据到三级坐标的转换由上游 `NewLogSplitStrategy` 完成：`groupKey` 对应 `metaKey`，`LogRestoreValueMarshaled::Goff` 对应 `groupOff`，`Foffs` 中每个文件偏移对应 `fileOff`。

位图块固定包含 64 个偏移。例如 `fileOff == 65` 映射到块 `1` 的第 `1` 位；`br/pkg/restore/log_client/parity_test.rs` 明确验证偏移 65 命中而相邻偏移 64 不命中。空间复杂度取决于出现过的 meta、group 与 64 位块数量，而不是最大文件偏移；这适合偏移稀疏的检查点集合。每个 meta key 在插入时复制成拥有所有权的 `String`，后续查询只借用 `&str`，不会重复分配。

代码使用 `i32` 偏移并未校验非负性。负值仍会按 Rust 的有符号右移和低 6 位掩码落入某个负块键及有效位，但生产输入预期来自检查点文件偏移；若调用边要接受不可信或更大范围的偏移，应在数据来源处验证，或同步 Go/Rust 契约后统一改型，不能只在一侧改变。

## 依赖与调用关系

上游生产调用关系为：

`LogClient` 安装/使用日志分裂策略相关能力 → `NewLogSplitStrategy` → `LogMetaManagerT::LoadCheckpointData` 回调 → `LogFilesSkipMap::Insert`；处理单个 `LogDataFileInfo` 时，`LogSplitStrategy::ShouldSkip` → `LogFilesSkipMap::NeedSkip`。其中只有匹配当前下游表 ID 集合的 `Foffs` 会被写入，防止已删除或不在本次恢复范围的表污染跳过判定。

本文件内部调用边为：

- `NewLogFilesSkipMap` 建立空的顶层 `HashMap`；`Insert` 调用 `newFileMap`、`newBitMap` 和 `bitMapSet`；`NeedSkip` 调用 `bitMapHit`。
- `NewLogFilesSkipMapExt` 建立空的扩展顶层表；`Insert`、`SkipMeta`、`SkipGroup` 按需调用 `newFileMapExt`/`newBitMapExt`；扩展版 `NeedSkip` 在未命中整层标志时调用 `bitMapHit`。
- `newBitMapExt` 调用 `newBitMap`；`bitMapSet` 和 `bitMapHit` 调用私有 `bit_pos`。

RustCodeGraph 将 `log_file_map.rs` 的明确外部使用定位到 `log_split_strategy.rs`（基础版）与测试模块；仓库搜索未发现 Ext 版的 Rust 生产调用者。`lib.rs` 的通配再导出意味着外部 crate 仍可直接引用这些公开符号，所以修改公开类型、字段或函数签名会产生 crate API 兼容风险。

## 错误处理与边界

本文件没有 `Result`、错误类型、日志或 panic 分支；普通缺失状态被建模为布尔值而不是错误。查询的关键保守边界是：没有证据表明文件已完成时一律返回 `false`，让上游继续处理文件，而不是错误跳过数据。

插入是幂等的，重复 meta/group/file 坐标只会重复执行按位或。位图跨块边界由统一的 `bit_pos` 处理，0～63 位于块 0，64～127 位于块 1；契约测试覆盖 64/65 的相邻边界。基础独立测试还以原生 `HashMap<String, HashMap<i32, HashSet<i32>>>` 为真值，在多种稀疏密度下验证所有已插入坐标均命中、所有未插入坐标均不误命中。

扩展版的覆盖操作不可逆：调用 `SkipMeta` 后，后续 `Insert` 和 `SkipGroup` 都不能把该 meta 恢复成局部状态；调用 `SkipGroup` 后，后续 `Insert` 也不能清除整 group 标志。当前 API 没有 unskip 操作，这是单调累积“已完成”状态的设计结果；如需撤销，必须明确新增契约并补测试，不能依赖直接改公开字段。

## 并发与资源生命周期

这些容器没有锁、原子变量、后台任务、通道或异步生命周期。修改方法要求 `&mut self`，Rust 类型系统会阻止在安全代码中同时对同一实例进行多个可变访问；只读 `NeedSkip(&self, ...)` 可以在调用方满足 `Sync`/共享所有权约束时并发读取。当前 `LogSplitStrategy` 将基础表作为自身字段持有，并通过 `&mut self` 的 `ShouldSkip` 串联进度回调，因此本文件不负责跨线程同步。

所有映射和字符串随拥有它们的 `LogFilesSkipMap`/`LogFilesSkipMapExt` 一起释放，没有显式 `Close` 或清理顺序。构造器只创建空顶层表，内部节点在首次插入或整层跳过时懒创建；`SkipMeta`/`SkipGroup` 覆盖旧节点时，旧映射立即按 Rust 所有权规则释放。

若未来要在多个 worker 间共享可变跳过表，应由上层选择锁、分片或消息传递策略，并评估 `HashMap` 锁竞争；不应在不改变 API 与调用模型的情况下假定当前结构具备并发写安全性。

## 与 Go 版本的对应关系

直接对照文件为 `br/pkg/restore/log_client/log_file_map.go`，Rust 保留了 Go 的类型层次、构造器/方法名称与判定顺序：Go `bitMap map[int]uint64` 对应 Rust `HashMap<i32, u64>`；`Set`/`Hit` 对应自由函数 `bitMapSet`/`bitMapHit`；`fileMap`、`LogFilesSkipMap` 及 Ext 结构逐层对应。两边都以 `off >> 6` 取块号、以 `1 << (off & 63)` 取位，并在缺失键时按零或“不跳过”处理。

主要语言差异是：Go 构造器返回指针，Rust 构造器返回拥有所有权的值并由 `&mut self`/`&self` 区分修改与查询；Go 的嵌入式 `bitMap` 方法在 Rust 中展开为显式字段和自由函数；Go 偏移类型是平台宽度的 `int`，Rust 固定为 `i32`。当前生产路径中的检查点偏移也使用 `i32`，因此 Rust 内部一致，但扩大范围时需注意与 Go `int` 的上限差异。

`br/pkg/restore/log_client/log_file_map_test.rs::test_log_files_skip_map` 移植了 Go `log_file_map_test.go::TestLogFilesSkipMap` 的基础版测试意图：以原生集合为真值，覆盖从稀疏到接近稠密的插入与全空间反向检查。Rust 使用固定种子的 LCG 代替 Go 全局随机源，使测试可复现。两个同名独立测试都没有覆盖 Ext；Rust 额外由 `parity_test.rs::go_rust_public_contract_matches` 验证整 meta、整 group、单 offset 及不同 group 不互相影响的公开契约。

## 扩展指南

- 若新增文件级操作（例如删除或合并状态），应优先在 `LogFilesSkipMap`/`LogFilesSkipMapExt` 方法中维护三级索引不变量，并同步 `log_file_map.go` 或明确记录语言差异；不要让调用方直接操作公开字段形成第二套语义。
- 若改变块宽、偏移类型或寻址算法，必须同时修改 `bit_pos`、`BitMap` 值类型、读写函数，并在独立的 `log_file_map_test.rs` 增加 63/64、127/128、最大允许偏移以及重复插入回归测试。Rust 测试逻辑应继续与 Go 同路径测试保持一致，不要内嵌回生产文件。
- 若把 Ext 版接入生产链路，应先确认数据源如何表达“整 meta/整 group 已完成”，再在相应策略或检查点加载函数中接线；同步扩展独立 Rust 测试，覆盖先插入后 `SkipMeta`/`SkipGroup`、先整层跳过后插入、缺失层查询等顺序效应。
- 若需要并发写，应在上层明确同步所有权和性能目标。简单加全局互斥锁可能把检查点加载或文件调度串行化，分片键则应优先按 `metaKey` 设计并用基准或压力测试验证。
- 若要收紧负偏移或溢出边界，应在 Go/Rust 输入契约、检查点反序列化类型和调用处一并验证；单独改变本文件会造成跨语言行为漂移或破坏已存检查点兼容性。
- 生产调用变化至少同步检查 `log_split_strategy.rs` 及其独立测试；公开契约变化同步 `parity_test.rs`；位图本身的行为变化同步 `log_file_map_test.rs` 和 Go 对照测试。

## 验证依据

- RustCodeGraph `status`：索引覆盖本仓库 7032 个 Rust 文件，可用于本次符号与调用关系核对。
- RustCodeGraph `node --file br/pkg/restore/log_client/log_file_map.rs`：读取全部 192 行实现，确认无条件编译项、无 I/O/错误路径，并核对所有类型、构造器、方法及内部调用。
- RustCodeGraph 对 `NewLogFilesSkipMap`、`NewLogFilesSkipMapExt`、`Insert`、`NeedSkip` 的 explore/caller/callee 结果，以及仓库 `rg` 交叉检查：确认基础版生产使用位于 `log_split_strategy.rs`，Ext 版当前只见于 `parity_test.rs`。
- `br/pkg/restore/log_client/Cargo.toml` 与 `br/pkg/restore/log_client/lib.rs`：确认 crate 名称、Go 包映射、模块挂载、公开再导出以及测试分文件挂载方式。
- `br/pkg/restore/log_client/log_split_strategy.rs::{NewLogSplitStrategy, ShouldSkip}` 及 Go 对照 `log_split_strategy.go`：确认检查点坐标的写入来源、下游表过滤和生产查询/进度回调链。
- `br/pkg/restore/log_client/log_file_map.go`：逐项核对位图计算、懒初始化、整层短路和缺失状态语义。
- `br/pkg/restore/log_client/log_file_map_test.rs` 与 `log_file_map_test.go`：核对基础版稀疏/近稠密测试、真值集合和正反向断言；`br/pkg/restore/log_client/parity_test.rs::go_rust_public_contract_matches` 补充 Ext 与 64 位块边界证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核唯一生产物、源码链接、测试位置和未接线事实。
