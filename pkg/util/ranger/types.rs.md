# `pkg/util/ranger/types.rs`

## 文件定位

`types.rs` 是 `astersql-util-ranger` crate 的范围值模型和基础区间算法实现。crate 根 `pkg/util/ranger/lib.rs` 通过 `#[path = "types.rs"] mod types_impl` 加载本文件，再用 `pub use types_impl::*` 对外导出；因此调用方通常看到的是 `astersql_util_ranger::Range`、`Ranges` 和 `MutableRanges`，而不是私有模块名 `types_impl`。

该文件处在 SQL 谓词到物理扫描边界之间：`points.rs`、`detacher.rs`、`ranger.rs` 负责产生和组合范围，本文件定义范围如何表示、比较、求交、识别点查/全扫描、格式化及编码成 KV 左闭右开边界。实际调用证据包括：`pkg/util/ranger/detacher.rs::getCNFItemRangeResult` 用 `Range::IsPoint` 评估 CNF 候选，`mergeTwoCNFRanges` 用 `Ranges::Subset`/`IntersectRanges` 合并候选；`pkg/planner/util/path.rs::OnlyPointRange` 判断访问路径能否按点查处理；`pkg/planner/core/operator/physicalop/physical_index_scan.rs::{IsFullScan,PlanCacheTP,IsPointGetByUniqueKey}` 判断扫描类型；`pkg/planner/cardinality/pseudo.rs::getPseudoRowCountByIndexRanges` 用 `PrefixEqualLen` 估算组合索引选择率。

crate 边界由 `pkg/util/ranger/Cargo.toml` 确认：包名为 `astersql-util-ranger`，Go 对照包元数据为 `pkg/util/ranger`；本文件直接用到经 crate 根转出的 `expression` 能力（`codec`、`collate`、`errctx`、`errors`、`types`），以及 `astersql-kv`、`astersql-planner-planctx` 和子 crate `astersql-util-ranger-context`。本文件没有条件编译项；测试由 `lib.rs` 在 `cfg(test)` 下把独立的 `types_test.rs` 接入。

## 核心职责

- 用 `Range` 表示一段多列扫描区间：`LowVal`/`HighVal` 保存 Datum 边界，`LowExclude`/`HighExclude` 保存开闭属性，`Collators` 决定各列比较规则。
- 用 `Ranges(Vec<Range>)` 表示范围集合，并提供集合级内存估算、子集检查和笛卡尔式两两求交。
- 用 `MutableRanges`/`RangeRebuildContext` 表达计划缓存重建边界。普通 `Ranges` 的 `Rebuild` 是空操作，但接口允许规划器侧实现按执行上下文重建的范围对象。
- 提供点范围、仅 NULL、全范围、相等前缀等形态判定，为点查选择、扫描类型选择和统计估算服务。
- 把范围格式化为诊断文本或按脱敏策略显示，并把 Datum 边界编码成 KV 层所需的左闭右开字节区间。
- 实现多列、不同宽度、不同开闭属性范围的比较与求交；较短边界通过无穷哨兵补齐后再逐列比较。
- 估算范围对象的内存占用，供计划及范围构建的内存核算使用。

本文件不负责从表达式构造范围、不访问存储、不执行扫描，也不管理计划缓存本身；这些职责分别在 ranger 的构造/拆分模块、planner 和 executor/store 层。

## 主要符号

