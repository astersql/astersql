# `pkg/store/copr/range_diagnostics.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate；crate 根由 `pkg/store/copr/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/store/copr/lib.rs` 以 `pub mod range_diagnostics` 装配模块，并通过 `pub use range_diagnostics::*` 将其公开项重导出。它位于 coprocessor 请求按 TiKV Region/bucket 拆分和构建任务的诊断侧：输入是 `crate::key_ranges::{KeyRange, KeyRanges}` 表示的半开键区间 `[start, end)`，空 `end` 统一表示正无穷。

该文件不负责发送请求、刷新 Region 缓存或执行 fallback。它只计算无副作用的诊断结果；调用方再决定排序、记录信息或继续自愈流程。当前 Rust 接线见 `pkg/store/copr/coprocessor.rs` 的 `ensure_monotonic_key_ranges`、`range_issue_stats`、`build_cop_tasks`，以及 `pkg/store/copr/region_cache.rs` 的 bucket fallback 分支。

## 核心职责

1. 统一比较有限 end 和代表 `+inf` 的空 end，避免普通字节序把空切片误当成最小值（`compare_range_end`）。
2. 判断两个半开区间的包含和相交关系，并把相邻异常分类为重复、包含、部分重叠或逆序（`range_contains`、`ranges_overlap`、`classify_pair`）。
3. 单次扫描 `KeyRanges`，统计相邻关系异常、非法边界和非末尾无限区间（`range_issues_for_key_ranges`）。
4. 为诊断日志计算整组区间的最小 start 与最大 end（`min_start_and_max_end_key_of_key_ranges`）。
5. 按固定优先级返回第一个越出 Region location 的区间、下标和稳定原因标签（`first_out_of_bound_key_range_in_location`）。

这些函数是诊断和防御性检查，不证明输入已覆盖完整键空间，也不修复区间本身。特别是 `range_issues_for_key_ranges` 只检查相邻对；调用方若需要规范化，必须另行排序或拆分。

## 主要符号

- `RangeIssueStats`：公开、可复制的六字段计数器，字段分别是 `duplicate`、`overlap`、`contain`、`out_of_order`、`invalid_bound`、`infinite_tail`。派生 `Default/Eq/PartialEq`，因此零值就是“无已识别问题”。
- `RangeIssueStats::is_empty(self) -> bool`：按值接收统计并与默认值比较；任一计数非零即返回 `false`。
- `compare_range_end(left, right) -> Ordering`：两个空 end 相等；仅左空时为大；仅右空时为小；否则使用字节切片词典序。
- `range_contains(outer, inner) -> bool`：要求 `outer.start <= inner.start` 且 `outer.end >= inner.end`，end 比较采用上述 `+inf` 规则。相等区间也满足包含，但 `classify_pair` 会先归为重复。
- `ranges_overlap(left, right) -> bool`：实现半开区间相交条件；有限端点与另一侧 start 相等时不相交，空 end 则不施加该侧上界。
- `classify_pair(...)`：私有分类器，优先级固定为“完全相等 → 任一方向包含 → 相交 → 逆序”。它只增加一个类别一次。
- `range_issues_for_key_ranges(...)`：公开统计入口。空集合返回零值；非空集合逐个验证 `start > end`（仅有限 end），再检查相邻关系。
- `min_start_and_max_end_key_of_key_ranges(...)`：空集合返回 `(None, None)`；非空时返回拥有所有权的 `Vec<u8>` 副本，最大 end 把空值视为 `+inf`。
- `first_out_of_bound_key_range_in_location(...)`：线性查找第一个问题，成功返回 `(index, range.clone(), reason)`，全部合法或空集合返回 `None`。

文件没有常量、trait、异步函数、条件编译项或本地测试模块。

## 执行流程

`range_issues_for_key_ranges` 的流程如下：

1. 用 `KeyRanges::ref_at(0)` 取得首项；没有首项立即返回零统计。
2. 若首项 end 非空且 `start > end`，增加 `invalid_bound`。`start == end` 是允许的空半开区间，不计为非法。
3. 对后续每项执行同样的边界检查。
4. 若前一项 end 为空，说明 `+inf` 后仍有区间，增加 `infinite_tail`；这一分支优先于相邻分类。
5. 否则仅在 `previous.end > current.start` 时调用 `classify_pair`。相接边界 `previous.end == current.start` 和正向间隙都不计问题。
6. 将当前项设为下一轮的前一项，最终返回累计统计。

`classify_pair` 在已知前一有限 end 越过当前 start 的条件下工作。完全相同只记 `duplicate`；非相同但一方覆盖另一方只记 `contain`；交叉但不包含只记 `overlap`；若当前区间整体位于前一区间之前、且两者不交叠，则记 `out_of_order`。因此各字段是相邻异常事件计数，不是异常区间去重后的数量。

