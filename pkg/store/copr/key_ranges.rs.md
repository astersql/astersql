# `pkg/store/copr/key_ranges.rs`

## 文件定位

本文件是 `astersql-store-copr` crate 中键区间集合的操作层，对应的真实源码为 [`key_ranges.rs`](./key_ranges.rs)。`KeyRange` 和 `KeyRanges` 本体定义在 `pkg/store/copr/batch_request_sender.rs`，本文件通过 `pub use crate::batch_request_sender::{KeyRange, KeyRanges}` 重导出它们，并为 `KeyRanges` 增加访问、切片、按键拆分、转换和展示能力。`pkg/store/copr/lib.rs` 公开 `key_ranges` 模块并再导出其符号，因此 crate 使用者可直接使用 `NewKeyRanges` 和相关类型。

`pkg/store/copr/Cargo.toml` 将该 crate 声明为 `astersql-store-copr`，并用 `package.metadata.porting.go-package = "pkg/store/copr"` 记录 Go 对照包。本文件自身只直接依赖 Rust 标准库的格式化 API 和 crate 内部类型；它不直接执行 RPC，而是为 Region 定位与 coprocessor 任务构建提供基础数据操作。

## 核心职责

- 对由有序半开区间 `[start, end)` 组成的 `KeyRanges` 提供安全的单项访问、显式边界校验的子区间拷贝和顺序遍历（`ref_at`、`at`、`slice`、`for_each`）。
- 按键空间边界将区间集分为左右两组，必要时把一个跨边界区间截断为两段（`split`）。这是 coprocessor 重试、剩余范围计算和 Region 边界处理的基础。
- 在 `KeyRanges` 和普通 `Vec<KeyRange>` 之间转换或整体替换内容（`to_ranges`、`to_pb_ranges`、`reset`）。
- 提供与 Go `%q` 字节串输出相容的 `Display` 实现，使不可见字符和非法 UTF-8 字节可稳定诊断。
- 保留 Go 命名的构造入口 `NewKeyRanges`，以降低移植代码的调用差异。

## 主要符号

- `KeyRange { start: Vec<u8>, end: Vec<u8> }`：在 `batch_request_sender.rs` 定义的半开键区间；空 `end` 在 `split` 及相关调用中表示无上界。
- `KeyRanges(pub Vec<KeyRange>)`：在 `batch_request_sender.rs` 定义的所有权集合。其 `new`、`len`、`is_empty`、`iter` 和 `into_sorted` 在定义文件中，本文件增加其余 Go 对齐操作。
- `KeyRanges::ref_at(index) -> Option<&KeyRange>`：借用访问，越界返回 `None`，不拷贝键字节。
- `KeyRanges::at(index) -> Option<KeyRange>`：通过 `ref_at(...).cloned()` 返回所有权副本；越界同样返回 `None`。
- `KeyRanges::slice(from, to) -> KeyRanges`：返回下标半开区间 `[from, to)` 的深拷贝。它用 `assert!` 要求 `from <= to <= len`。
- `KeyRanges::for_each(visit)`：按底层 `Vec` 顺序以借用传给 `FnMut`。
- `KeyRanges::split(key) -> (KeyRanges, KeyRanges)`：使用 `partition_point` 定位首个 `end` 为空或 `end > key` 的区间；若 `key > start`，将当前区间截成 `[start, key)` 和 `[key, end)`，否则仅在区间之间分组。
- `KeyRanges::to_ranges()` / `to_pb_ranges()`：当前两者都克隆底层 `Vec<KeyRange>`。后者是按 Go API 语义保留的请求转换入口，但当前 Rust `CopWireRequest.ranges` 仍是 `Vec<KeyRange>`。
- `KeyRanges::reset(ranges)`：需要 `&mut self`，一次替换底层向量；`coprocessor.rs::ensure_monotonic_key_ranges` 排序后用它写回。
- `Display for KeyRanges`、`write_go_quoted_bytes` 和 `write_go_quoted_text`：连续输出 `["start", "end"]`，对 Go 定义的控制字符、反斜线、引号、U+2028/U+2029 以及非法 UTF-8 字节做对应转义。
- `NewKeyRanges(Vec<KeyRange>) -> KeyRanges`：命名与 Go 构造函数对齐，内部调用 `KeyRanges::new`。