- `trait RangeRebuildContext: Send + Sync`：对象安全的重建上下文边界；所有同时实现 `planctx::PlanContext + Send + Sync` 的类型自动实现该 trait。
- `trait MutableRanges`：公开三个操作：`Range() -> Ranges` 读取当前范围，`Rebuild(&mut self, &dyn RangeRebuildContext)` 重建，`CloneForPlanCache()` 为缓存复制。`Ranges` 是本文件中的普通实现。
- `struct Ranges(pub Vec<Range>)`：拥有 `Range` 的新类型包装，派生 `Clone`/`Default`，实现 `Deref`、`DerefMut`、按值和按引用的 `IntoIterator`，并实现 `MutableRanges`。
- `Ranges::{MemUsage,Subset,IntersectRanges}`：分别聚合内存估算、检查两个范围列表的覆盖关系、计算两组范围的所有非空交集。`IntersectRanges` 用 `Option<Ranges>` 区分成功结果与 collator/比较失败；空交集的成功结果是 `Some(Ranges(vec![]))`。
- `struct Range`：核心公开数据结构。它手写 `Clone`，以便对每个 `Collator` 调用对象级 `Clone()`；`Default` 产生四个空容器/关闭排除标志的空结构。
- `Range::{Width,Clone,Equal,MemUsage}`：基础值操作。`Equal` 比较开闭标志与 Datum 内容，但不比较 collator；`MemUsage` 计入结构体、每个 trait-object 槽按 16 字节估算及 Datum 内容，明确忽略 collator 实例本体。
- `Range::{IsPoint,IsPointNonNullable,IsPointNullable,IsOnlyNull}`：点范围族。共同核心是私有 `isPoint`：要求两端等宽、逐列按对应 collator 相等、没有不允许的无穷边界、两端闭合，并由参数决定 `[NULL,NULL]` 是否视为点。
- `Range::IsFullRange` 与 `HasFullRange`：识别全扫描区间。无符号整数 handle 只接受单列并把 `0..u64::MAX` 识别为边界；普通路径还接受一端 NULL、另一端无穷的形态，但拒绝 `[NULL,NULL]`。
- `Range::{String,Redact}`、`string`、`formatDatum`、`dealWithRedact`：生成 `[low,high]`/`(low,high)` 形式的诊断文本，特殊显示 `NULL`、`±inf`，并支持原文、`?` 或 `‹value›` 三类脱敏策略。源码明确注明 `String` 不应用作产品协议。
- `Range::Encode`：用 `codec::EncodeKey` 分别编码两端；低端开区间时推进 `PrefixNext`，高端闭区间时推进 `PrefixNext`，把数学区间转换为 KV 的左闭右开边界。传入的两个缓冲区先 `clear`，因此可复用容量。
- `Range::{Subset,IntersectRange}`：单范围覆盖与求交入口。`Subset` 检查宽度、collator 类型、开闭兼容和边界前缀；`IntersectRange` 先排除互不相交，再选择较大的低端和较小的高端，并保留较细粒度边界的 collator 数量。
- `Range::PrefixEqualLen`：返回低/高边界从首列开始相等的列数，供组合索引选择率估算。
- `const EmptyRangeSize`：`size_of::<Range>()` 的运行平台相关值，是内存估算基数。
- 私有 `isBoundaryValue`、`extendBound`、`compareLexicographically`、`prefix`、`checkCollators`：分别处理边界哨兵、宽度补齐、带开闭语义的字典序比较、前缀相等和 collator 类型一致性。

## 执行流程

1. 范围构造模块把表达式端点变成 `Datum`，并为每列装配对应 `Collator`，形成 `Range`/`Ranges`。`pkg/util/ranger/lib.rs` 将这些类型与 `points`、`detacher`、`ranger` 的构造 API 放在同一公开 crate 表面。
2. 优化阶段消费范围形态：`detacher.rs` 用 `IsPoint` 比较 CNF 候选，并在优化器开关 `Fix54337` 打开时先用 `Subset` 选择更窄集合；互不覆盖时调用 `IntersectRanges`。若求交返回 `None`，调用方退回启发式选择，而不是采用不可信交集。
3. 单范围求交 `IntersectRange` 选择两个输入中更宽的列数和相应 collator 列表；它四次调用 `compareLexicographically`：两次判断是否分离、一次选最终低端、一次选最终高端。不同宽度的边界先由 `extendBound` 以 `MinNotNullDatum`/`MaxValueDatum` 补齐；同值时再用“低端或高端、开或闭”打破平局。
4. 访问路径和物理计划据此分类：`OnlyPointRange` 分别使用 nullable/non-nullable 点判断；物理索引/表扫描用 `IsFullRange` 判断 full scan；唯一索引还要求单个、等宽、非 NULL 点范围才视为 point get。
5. 统计估算读取同一结构：`GetRowCountByIndexRanges` 用 `IsFullRange` 决定是否可走全范围快捷路径，`getPseudoRowCountByIndexRanges` 用 `PrefixEqualLen` 计算组合索引等值前缀。
6. 生成存储请求时，`Encode` 将 Datum 边界编码为字节；开低端向后推进，闭高端也向后推进，从而让最终字节区间统一为 `[start,end)`。编码错误先交给 `errctx::Context::HandleError`，由上下文决定返回错误还是吞掉并继续使用空编码结果。
7. 诊断与资源核算路径分别使用 `String`/`Redact` 和 `MemUsage`；例如物理索引扫描的 EXPLAIN 范围文本直接映射 `Range::String`，而计划内存估算累计每个 `Range::MemUsage`。

