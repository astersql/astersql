# `br/pkg/utiltest/crr/builder.rs`

## 文件定位

本文件属于 Cargo 库 `astersql-br-pkg-utiltest-crr`，crate 根为 [`lib.rs`](lib.rs)，并由该入口以 `pub use builder::*` 再导出。它不是生产 SQL、备份或复制请求的执行入口，而是 CRR（跨区域复制）测试夹具的静态 Region 布局构建层：调用方先组合布局选项，得到 `Vec<RegionBoundary>`，再把结果交给 [`harness.rs`](harness.rs) 的 `NewLocalTestHarnessWithTestContext` 或 [`pd_sim.rs`](pd_sim.rs) 的 `NewPDSimWithTestContext`，由后者将边界物化为 fakecluster 中的 store 和 region。

直接数据类型 `RegionBoundary` 定义在 [`types.rs`](types.rs)，包含字节形式的 `StartKey`、`EndKey` 与 `StoreID`。空起键表示负无穷，空止键表示正无穷，区间按左闭右开 `[start, end)` 理解。对应 Go 实现为 [`builder.go`](builder.go)。

## 核心职责

- `BuildRegionLayout` 顺序折叠一组 `RegionLayoutOption`，在选项失败时立即短路，并在结束时拒绝空布局或末段未以空 `EndKey` 闭合的布局。
- `StoreIDRange` 生成连续 store ID，供测试方便地构造轮询目标。
- `AddRegion`、`AddRegionsBySplitKeys`、`AddRoundRobinRegions` 提供三种可组合的布局选项：追加显式区间、按给定 split key 切段、按自动生成的 `k01` 风格 split key 切段。
- 私有 `appendRegion` 集中检查单次追加的 store ID、首段起点、与前段的连续性和非空终点区间的字节序；`cloneRegionBoundaries` 对边界及键字节做深拷贝。

职责边界必须注意：`BuildRegionLayout` 自身只做“非空”和“最后闭合”两项终检。连续性、首段空起键和区间有序主要由内置选项经 `appendRegion` 保证；自定义 `RegionLayoutOption` 可以绕过这些局部检查。下游 `pd_sim.rs::validateBoundaries` 会再次检查首段、末段、排序、连续性和非零 store，但本文件不能被描述成会全面验证任意自定义选项的结果。

## 主要符号

- `pub type RegionLayoutOption = Box<dyn Fn(Vec<RegionBoundary>) -> Result<Vec<RegionBoundary>, String> + Send + Sync>`：拥有输入布局并返回新布局的选项闭包。`Send + Sync` 允许闭包本身跨线程传递/共享，但本文件的执行仍是同步串行的。
- `BuildRegionLayout(opts)`：从空向量开始，依次执行选项；成功后深拷贝返回。公开 API 使用 `String` 作为错误类型。
- `StoreIDRange(start, count)`：当 `count <= 0` 时返回空向量；否则生成 `[start, start + count)`。加法用 `wrapping_add`，因此溢出按 Go `uint64` 模 (2^64) 行为处理。
- `AddRegion(startKey, endKey, storeID)`：把字符串参数复制进闭包，执行时委托 `appendRegion` 追加一个区间。
- `AddRegionsBySplitKeys(splitKeys, storeIDs)`：按 split key 顺序追加若干区间，再追加一个空终点的尾段；store 按下标取模轮询。若已有未闭合前缀，则从其最后 `EndKey` 续接，并以已有 region 数决定轮询起点。
- `AddRoundRobinRegions(regionCount, storeIDs)`：要求从空布局开始；单 region 直接生成 `[空, 空)`，多个 region 则生成补零的 `k01`、`k02` 等 split key，再复用 `AddRegionsBySplitKeys`。
- `appendRegion(...)`：私有不变量守门点，成功时克隆旧布局并压入新 `RegionBoundary`。
- `cloneRegionBoundaries(...)`：私有深拷贝函数，逐项复制两个键向量及 `StoreID`。

本文件没有模块级常量、struct、enum、trait、`impl` 或条件编译项。

## 执行流程

典型调用链如下：

1. 测试用 `StoreIDRange(1, 3)` 准备 store 列表，并创建 `AddRoundRobinRegions(5, stores)` 等选项。
2. `BuildRegionLayout` 从空布局开始，按传入顺序调用每个闭包；任一闭包返回 `Err` 时，`?` 立即结束整条链。
3. `AddRoundRobinRegions` 验证数量、store 列表和空初始布局。数量大于一时，根据 `regionCount - 1` 的十进制位数计算宽度（至少为 2），生成有序 split key，然后调用 `AddRegionsBySplitKeys`。
4. `AddRegionsBySplitKeys` 克隆已有边界，确定续接起键与轮询下标；每个 split key 都通过 `appendRegion` 形成 `[startKey, splitKey)`，最后形成 `[lastSplitKey, 空)`。
5. `appendRegion` 按字节检查新段，深拷贝旧列表后追加新值。split key 无序、重复或倒序会在这里因 `start >= end` 失败。
6. `BuildRegionLayout` 确认结果非空且最后 `EndKey` 为空，再深拷贝交付调用方。
7. `harness.rs::newLocalTestHarness` 把布局传给 `NewPDSimWithTestContext`；`pd_sim.rs` 再验证整个序列，并为每一项确保 store、构造 fakecluster region、设置单 store peer。