## 执行流程

1. 上游将逻辑键范围构造为 `KeyRanges`。`region_cache.rs::split_key_ranges_by_locations` 先用 `to_ranges` 交给后端批量定位，再遍历 `ranges.iter()`，把每个逻辑区间切成落在具体 Region 内的片段。
2. `coprocessor.rs::build_cop_tasks` 先经 `ensure_monotonic_key_ranges` 检查并必要时用 `to_ranges` + 排序 + `reset` 恢复单调顺序，然后请求后端按 Region 定位。
3. 每个 Region 内的区间集按 `RANGES_PER_TASK` 分批，用 `slice(from, to)` 构造每个 `CopTask.ranges`。`ref_at` 被用来读取定位段的首尾键并估算 row hint。
4. 执行期发生部分成功或需重试时，`calculate_retry` 和 `calculate_remain` 依据扫描方向选取 `split` 的左或右半部，避免重复处理已完成的键空间。
5. 组装请求时，`build_wire_request` 调用 `to_pb_ranges` 生成 `CopWireRequest.ranges`。目前该步骤会克隆每个 `KeyRange` 及其键字节。
6. 诊断或日志需要文本时，`Display::fmt` 顺序处理每个范围，使用 Go 风格引号和转义写入 formatter。

`split` 的具体分支是：先跳过所有有界且 `end <= key` 的范围；若候选范围存在且 `key > start`，则克隆前缀并追加左截断段，右侧以右截断段开头并追加余下范围；否则用两次 `slice` 在找到的下标处直接分割。键等于某范围的 `start` 时不会生成空左片段；键等于有界 `end` 时该范围完整归左。

## 数据与状态

`KeyRanges` 的唯一可变状态是其 `Vec<KeyRange>`。每个 `KeyRange` 拥有 `start` 和 `end` 字节向量，因此 `clone`、`slice`、`to_ranges`、`to_pb_ranges` 和 `split` 的截断分支都可能复制键数据。`ref_at`、`iter` 和 `for_each` 只借用原数据。

核心语义依赖两个不在类型系统中强制的不变量：区间按键空间排列，且 `split` 所依赖的 `end` 顺序可用二分分区查找。`coprocessor.rs::ensure_monotonic_key_ranges` 会在任务构建入口诊断异常并排序，但 `KeyRanges::split` 本身不验证该前置条件。空 `end` 被视为正无穷：`partition_point` 不会跳过它，从而可把 `[start, +inf)` 拆为有界左段和仍无界的右段。

`reset` 是唯一直接修改现有对象的本文件 API；其余转换和拆分都返回新集合。本文件没有全局状态、缓存或隐式共享所有权。

## 依赖与调用关系

下游依赖：

- `std::fmt::{Display, Formatter}` 和 `write!` 完成无中间大字符串的渐进格式化。
- `batch_request_sender.rs::{KeyRange, KeyRanges}` 提供数据类型以及 `new`、`len`、`is_empty`、`iter`、`into_sorted` 等基础方法。
- `Vec` 的 `get`、切片、`partition_point`、`clone`、`extend_from_slice` 和容量预分配实现所有区间操作。

直接上游证据：

- `coprocessor.rs::build_cop_tasks` 用 `slice` 将已按 Region 定位的区间分配给多个 `CopTask`；`row_hint_for_location` 用 `ref_at` 取首尾范围。
- `coprocessor.rs::calculate_retry` 和 `calculate_remain` 直接调用 `split`；`build_wire_request` 调用 `to_pb_ranges`。
- `coprocessor.rs::ensure_monotonic_key_ranges` 用 `to_ranges` 取出副本，按 `(start, end)` 排序后用 `reset` 写回。
- `region_cache.rs::split_key_ranges_by_locations` 用 `to_ranges` 调用后端定位，然后用 `iter` 遍历并按 Region 边界构造 `LocationKeyRanges`。
- `range_diagnostics.rs::range_issues_for_key_ranges` 和 `min_start_and_max_end_key_of_key_ranges` 组合 `ref_at` 与 `iter`诊断非法边界、重叠、包含、乱序和无限尾。
- `pkg/store/copr/lib.rs` 通过 `pub use key_ranges::*` 对 crate 外暴露本文件的构造函数与 impl 能力。

