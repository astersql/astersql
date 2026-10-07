# `br/pkg/restore/split/region.rs`

源码：[region.rs](./region.rs)

## 文件定位

该文件属于 Cargo 包 `astersql-br-pkg-restore-split`。包入口 `br/pkg/restore/split/lib.rs` 通过 `#[path = "region.rs"] pub mod region` 装配模块，并用 `pub use region::*` 把这里的符号提升到 crate 根。这个 crate 对应 Go 包 `br/pkg/restore/split`，其职责是在 BR 恢复写入 SST 前提供 Region 扫描、分裂和 scatter 辅助；`region.rs` 是其中很小但基础的数据模型与键区间判定层。

文件只依赖 crate 内的 `stubs::metapb`，没有条件编译项，也不发起 PD/RPC 请求。`br/pkg/restore/split/Cargo.toml` 将本包声明为 library，入口为 `lib.rs`，未声明 feature；因此这里的 `RegionInfo` 会被 `client.rs`、`split.rs`、`splitter.rs`、`mock_pd_client.rs` 以及包外的恢复逻辑复用，而不是独立运行。

## 核心职责

1. 用 `RegionInfo` 聚合一个 Region 的元数据、leader、pending peers 和 down peers，形成 split/scatter 链路统一传递的状态对象。
2. 用 `RegionInfo::ContainsInterior` 判断键是否严格位于 Region 内部。区间规则是 `start < key < end`，其中空 `end` 表示无穷上界；特别排除 `start`，避免把已经是 Region 边界的键再次当作分裂点。
3. 用 `beforeEnd` 集中表达“严格小于结束键，或结束键为空”的 PD Region 上界约定。
4. 用 `RegionInfo::ToZapFields` 提供 nil-safe 的展示入口。不过当前 Rust 版本接的是本 crate 的日志桩并返回 `String`，不能等同于 Go 版本真正的 `zap.Field`。

该文件不负责 Region 扫描、重试、分裂 RPC、scatter、epoch 校验或并发调度；这些行为在相邻的 `client.rs`、`split.rs` 和 `splitter.rs` 中实现。

## 主要符号

- `pub struct RegionInfo`：派生 `Clone`、`Debug`、`Default`，四个字段均公开。
  - `Region: Option<metapb::Region>`：Region ID、半开范围、epoch 和 peers 等元数据。使用 `Option` 表达 Go 指针可能为 nil。
  - `Leader: Option<metapb::Peer>`：当前 leader；本文件不校验它是否属于 `Region.Peers`。
  - `PendingPeers: Vec<metapb::Peer>` 与 `DownPeers: Vec<metapb::Peer>`：PD 返回的异常 peer 集合；本文件只保存，不解释或修改。
- `pub fn RegionInfo::ContainsInterior(&self, key: &[u8]) -> bool`：读取 `Region` 的起止键并执行内部键判定。`Region == None` 时按 Go protobuf nil getter 的结果使用两个空切片，所以空键返回 `false`，任意非空键返回 `true`。
- `pub fn RegionInfo::ToZapFields(region: Option<&RegionInfo>) -> String`：关联函数而非 `&self` 方法。只有外层和内层 `Region` 都存在时才调用 `stubs::logutil::Region`；其余情况返回空字符串。
- `pub fn beforeEnd(key: &[u8], end: &[u8]) -> bool`：若 `end` 为空则无条件通过，否则要求 `key < end`。比较采用 Rust 字节切片的字典序，与 Go `bytes.Compare` 的用途一致。

文件没有模块级常量、enum、trait、类型别名或私有函数；三个 API 都是公开符号，但通过 `lib.rs` 的 crate 级 `allow` 保留了 Go 风格的字段和函数命名。

## 执行流程

`ContainsInterior` 的流程如下：

1. 若 `self.Region` 为 `Some`，从 `metapb::Region::GetStartKey` 和 `GetEndKey` 借用起止键；若为 `None`，使用 `(&[], &[])` 模拟 Go 生成 protobuf getter 对 nil receiver 返回零值的语义。
2. 先检查 `key > start`。等于起点或位于起点之前都不是内部键。
3. 再调用 `beforeEnd(key, end)`；有界 Region 要求严格小于 `end`，无界 Region（空 `end`）通过上界检查。
4. 两个条件都满足才返回 `true`。由此形成开区间 `(start, end)` 的“可分裂点”语义，而 Region 自身的覆盖范围通常仍描述为 `[start, end)`。

生产主链中，`split.rs::getSplitKeysOfRegions` 先编码有序分裂键、显式跳过空键和等于 `StartKey` 的键，再调用 `ContainsInterior` 将剩余键归入 `Region.Id` 对应的批次。`client.rs::splitKeys` 扫描覆盖范围后调用该映射，再按 Region 执行 `splitWaitAndMaybeScatter`。另一个直接调用点在 `mock_pd_client.rs::SplitWaitAndScatter`：它遍历内存 Region 集合，找到包含分裂键的目标 Region 后构造新的左右 Region，用于不依赖真实 PD 的行为测试。