`AddRegionsBySplitKeys` 的空 `splitKeys` 仍会创建一个从当前起点到正无穷的尾段。已有布局若最后一段已经闭合，该函数会明确拒绝继续追加。

## 数据与状态

构建过程没有全局状态。每个选项通过所有权接收 `Vec<RegionBoundary>`，成功时返回新向量；捕获的 `String`、`Vec<String>` 和 `Vec<u64>` 在构造选项时移入闭包。`StartKey`/`EndKey` 始终以原始字节保存，特别是从已有布局续接时直接克隆 `last.EndKey`，不会经有损 UTF-8 转换。

主要不变量是：内置选项构造的首段从空键开始，相邻段满足 `prev.EndKey == next.StartKey`，非空 `EndKey` 满足 `StartKey < EndKey`（字节字典序），且 store ID 非零；完整布局最终以空 `EndKey` 结束。`cloneRegionBoundaries` 使返回值不与输入边界的键缓冲共享所有权，虽然 Rust 所有权本身已阻止普通可变别名，这仍保持了与 Go 深拷贝实现一致的值语义。

轮询状态只存在于一次闭包调用内：`storeIndex` 从 0 开始，或在续接布局时从 `result.len() % storeIDs.len()` 开始。生成的 Region 数等于 `splitKeys.len() + 1`；自动模式下等于 `regionCount`。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::types::RegionBoundary`；本文件不直接使用 `Cargo.toml` 中的外部 crate。所属 [`Cargo.toml`](Cargo.toml) 将该包声明为 library、porting kind 为 `library`，Go 包映射为 `br/pkg/utiltest/crr`；crate 还依赖 stream、streamhelper、fakecluster、`rand` 和 `serde` 等，但这些依赖由相邻模块使用，不是 builder 的直接依赖。

RustCodeGraph 的符号轨迹显示：

- `BuildRegionLayout -> cloneRegionBoundaries`。
- `AddRoundRobinRegions -> AddRegionsBySplitKeys`，单 region 分支还直接调用 `appendRegion`。
- `AddRegionsBySplitKeys -> appendRegion`；`appendRegion -> cloneRegionBoundaries` 并实例化 `RegionBoundary`。
- `AddRegion -> appendRegion`。
- 已索引的直接 Rust 调用者集中在 [`builder_test.rs`](builder_test.rs)、[`parity_test.rs`](parity_test.rs) 和 [`harness_test.rs`](harness_test.rs)。生产测试夹具链的间接消费者是 `harness.rs` 与 `pd_sim.rs`：它们接收这里产出的 `Vec<RegionBoundary>`，但不在源码中直接调用 builder 函数。

因此它在完整 CRR 测试应用中的位置是“静态拓扑描述生成器”，下游才负责目录、存储、事件通道、checkpoint 和 fakecluster 生命周期。

## 错误处理与边界

错误以英文 `String` 返回，没有自定义错误枚举或错误链。主要失败条件如下：

- `BuildRegionLayout`：没有生成 region；最后一个 region 的 `EndKey` 非空；或任一选项失败。
- `AddRegionsBySplitKeys`：store 列表为空；已有布局已经以空 `EndKey` 闭合；split key 导致不连续或非法区间；所选 store ID 为零。
- `AddRoundRobinRegions`：`regionCount <= 0`、store 列表为空、输入布局非空；后续生成也可能传播 `appendRegion` 错误。
- `appendRegion`：store ID 为零；首 region 起键非空；前段终点与新段起点不相等；非空终点不大于起点。

错误前的中间结果只在当前拥有的局部向量中，不会被返回，因此选项链对调用方表现为失败短路。错误消息用 `String::from_utf8_lossy` 仅作诊断显示，实际连续性和排序比较仍基于原始字节。

两个容易误判的边界是：第一，`StoreIDRange` 允许溢出后产生 0，只有真正把该 ID 用于 `appendRegion` 时才会拒绝；[`builder_test.rs`](builder_test.rs) 明确锁定了 `u64::MAX` 后环绕为 0 的行为。第二，只有 `AddRegionsBySplitKeys` 显式拒绝在闭合布局后追加；直接使用 `AddRegion("", "", id)` 时，`appendRegion` 的连续性条件仍可能成立。因此若要把“闭合后禁止任何追加”提升为全局契约，需要修改公共终检或统一追加入口，并新增回归测试，不能只依赖当前模块注释。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务。`BuildRegionLayout` 在当前线程按顺序消费选项，生命周期在函数返回时结束。`RegionLayoutOption` 的 `Send + Sync` 约束只说明捕获环境可安全跨线程，并不代表 builder 会并行执行选项。

所有输入向量、字符串和键字节都按所有权移动或克隆；错误返回时局部中间布局自动释放，成功返回时调用方独占结果。真实资源生命周期始于下游 [`harness.rs`](harness.rs)：它根据布局创建临时目录、存储、事件通道和 CRR 组件，并由 `TestHarness::Close` 清理；这些资源不由 builder 管理。

## 与 Go 版本的对应关系

[`builder.go`](builder.go) 与本文件在 API 角色、选项折叠顺序、split key 生成、store 轮询、逐段校验、错误文本和深拷贝意图上基本一一对应。Rust 的主要适配为：

- Go 的函数类型和可变参数对应 Rust 的 boxed `Fn`、`Vec<RegionLayoutOption>` 与显式 `Vec<u64>`。
- Go `nil` 切片在 Rust 中统一表示为空 `Vec`；因此 `StoreIDRange(..., 0)` 只能断言 `is_empty()`，不能保留 nil/非 nil 区别。
- Go 字符串能保存任意字节。Rust 的公开 split key/显式 key 参数仍是 UTF-8 `String`/`&str`，但从已有二进制 `EndKey` 续接时保持 `Vec<u8>`，避免有损转换；[`builder_test.rs`](builder_test.rs) 的二进制边界测试覆盖了这一移植差异。
- Rust 用 `wrapping_add` 显式复现 Go `uint64` 溢出；相关测试覆盖 `u64::MAX -> 0`。
- Go 返回 `error`，Rust 返回 `Result<_, String>`；当前没有结构化错误分类。

[`builder_test.go`](builder_test.go) 覆盖三个基础正常路径；Rust [`builder_test.rs`](builder_test.rs) 复刻这些断言并补充溢出与二进制续接。[`parity_test.rs`](parity_test.rs) 进一步覆盖空布局、未闭合末段、首段非空、零 region 与空 store 列表，并验证布局能进入 PDSim/harness 主链。

## 扩展指南

- 新增布局组合方式时，优先返回 `RegionLayoutOption` 并复用 `appendRegion`，以维持字节级连续性、区间顺序、store ID 与深拷贝语义；若必须接收任意二进制 split key，应设计 `Vec<u8>` API，而不是把字节经 UTF-8 字符串往返。
- 修改完整布局不变量时，需要同时审查 `BuildRegionLayout` 与 `pd_sim.rs::validateBoundaries`，明确约束是在构建阶段还是物化阶段保证。若禁止所有闭合后追加，需为 `AddRegion`/自定义选项路径补测试。
- 修改自动 split key 格式时，应保持字典序与固定宽度，否则 Region 顺序会变化；同步更新 `builder_test.rs`、`builder_test.go`（若 Go 契约也改变）及 `parity_test.rs`。
- 修改轮询算法时，要覆盖从空布局开始和续接未闭合前缀两种下标起点，并评估 store 分布兼容性；大布局的当前实现会在每次 `appendRegion` 深拷贝已有向量，呈二次复制成本，性能优化必须保留失败不泄漏半成品及 Go 值语义。
- 错误类型或文案变化可能影响按字符串断言的测试及 Go/Rust 对齐，应先检索消费者再调整。
- Rust 测试继续放在独立的 `builder_test.rs`/`parity_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 7032 个 Rust 文件；`files --filter br/pkg/utiltest/crr` 确认目标、Go 对照和独立测试均被索引。
- RustCodeGraph `node --file br/pkg/utiltest/crr/builder.rs --offset 1 --limit 260`：读取目标文件全部 210 行；`node BuildRegionLayout`、`node AddRegionsBySplitKeys`、`node AddRoundRobinRegions`、`node appendRegion`、`node RegionLayoutOption` 核对签名、源码与调用轨迹。
- RustCodeGraph 的独立 `callers/callees --file ...` 查询在本次环境中 30 秒内没有产生输出；调用边改由上述 `node` trail 与精确符号引用检索交叉确认，未据此推断额外调用者。
- crate 与入口：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。数据契约与下游物化：[`types.rs`](types.rs) 的 `RegionBoundary`、[`harness.rs`](harness.rs) 的 `newLocalTestHarness`、[`pd_sim.rs`](pd_sim.rs) 的 `NewPDSimWithTestContext`/`validateBoundaries`。
- Go 对照与测试：[`builder.go`](builder.go)、[`builder_test.go`](builder_test.go)。Rust 独立测试：[`builder_test.rs`](builder_test.rs)、[`parity_test.rs`](parity_test.rs)、[`harness_test.rs`](harness_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以任务指定命令确认本文档存在且恰有 11 个固定二级标题。