## 数据与状态

`Range` 完全拥有边界 Datum 与 collator trait object，没有借用外部缓冲。有效对象依赖调用方维持这些结构不变量：低/高边界通常等宽；涉及逐列比较时 `Collators` 至少覆盖被访问的列；同一逻辑列的两个范围应使用相同 collator 类型。部分方法会显式检查等宽或 collator，另一些方法（如 `PrefixEqualLen`、`isPoint`、`checkCollators`）按既有 Go 调用契约直接索引，构造不完整对象可能触发 Rust 越界 panic。

`Ranges` 拥有 `Vec<Range>`，不同于 Go 的 `[]*Range`：Rust 集合不能保存 nil `Range` 元素。Go nil slice 在部分 API 语义上改用 `Option<Ranges>`/`Option<Box<dyn MutableRanges>>` 表达；但当前 `Ranges::CloneForPlanCache` 对任何已存在的 `Ranges` 都返回 `Some`，因为 Rust 的 `Vec` 自身没有 nil 状态。`Range::Equal` 的参数用 `Option<&Range>` 表示 Go 的 nil 指针。

开闭状态只存于 `LowExclude`/`HighExclude`；无穷不是额外枚举，而由 `KindMinNotNull`、`KindMaxValue` 以及特定整型极值在不同语境中承担。`compareLexicographically` 只克隆并补齐局部边界，不修改输入；`Encode` 消耗并复用输入 `Vec<u8>`；其余判断方法只读。`Clone` 深拷贝 Datum 容器和 collator 对象，避免计划缓存副本共享可变范围状态。

## 依赖与调用关系

上游调用分为四类：

- 范围构造与合并：`pkg/util/ranger/detacher.rs` 直接调用 `IsPoint`、`Subset`、`IntersectRanges` 和 `MemUsage`；`pkg/util/ranger/ranger.rs` 调用 `IsPoint` 并累计 Datum 内存。
- 规划器访问路径：`pkg/planner/util/path.rs::OnlyPointRange` 调用 nullable/non-nullable 点判断；`pkg/planner/core/operator/physicalop/{physical_index_scan.rs,physical_table_scan.rs}` 持有 `Ranges` 并调用 `IsFullRange`、`IsPointNonNullable`、`String` 和 `MemUsage`。
- 基数估算：`pkg/planner/cardinality/{row_count_index.rs,pseudo.rs}` 分别调用 `IsFullRange`、`IsPoint`、`PrefixEqualLen`。
- 计划数据模型：logical/physical scan、index join、selectivity 等多个 planner 结构直接把 `Range` 或 `Ranges` 作为字段，证明它是规划主链的共享值类型，而非 ranger 内部临时对象。

下游依赖为：`expression::types::Datum` 提供类型标签、取值、比较、相等与内存估算；`expression::collate::Collator` 决定字符串比较并提供 trait-object 克隆；`expression::codec::EncodeKey` 生成可排序 key；`expression::errctx::Context` 控制编码错误策略；`astersql-kv::Key::PrefixNext` 把端点推进为 KV 半开边界；`chrono_tz::Tz` 提供时间 Datum 编码时区；`planctx::PlanContext` 为重建上下文建立类型边界。

RustCodeGraph `status` 显示索引包含本文件并识别 56 个符号，文件节点报告本文件被 28 个文件使用。精确的同名方法 `callers/callees` 查询未返回边，因此调用边进一步通过上述 Rust 源文件的精确方法调用检索核验；不能把空图结果解释成“没有调用者”。

## 错误处理与边界