`min_start_and_max_end_key_of_key_ranges` 从首项克隆初值，再线性扫描：start 用普通字节序取最小，end 用 `compare_range_end` 取最大。任何空 end 一旦成为最大值，后续有限 end 都不会替换它。

`first_out_of_bound_key_range_in_location` 对每项依次检查并在首个命中处返回，单个区间内的原因优先级为：start 小于 location start；start 大于等于有限 location end；range end 为无限但 location end 有限；有限 range end 超过有限 location end；有限区间 `start > end`。该顺序意味着同时满足多个条件时只暴露最先命中的原因。

## 数据与状态

所有状态都局限于函数栈：计数器、前一项借用、最小/最大键副本和循环下标。函数不修改输入 `KeyRanges`，没有全局变量、缓存、日志副作用或外部 I/O。

`KeyRange` 与 `KeyRanges` 的真实定义在 `pkg/store/copr/batch_request_sender.rs`：前者拥有 `Vec<u8>` 的 `start/end`，后者是 `Vec<KeyRange>` 新类型；`pkg/store/copr/key_ranges.rs` 再提供 `ref_at`、`iter` 等辅助方法。键按原始字节的词典序比较，不做 SQL 编码解析。

需要区分三种空值语义：空的 `KeyRanges` 表示没有区间；空 start 按普通字节序处理，通常是键空间起点；空 end 才表示 `+inf`。`min_start_and_max_end_key_of_key_ranges` 用外层 `Option` 区分“无区间”和“存在一个无限 end 的区间”。

## 依赖与调用关系

直接 Rust 依赖只有标准库比较类型和同 crate 的 `KeyRange/KeyRanges`；本文件没有直接使用 `Cargo.toml` 中的网络、异步或 TiKV client 依赖。

已核验的上游调用关系：

- `pkg/store/copr/coprocessor.rs::ensure_monotonic_key_ranges` 调用 `range_issues_for_key_ranges`；统计非空时按 `(start, end)` 原地重排输入。该路径随后由 `build_cop_tasks` 使用，因此诊断可影响 row hints 是否清空，但排序动作不在本文件。
- `pkg/store/copr/coprocessor.rs::range_issue_stats` 是统计入口的薄公开包装。
- `pkg/store/copr/coprocessor.rs::build_cop_tasks` 在 `skip_buckets` 路径调用 `first_out_of_bound_key_range_in_location`；当前命中仅进入非致命空分支，注释说明后续 bounded-response retry 才是自愈机制。
- `pkg/store/copr/region_cache.rs::split_key_ranges_by_buckets` 的 fallback 路径调用 `range_issues_for_key_ranges`，结果当前保存在下划线变量中；真正动作是回退到按 Region 拆分。

`min_start_and_max_end_key_of_key_ranges` 在当前 Rust 仓库检索不到生产调用；它仍由 `lib.rs` 重导出。Go 版本在 coprocessor 错误诊断日志中使用对应函数，但不能据此宣称 Rust 已完成相同日志接线。

RustCodeGraph 能查询到本文件的 `RangeIssueStats`、`classify_pair` 和三个公开入口，但对这些节点执行 `callers/callees` 返回空边；以上接线因此由精确源码引用检索核验，而不是把空图误解为“无调用”。

## 错误处理与边界

本文件不返回 `Result`、不 panic，也不构造网络错误；异常输入被转换为计数或 `Option`。空集合在三个聚合入口中分别得到零统计、`(None, None)` 和 `None`。

关键边界包括：

- 有限 `start > end` 才计 `invalid_bound`；相等边界合法，空 end 永不按非法上界处理。
- 半开区间 `[a,b)` 与 `[b,c)` 不重叠。
- 前一项为无限尾时，每一个后继相邻位置各增加一次 `infinite_tail`，不再同时分类为包含或重叠。
- 正向区间之间存在 gap 不属于问题；本模块诊断顺序/交叠，不验证连续覆盖。
- location end 为空时，start 上界和 end 上界检查都放宽，但 start 仍不得小于 `location_start`，且有限 range 自身仍不得 `start > end`。
- `first_out_of_bound_key_range_in_location` 返回克隆，诊断方持有结果不会延长输入借用；代价与该 range 两个键的长度成正比。

## 并发与资源生命周期

全部 API 是同步纯计算，不创建线程、Tokio task、锁、通道、事务或网络资源。只读借用使同一 `KeyRanges` 可被多个线程并发读取，实际线程安全性由 `Vec<u8>` 组成的数据类型自然提供；本文件没有内部共享可变状态。