`ToZapFields` 的流程只有一次 Option 链：`Option<&RegionInfo>` 经 `and_then` 取内部 `Region`，存在则格式化，不存在则返回空字符串。当前代码搜索未发现生产调用点，独立测试 `region_test.rs` 覆盖了三种输入。

## 数据与状态

`RegionInfo` 拥有 `metapb::Region`、`Peer` 及 peer 向量，不保存引用，因而 clone 会复制 Region 键和 peer 集合。`Default` 产生 `Region == None`、`Leader == None` 和两个空向量；调用方必须区分“缺失 Region”与“存在但字段为零值的 Region”。

本 crate 的 `stubs.rs::metapb::Region` 是最小迁移结构，包含 `Id`、`StartKey`、`EndKey`、可选 `RegionEpoch` 和 `Peers`；`GetStartKey`/`GetEndKey` 只借用内部 `Vec<u8>`。因此 `ContainsInterior` 不分配、不复制键，也不改变任何状态。

键比较是原始字节的字典序比较。这里不负责 memcomparable 编码：`split.rs::getSplitKeysOfRegions` 会在调用前通过 `codec::EncodeBytesExt` 把业务键变换到与 Region 边界一致的编码域。把未编码键与已编码边界混用会得到错误归属，这是调用方必须维持的不变量。

## 依赖与调用关系

- 模块装配：`br/pkg/restore/split/lib.rs` 声明并公开再导出 `region`，同时把 `region_test.rs` 作为独立 `#[cfg(test)]` 模块挂载。
- 下游依赖：`RegionInfo` 和两个方法只直接依赖 `stubs.rs::metapb::{Region, Peer}`、`Region::{GetStartKey, GetEndKey}` 以及 `stubs.rs::logutil::Region`。`beforeEnd` 只使用标准库切片比较和 `is_empty`。
- 直接调用边：`ContainsInterior -> beforeEnd`；`ContainsInterior -> metapb::Region::{GetStartKey, GetEndKey}`；`ToZapFields -> stubs::logutil::Region`。
- 直接上游：`split.rs::getSplitKeysOfRegions -> RegionInfo::ContainsInterior`；`mock_pd_client.rs::SplitWaitAndScatter -> RegionInfo::ContainsInterior`。`region_test.rs` 和 `parity_test.rs` 也直接验证该函数与 helper。
- 类型消费者：`client.rs` 的 `SplitClient`/`PDClient` 接口和分裂实现、`splitter.rs` 的执行与缓存结构、`split.rs` 的 Region 一致性与分裂流程，以及 `br/pkg/restore/misc.rs::regionScanner` 都传递或缓存 `RegionInfo`。
- Cargo 边界：`Cargo.toml` 没有外部 protobuf 或 zap 依赖；当前 metapb 与日志行为来自 crate 内桩。manifest 中的四个直接依赖用于同 crate 的其他模块，不是 `region.rs` 的直接导入。

RustCodeGraph 的文件节点将 `region.rs` 标为被 12 个文件使用，并能定位 `RegionInfo`、`ContainsInterior`、`beforeEnd`、`ToZapFields` 的 Rust/Go 对照定义；精确调用边查询在本次会话中未在 60 秒内返回，所以上述直接调用边又以目标目录内的精确符号搜索和源码上下文复核。

## 错误处理与边界

本文件没有 `Result`、panic、日志错误或重试路径；所有函数均为确定性的同步计算。需要特别保留的边界语义是：

- `key == start`：返回 `false`，避免重复分裂已有起点。
- `key == end`：当 `end` 非空时返回 `false`，因为终点属于下一个 Region。
- 空 `end`：视为正无穷，任何满足 `key > start` 的键通过。
- `Region == None`：按空起止键处理；空键返回 `false`，非空键返回 `true`。这与 Rust 常见的“缺失即拒绝”不同，但由 `region_test.rs::contains_interior_matches_go_region_getter_semantics` 明确锁定为 Go 兼容行为。
- `ToZapFields(None)`、`ToZapFields(Some(&RegionInfo::default()))`：均返回空字符串；存在零值 Region 时返回日志桩的 `"region"`。

该实现不验证 Region ID、epoch、peer/leader 一致性，也不检测边界倒置（`start > end`）。相关错误应由 Region 扫描一致性检查、PD 返回验证或具体调用链处理，不能在此 helper 中擅自改变语义。

## 并发与资源生命周期

该文件不创建线程、任务、通道、锁、事务、文件句柄或网络连接。`ContainsInterior` 和 `ToZapFields` 都只在调用期间借用输入；借用结束后不保留引用。`beforeEnd` 同样是纯函数。