- `Encode` 不用单一 `Result`，而返回 `(Option<Vec<u8>>, Option<Vec<u8>>, Option<GoError>)` 以保持 Go 三返回值形态。任一端编码错误经 `HandleError` 后若仍是错误，会立刻返回两个 `None` 和错误；若上下文消解错误，则该端以空字节继续。调用者必须检查第三项，不能仅解包前两项。
- `PrefixEqualLen` 在 Datum 比较失败时返回 `(0, Some(errors::Trace(error)))`，会丢弃错误发生前已匹配的前缀长度；`pseudo.rs` 将该错误继续向上传播。
- `IntersectRange` 把“无交集”表示为 `(None,None)`，把比较失败表示为 `(Some(Range::default()),Some(error))`；集合级 `IntersectRanges` 将 collator 不匹配或任意比较错误统一折叠为 `None`。其调用方 `mergeTwoCNFRanges` 因而只能知道求交失败并退回启发式，无法区分具体原因。
- `Subset`、`isPoint` 和 `prefix` 把 Datum 比较错误降级为 `false`；这是保守分类，不会把无法证明的范围误判为点或子集。
- `checkCollators` 比较 Rust collator 具体类型的 `TypeId`，对应 Go 的 collator 相等检查，但不比较 trait object 内部配置。它还假设双方 collator 数量覆盖 `length`。
- 空集合有特别语义：`Ranges::Subset` 中，空 self 只属于空 super 集合；非空 self 对空 super 集合返回 true，因为空 super 在此 API 中代表“不受限”。这不是通常数学集合的空集语义，扩展时不能擅自改写。
- `IsOnlyNull` 只遍历 `LowVal` 长度并索引 `HighVal`；空边界会真空地返回 true，边界不等宽则可能越界。正常构造路径需要先维护范围结构不变量。
- `String` 是诊断格式，且源码注明不要用于产品逻辑；计划缓存中的现有字符串比较是特定诊断/配额判断接线，不应将其提升为稳定序列化协议。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络连接。`Range`/`Ranges` 的一般方法依赖 `&self` 或 `&mut self` 的 Rust 借用规则，不含内部可变性。

并发约束主要体现在重建接口：`RangeRebuildContext` 要求 `Send + Sync`，使上下文可安全作为跨线程边界的 trait object；`MutableRanges` 自身没有声明 `Send + Sync`，是否跨线程共享由具体持有者和实现决定。普通 `Ranges::Rebuild` 不保存上下文且立即成功。

资源均为内存所有权资源：`Range::Clone` 克隆 Datum 和 collator 对象；`IntersectRange` 为结果分配边界/Collator 容器并克隆所选数据；临时补齐向量在比较结束时释放；`Encode` 复用调用方传入缓冲的容量后把所有权随返回值交回。`MemUsage` 是估算值，不负责分配限额或释放，并且明确不计 collator 实例本体。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/ranger/types.go`，Rust 的结构、方法命名、分支顺序和注释基本逐段对应：`MutableRanges`、`Ranges`、`Range`、点/全范围判断、格式化、编码、内存估算、边界补齐、字典序比较、子集和交集均有同名 Go 实现。`pkg/util/ranger/Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向该 Go 包。

需要注意的语言映射差异：

- Go `Ranges` 是 `[]*Range`，Rust 是 `Vec<Range>`；nil slice/nil pointer 分别用 `Option` 在少数 API 边界表达，集合内部不容纳 nil 元素。
- Go 方法错误通常是 `error` 或 `(*Range,error)`；Rust 为贴近调用形态使用 `Option<GoError>` 及元组，而不是统一的 `Result`。
- Go collator 接口值可直接比较；Rust `checkCollators` 比较动态对象的 `TypeId`，`Range::Clone` 通过 `Collator::Clone` 克隆 trait object。
- Go `time.Location` 对应 `chrono_tz::Tz`，Go `kv.Key.PrefixNext` 对应 Rust `kv::Key(...).PrefixNext()`。
- Go `unsafe.Sizeof(Range{})` 对应 Rust `size_of::<Range>()`；两者的数值均与各自 ABI 有关，测试只验证本实现内部公式，不要求跨语言字节数相等。
- Rust `formatDatum` 为避免打印 Rust 枚举调试包装，显式提取浮点、bytes、string、ENUM、SET、JSON、binary literal/bit 的用户可见值；`types_test.rs::test_range_string_formats_mysql_named_values` 是 Rust 额外的格式兼容回归。

测试对照为独立文件 `pkg/util/ranger/types_test.rs` 与 `pkg/util/ranger/types_test.go`。Rust 测试复现 Go 的点范围、全范围、内存公式和大量交集案例，并额外检查多列交集保留粒度/Collator 数量。当前 Rust 独立测试未直接覆盖 `Encode`、`Subset`、`PrefixEqualLen`、`Redact`、`Equal`、`MutableRanges` 的全部分支；这些不能仅凭已有测试声称完全验证。