RustCodeGraph 的文件关系显示目标文件被 `coprocessor.rs`、`region_cache.rs`、相应独立测试等多个文件使用；由于索引未为该 Rust `impl` 的各方法返回精确 callers，上述边进一步由定向源码搜索与调用片段核实。

## 错误处理与边界

- `ref_at` 和 `at` 用 `Option` 表示越界，不 panic。这与 Go `RefAt`/`At` 的越界语义不同：Go 版本在无可用 `last` 时可能返回 `nil`，`At` 随后解引用会 panic。
- `slice` 对 `from > to` 或 `to > len` 执行 `assert!` 并 panic，这是编程器不变量失败，不是可恢复错误。空切片（`from == to`）合法。
- `split` 对空集合返回两个空集合；键落在区间间隙时不会构造伪区间；无界末段可被正常截断。
- `split` 不拒绝乱序、重叠或 `start > end` 的输入。对这些输入，`partition_point` 的分区前提不再可保证，调用者应在入口校验/排序，或先使用 `range_diagnostics.rs` 的诊断函数。
- `Display::fmt`、`write_go_quoted_bytes` 和 `write_go_quoted_text` 传播 formatter 的 `fmt::Error`。对非法 UTF-8，它们保留合法前缀并把无效字节写成 `\xNN`；内部 `expect` 依赖标准库 `Utf8Error::valid_up_to` 的契约。
- 本文件不返回 crate 业务错误类型；Region 定位无进展、边界不匹配或重定位超预算等可恢复错误由上层 `region_cache.rs` 产生 `BatchError`。

## 并发与资源生命周期

本文件不启动任务、不持有锁、通道、网络连接或事务。所有资源是常规所有权值：`ref_at`/`iter`/`for_each` 的借用受 `&self` 限制，`reset` 通过 `&mut self` 要求独占访问，其余方法通过拥有的 `Vec` 返回独立结果。