`RegionInfo` 本身没有内部同步。其字段公开且拥有数据，调用方可移动、clone 或在拥有可变引用时修改它。并发共享策略由上层决定：例如 `mock_pd_client.rs` 把 Region 集合放入锁中，`client.rs` 在重试流程中用 `Mutex<Vec<RegionInfo>>` 汇总结果；这些锁的生命周期不由本文件管理。若未来给 `RegionInfo` 增加共享可变状态，会影响现有 clone/value 语义和上层并发假设，应避免在没有全链路审查时这样扩展。

## 与 Go 版本的对应关系

直接对照文件为 `br/pkg/restore/split/region.go`：

- 字段一一对应：Go 的 `*metapb.Region`、`*metapb.Peer` 和 peer 指针切片在 Rust 中分别表示为 `Option<Region>`、`Option<Peer>` 和拥有值的 `Vec<Peer>`。
- `ContainsInterior` 保留 Go 的两次严格比较：`bytes.Compare(key, start) > 0` 与 `beforeEnd`。Rust 使用切片字典序运算符实现相同排序意图。
- `beforeEnd` 保留 Go 的“`key < end || len(end) == 0`”顺序无关布尔语义，空结束键仍代表无限上界。
- nil Region 的行为被 Rust 显式兼容：Go 代码经生成 protobuf getter 得到空键，Rust 在 `None` 分支返回空切片；`region_test.rs` 专门覆盖了这一点。
- 日志接口尚未完全等价。Go `(*RegionInfo).ToZapFields() zap.Field` 在 nil receiver 时返回 `zap.Skip()`，否则调用真实 `logutil.Region`；Rust 使用关联函数 `ToZapFields(Option<&RegionInfo>) -> String`，并调用返回固定占位串的 `stubs::logutil::Region`。它只保留 nil-safe/有无值分支，不具备结构化 zap 字段的类型和内容。

因此，键区间判定已有明确的 Go 对齐测试；日志格式化仍是迁移期桩能力，扩展文档或调用方不得宣称其已经完整移植。

## 扩展指南

- 若修改 Region 内部键规则，优先改 `ContainsInterior`/`beforeEnd`，同时更新独立的 `region_test.rs` 和 `parity_test.rs`；还要复核 `split.rs::getSplitKeysOfRegions` 与 `mock_pd_client.rs::SplitWaitAndScatter`，因为它们依赖“起点不能再分裂、空 end 无上界”的不变量。
- 新增边界测试至少覆盖空键、等于 start、start/end 之间、等于 end、超过 end、空 end 和 `Region == None`。测试逻辑继续放在独立测试文件中，不要内嵌到 `region.rs`。
- 若替换 `stubs::metapb` 为真实 protobuf 类型，要验证 getter 的 nil/默认值、clone 成本、peer 表示以及键切片生命周期；不能只做类型名替换。
- 若完成日志移植，应让 `ToZapFields` 的返回类型和字段内容与 Go `zap.Field` 等价，并同步调整全部调用方和 `to_zap_fields_is_nil_safe`。在此之前，应把空字符串视为“跳过日志字段”的桩约定。
- 若给 `RegionInfo` 增加字段，应同步 Go 对照、所有构造字面量、client/mock 转换逻辑和独立测试；公开字段及 `Default` 是当前调用方广泛依赖的兼容面。
- 性能上应维持热路径无分配的键判定。不要为了便利把 `GetStartKey`/`GetEndKey` 转成新 `Vec`，也不要在逐键映射中增加日志或线性复制。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/restore/split` 确认 `region.rs`、Go 对照、独立测试和调用模块均被索引；`node --file br/pkg/restore/split/region.rs --offset 1 --limit 100` 读取了 59 行完整源码，并报告该文件被 12 个文件使用；`query` 分别定位了 `RegionInfo`、`ContainsInterior`、`beforeEnd`、`ToZapFields` 的 Rust/Go 定义。
- RustCodeGraph 限制：`callers ContainsInterior --file br/pkg/restore/split/region.rs` 连续 60 秒无输出后被终止，没有把未返回结果当作调用关系证据；随后通过 `rg` 精确搜索并读取上下文确认生产调用点。
- 已读生产文件：`br/pkg/restore/split/region.rs`、`lib.rs`、`Cargo.toml`、`stubs.rs`、`split.rs`、`client.rs`、`mock_pd_client.rs`，以及包外消费者 `br/pkg/restore/misc.rs` 的 Region 缓存片段。
- 已读 Go 对照：`br/pkg/restore/split/region.go`；并用同目录 Go/Rust 精确符号搜索核对 `RegionInfo` 在 split/client/splitter 链中的用途。
- 已读独立测试：`br/pkg/restore/split/region_test.rs`；补充核对 `br/pkg/restore/split/parity_test.rs` 的半开区间与内部键断言。任务是纯文档分析，按计划未运行 Cargo。
- 结构验收使用任务指定命令，要求目标文件存在且上述固定二级标题恰好出现 11 个；提交前另做链接/路径与事实人工复核。