## 扩展指南

- 新增范围形态判断时，优先放在 `impl Range`，并明确 NULL、`MinNotNull`/`MaxValue`、整数极值、开闭端点和多列宽度语义；若是集合属性则放在 `impl Ranges`。同步扩展独立的 `pkg/util/ranger/types_test.rs`，并与 `types_test.go` 的对应语义核对，禁止把测试内嵌回生产文件。
- 修改比较或求交时必须同时检查 `extendBound`、`compareLexicographically`、`IntersectRange` 的组合不变量，至少覆盖交换输入对称性、相接但因开区间为空、不同列宽、较细粒度范围和 collator 不匹配。错误与空交集的 `Option` 区分是调用契约，不能无调用方迁移就改变。
- 新增 Datum 类型或格式时，需同步审查 `formatDatum` 的左右边界极值显示和脱敏行为；不要让 Rust `Debug` 表示泄露成用户诊断文本。若输出被用于 EXPLAIN，还要核对 `physical_index_scan.rs::ranges_to_string`。
- 修改 `Encode` 时必须维持 KV `[start,end)` 契约、缓冲复用、时区语义和 `errctx` 处理顺序；应在独立测试中覆盖低端开/闭、高端开/闭、编码错误被提升/消解两类策略，而不能只比较可读字符串。
- 扩展 `MutableRanges` 时应保留对象安全，并评估是否需要为 trait 本身增加 `Send + Sync`；普通 `Ranges` 的空操作重建不能被误认为所有动态范围都无需上下文。若计划缓存要求可空语义，需要明确设计 `Option`，不能用空 `Vec` 偷换 nil。
- 调整内存估算时同步审查 `EmptyRangeSize`、trait-object 槽位假设、Datum 容量和是否纳入 collator 本体；这会影响 ranger 配额及物理计划内存统计，存在性能和兼容性风险。
- 任何字段新增都要检查所有直接结构字面量调用者、`Clone`、`Default`、`Equal`、`MemUsage` 和计划缓存快照代码。公开字段被 planner 广泛构造，变更影响面不止本 crate。

## 验证依据

- RustCodeGraph：运行 `rustcodegraph status`，索引包含 7,032 个 Rust 文件；`rustcodegraph files --filter pkg/util/ranger` 列出 `types.rs`、模块同伴及独立测试；`rustcodegraph node --file pkg/util/ranger/types.rs --offset 1 --limit 500` 与 `--offset 492 --limit 500` 阅读完整 885 行和 56 个符号，文件节点报告 28 个使用文件。
- RustCodeGraph 符号查询：查询 `IntersectRange`、`HasFullRange`、`MutableRanges`、`IsPoint`，核对 Rust/Go 同名定义与签名；对精确符号执行 `callers/callees` 返回空结果，故未把它作为无调用者证据，而按技能的图未覆盖规则用精确源码调用检索补齐。
- 直接读取的边界/对照文件：`pkg/util/ranger/Cargo.toml`、`pkg/util/ranger/lib.rs`、`pkg/util/ranger/types.go`、`pkg/util/ranger/types_test.rs`、`pkg/util/ranger/types_test.go`。`pkg/util/ranger` 及其上层 `pkg/util` 未发现适用的 `doc.go`。
- 直接调用证据：RustCodeGraph 文件节点读取了 `pkg/util/ranger/detacher.rs`、`pkg/planner/util/path.rs`、`pkg/planner/core/operator/physicalop/physical_index_scan.rs`、`pkg/planner/cardinality/row_count_index.rs`、`pkg/planner/cardinality/pseudo.rs` 的相关区段；精确 `rg` 还核对了 ranger/planner 内的 `IsPoint`、`IsFullRange`、`Subset`、`IntersectRanges`、`PrefixEqualLen`、`MemUsage` 调用及 `Range`/`Ranges` 字段使用。
- 行为测试证据（仅阅读，按任务要求未运行 Cargo）：`types_test.rs` 覆盖字符串/点范围、MySQL ENUM/SET 格式、普通及 unsigned full range、单个及列表内存估算、范围列表求交、空交集、子集、重叠和多列粒度；Go 测试提供相同主干语义对照。
- 文档结构验证使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。该任务只新增文档，不修改 Rust、Go、Cargo 或只读的总计划，也不执行 Cargo。