因为 `KeyRange` 的字节存储是 `Vec<u8>`，克隆后的任务范围不会与原集合共享可变缓冲区；这降低了并发任务之间的别名风险，代价是切片、拆分与请求转换时的分配和字节拷贝。是否可在线程间移动/共享由成员类型的自动 trait 推导决定，本文件没有额外 `unsafe` 实现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/key_ranges.go`，对照测试是 `pkg/store/copr/key_ranges_test.go`；Rust 独立测试为 `pkg/store/copr/key_ranges_test.rs`。两版本的主要 API 一一对应：`NewKeyRanges`、`RefAt/ref_at`、`At/at`、`Slice/slice`、`Do/for_each`、`Split/split`、`ToRanges/to_ranges`、`Reset/reset`、`ToPBRanges/to_pb_ranges` 和 `String/Display`。

必须保留的行为对齐包括：区间为半开边界；按首个 `end > key` 或无界 `end` 的位置拆分；键在区间内时左右各保留一个截断片段；键在边界或间隙时不生成空片段；展示格式为连续的 `[%q, %q]`。Rust 测试 `key_ranges_split_matches_all_go_cases` 复现了 Go `TestCopRangeSplit` 的边界、内部键、间隙与尾部案例，并额外覆盖无界 `end`、`reset`、转换和非法 UTF-8 格式化。

当前实现差异：

- Go `KeyRanges` 用 `first + mid + last` 表示，以便在构建 copTask 时通过首尾指针避免大切片分配；Rust 版本是平坦 `Vec<KeyRange>`，`slice` 和 `split` 生成拥有的克隆。因而语义相近，但性能和所有权模型不同。
- Go `RefAt` 返回指针，`At` 直接解引用；Rust 用 `Option` 显式表达越界，且 `at` 返回克隆。
- Go `Slice` 可通过复用 `mid` 切片及首尾指针共享底层数据；Rust `slice` 校验边界后执行 `to_vec`。
- Go `ToPBRanges` 用 `unsafe.Pointer` 将 `kv.KeyRange` 指针视为 protobuf `coprocessor.KeyRange` 以避免额外分配；Rust 版本无 `unsafe`，当前只克隆为 `Vec<KeyRange>`，与当前 `CopWireRequest` 内部模型配套。
- Go 格式化直接依赖 `fmt.Sprintf("%q")`；Rust 用两个私有辅助函数复现 Go 字节切片的引号与转义规则。

## 扩展指南

- 新增区间操作时，先判断它属于数据类型的通用基础能力（更适合 `batch_request_sender.rs` 的 `impl KeyRanges`），还是 Go `key_ranges.go` 对齐的切片/拆分能力（更适合本文件）。避免在两个 impl 中引入语义重复的方法。
- 修改 `split` 前必须同时核对 `coprocessor.rs::calculate_retry`、`calculate_remain` 的升/降序选边逻辑，以及 `region_cache.rs` 对空 `end` 作为无上界的解释。新回归用例应放在独立的 `key_ranges_test.rs`，不要嵌入生产文件。
- 若要改变排序或允许重叠，必须先定义 `partition_point` 需要的全局不变量，并同步 `range_diagnostics.rs`、`ensure_monotonic_key_ranges` 及 Region 拆分测试。不能仅调整单个边界比较。
- 若要减少分配，需要评估 Go `first/mid/last` 表示的所有权 Rust 替代（例如共享不可变存储加索引视图）。这会影响 `KeyRanges` 的公开 tuple 字段、`RegionInfo.Ranges`、所有直接 `.0` 访问及并发任务所有权，应作为单独性能/兼容性任务。
- 若 `to_pb_ranges` 未来真正转换为 protobuf 类型，应同步修改 `CopWireRequest.ranges`、网络后端适配层和序列化测试，并明确是否仍需与 Go 零拷贝策略对齐。
- 扩展 Go 引号规则时，优先在 `key_ranges_test.rs::key_ranges_string_matches_go_quoted_bytes` 补充控制字符、有效 Unicode、U+2028/U+2029、截断 UTF-8 和连续无效字节案例，并与 Go `%q` 实际输出对照。

## 验证依据

本文档基于以下直接证据：

- RustCodeGraph `status`：当前项目索引包含 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/store/copr` 确认目标、Go 对照与独立测试均已索引。
- RustCodeGraph `node --file pkg/store/copr/key_ranges.rs --offset 1 --limit 400`：读取目标文件全部 161 行，确认所有导出、impl、私有格式化辅助函数和构造函数。
- RustCodeGraph `query KeyRanges --kind struct`：确认 Rust 实体定义位于 `pkg/store/copr/batch_request_sender.rs`，并确认同路径 Go 类型；`node --file .../batch_request_sender.rs --offset 80 --limit 65` 核对字段和基础方法。
- RustCodeGraph `node --file pkg/store/copr/key_ranges_test.rs --offset 1 --limit 300`：读取全部 155 行 Rust 独立测试，核对访问、所有合法切片、Go 拆分用例、无界末段、reset/PB 转换和引号字节行为。
- `pkg/store/copr/Cargo.toml`、`pkg/store/copr/lib.rs` 与根 `Cargo.toml`：确认 crate 名、Go 包映射、workspace 成员身份、模块声明和对外再导出。目标包下没有 `doc.go`，最近的模块契约为 `lib.rs` 顶部文档。
- `pkg/store/copr/key_ranges.go` 与 `pkg/store/copr/key_ranges_test.go`：核对 Go 的 `first/mid/last` 表示、切片优化、二分拆分、`unsafe` PB 转换和原始边界用例。
- 定向调用点搜索及源码片段：`coprocessor.rs::{ensure_monotonic_key_ranges, build_cop_tasks, calculate_retry, calculate_remain, build_wire_request}`、`region_cache.rs::{split_region_ranges, split_key_ranges_by_locations, split_key_ranges_by_buckets}` 和 `range_diagnostics.rs::{range_issues_for_key_ranges, min_start_and_max_end_key_of_key_ranges}`。

人工复核结论：该文件存在于把 Go coprocessor 的键区间访问与边界拆分语义提供给 Rust 任务构建链；其运行依赖有序半开区间与空 `end` 无上界约定；安全扩展需同步 Rust 独立测试，并重新检查 coprocessor 重试、Region 切分、PB 请求类型及 Go 对照语义。本任务为纯文档分析，按计划不运行 Cargo。