时间复杂度方面，三个聚合入口都是 `O(n)`；相邻判断和键比较还受键长度影响。`range_issues_for_key_ranges` 不克隆键；最小/最大函数只在候选更新时克隆；越界函数仅在命中时克隆第一个异常 range。所有临时值在函数返回时释放，返回的统计和键副本独立于输入生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/range_diagnostics.go`，共享的分类基础定义位于 `pkg/store/copr/coprocessor.go` 的 `rangeIssueStats`、`compareRangeEnd`、`rangeContains`、`rangesOverlap`、`classifyRangePair`。

Rust 的六项统计、分类优先级、空 end 为 `+inf`、相邻扫描、越界原因字符串及其判断次序均与 Go 对应逻辑一致。主要语言层差异是：Go 接受可能为 `nil` 的 `*KeyRanges`，Rust 接受必然有效的 `&KeyRanges`；Go 用 `(-1, zero range, "")` 表示未命中，Rust 用 `None`；Go 的最小/最大键返回底层切片引用语义，Rust 返回拥有所有权的克隆；Go 通过字符串类别和 `add` 更新计数，Rust 的私有分类器直接更新强类型字段。

接线尚非完全对等。Go 在 `coprocessor.go` 和 `region_cache.go` 的多个错误日志中记录统计、最小/最大边界和越界原因；当前 Rust 只在排序、bucket fallback 和 `build_cop_tasks` 的非致命检查中使用部分入口，且 `min_start_and_max_end_key_of_key_ranges` 尚无 Rust 生产调用。因此本文件提供了对应计算能力，但不能据此推断所有 Go 诊断日志都已移植。

## 扩展指南

新增异常类别时，应同时修改 `RangeIssueStats`、`classify_pair` 或 `range_issues_for_key_ranges` 的触发位置、`is_empty` 的零值语义说明，并核对 Go 的 `rangeIssueStats`/分类常量；若要求 Go/Rust 行为一致，不能只在日志调用方新增字符串。分类优先级属于兼容行为，调整顺序会改变同一相邻对落入的字段。

新增 location 边界原因时，应把检查插入 `first_out_of_bound_key_range_in_location` 的明确优先级位置，并保持 reason 字符串稳定，因为调用方可能把它作为日志或监控维度。若要从诊断升级为拒绝请求或触发重试，应在 `coprocessor.rs`/`region_cache.rs` 的调用方实现，避免让本文件混入 I/O 和策略副作用。

测试必须放在独立文件，不能内嵌到本源文件。最合适的直接单元测试位置是同目录新建 `range_diagnostics_test.rs` 并由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 接入；这会同时要求按仓库规则更新构建元数据。当前可扩展的间接回归位于 `pkg/store/copr/coprocessor_test.rs`（`ensure_monotonic_key_ranges_sorts_only_invalid_input`）和 `pkg/store/copr/region_cache_test.rs`（重叠、无限端点、越界后 fallback 场景），Go 对照回归位于 `pkg/store/copr/coprocessor_test.go` 与 `pkg/store/copr/region_cache_test.go`。

建议直接测试覆盖：六个分类计数各自及优先级；相接/gap 不计异常；多个无限尾后继的计数；空输入与空 start/end；所有五个越界 reason 及多原因优先级；最大 end 的 `+inf` 语义。性能敏感改动应保持单次线性扫描，避免为诊断复制整组 ranges。

## 验证依据

- 目标实现：`pkg/store/copr/range_diagnostics.rs`；逐项核对了结构体、方法、五个公开函数和私有 `classify_pair`。
- 类型与集合操作：`pkg/store/copr/batch_request_sender.rs` 的 `KeyRange/KeyRanges`，以及 `pkg/store/copr/key_ranges.rs` 的 `ref_at/iter` 等实现。
- crate 边界：`pkg/store/copr/Cargo.toml` 与 `pkg/store/copr/lib.rs`。
- Rust 调用方：`pkg/store/copr/coprocessor.rs` 的 `ensure_monotonic_key_ranges`、`range_issue_stats`、`build_cop_tasks`；`pkg/store/copr/region_cache.rs` 的 bucket fallback。
- Go 对照：`pkg/store/copr/range_diagnostics.go`、`pkg/store/copr/coprocessor.go`、`pkg/store/copr/region_cache.go`。
- 测试证据：没有发现直接引用本文件公开符号的独立 Rust 测试；间接行为证据来自 `pkg/store/copr/coprocessor_test.rs` 和 `pkg/store/copr/region_cache_test.rs`，对应 Go 场景来自 `pkg/store/copr/coprocessor_test.go`、`pkg/store/copr/region_cache_test.go`。该事实是覆盖限制，不等同于本任务运行过代码测试。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query` 定位到 `RangeIssueStats`（第 26 行）、`classify_pair`（第 71 行）、`range_issues_for_key_ranges`（第 84 行）、`min_start_and_max_end_key_of_key_ranges`（第 108 行）、`first_out_of_bound_key_range_in_location`（第 128 行）。这些节点的 `callers/callees` 查询返回空数组，故调用关系另以源码引用核验。
- 本任务是纯文档分析，依计划未运行 Cargo 或代码测试；交付前仅执行任务指定的 11 章节结构验证，并人工检查没有把 Go 已接线能力写成 Rust 现状。
