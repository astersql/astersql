# `br/pkg/streamhelper/spans/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-streamhelper-spans` 的 crate 根。`br/pkg/streamhelper/spans/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确指定它为库入口，并以 `package.metadata.porting.go-package = "br/pkg/streamhelper/spans"` 标记其 Go 对照包。这个文件本身不实现区间算法，而是把同目录的三个实现文件组织成一个公开 API 门面：`sorted.rs` 负责按起始键排序的区间树，`utils.rs` 负责区间几何和比较工具，`value_sorted.rs` 负责按检查点值建立二级索引。

该 crate 位于 BR 流备份 `streamhelper` 子系统。直接生产侧证据见 `br/pkg/streamhelper/advancer.rs`：推进器从本 crate 导入 `Full`、`NewFullWith`、`Sorted`、`Span`、`ValueSortedFull`、`Valued`，用它们维护任务键空间内各区间的 flush checkpoint，并以最小值作为全局安全推进水位。

## 核心职责

本文件承担四项边界职责：

1. 以 `#[path = "..."] pub mod ...` 声明 `sorted`、`utils`、`value_sorted` 三个生产模块，使非标准的“crate 根与模块文件同目录”布局可被 Rust 模块系统加载。
2. 以 `pub use sorted::*`、`pub use utils::*`、`pub use value_sorted::*` 扁平再导出模块公开项，使调用方可直接写 `astersql_br_pkg_streamhelper_spans::Span`，而不必经过 `::sorted::Span`。
3. 在 `cfg(test)` 下挂载 `parity_test.rs`、`sorted_test.rs`、`utils_test.rs`、`value_sorted_test.rs`。测试逻辑保持在独立文件，没有内嵌到生产源文件。
4. 在 crate 级暂时允许 `dead_code`、Go 风格命名和未使用项。这使迁移代码可以保留 Go 的公开符号名与尚未由 Rust 主链调用的接口，但也会降低编译器对无效 API 的告警力度。

因此，`lib.rs` 的价值是稳定 crate 的编译边界和公开调用面；区间切分、合并、排序或检查点计算均不在这里执行。

## 主要符号

- `pub mod sorted`：加载 `sorted.rs`。其关键公开符号包括 `Value = u64`、半开区间 `Span`、带值区间 `Valued`、键序主树 `ValuedFull`、构造函数 `NewFullWith`，以及 `ValuedFull::{Merge, Traverse}`。
- `pub mod utils`：加载 `utils.rs`。其公开函数包括 `Overlaps`、`CompareBytesExt`、`Collapse`、`Full`、`ValuedSetEquals` 和调试辅助 `Debug`；该模块还为 `Valued` 实现 `Equals`。
- `pub mod value_sorted`：加载 `value_sorted.rs`。其关键公开符号是 `ValueSortedFull`、`Sorted`、`NewSortedFull`，以及 `Merge`、`MergeAll`、`TraverseValuesLessThan`、`Min`、`MinValue` 等方法。
- `mod parity_test`、`mod sorted_test`、`mod utils_test`、`mod value_sorted_test`：只在测试构建中存在的私有模块。它们可通过 crate 根的再导出直接使用生产 API；`value_sorted_test` 还复用 `sorted_test::{s, kv}` 测试辅助函数。
- 三条 glob re-export：形成当前公开兼容面。新增的模块公开项会自动成为 crate 根公开项，因此它们既方便 Go 包级 API 的机械对齐，也可能无意扩大稳定 API。

`lib.rs` 不定义常量、结构体、trait、函数或 `impl`，也没有 feature 条件；唯一条件编译项是四个 `cfg(test)` 测试模块。

## 执行流程

编译期流程如下：

1. Cargo 根据 `Cargo.toml` 选择 `lib.rs` 作为 crate 根。
2. 编译器先应用 crate 级 lint allow 列表，然后按显式 `path` 解析三个生产模块。
3. 模块之间通过 crate 根互相引用：例如 `sorted.rs` 使用 `crate::utils::{Collapse, CompareBytesExt, Overlaps}`，`utils.rs` 使用 `crate::sorted::{Span, Valued}` 与 `crate::value_sorted::ValueSortedFull`，`value_sorted.rs` 使用前两者提供的类型和构造函数。
4. 三个 `pub use ...::*` 将公开项提升到 crate 根，外部 crate 随后从统一门面导入所需符号。
5. 仅在 `cfg(test)` 为真时，编译器再加载四个独立测试文件；普通库构建不会包含这些测试模块。

运行期的代表性业务链由消费者触发：`CheckpointAdvancer::SetTask` 在 `advancer.rs` 中执行 `Sorted(NewFullWith(&init, 0))` 初始化覆盖任务范围的树；采集到 region flush TS 后调用 `ValueSortedFull::Merge`，重叠区间以较大值合并；`importantTick` 调用 `MinValue`/`Min` 取得最落后的区间，并据此上传全局 checkpoint、推进 GC safe point。`lib.rs` 只让这些符号可达，不参与任何一次运行期分派。

## 数据与状态

本文件自身没有全局变量、缓存或可变状态。它暴露的数据模型来自子模块：

- `Span { StartKey, EndKey }` 表示半开区间 `[StartKey, EndKey)`；空 `EndKey` 在比较路径中表示正无穷，`Full()` 返回默认空起止键以表达全键空间。
- `Valued { Key, Value }` 将区间绑定到 `u64` 水位；流备份调用方把 `Value` 解释为 checkpoint TS。
- `ValuedFull` 的私有 `BTreeMap<Vec<u8>, Valued>` 以起始键排序，目标不变量是内部区间互不重叠；`NewFullWith` 先 `Collapse` 初始范围来建立这一不变量。
- `ValueSortedFull` 同时保存键序主树和以 `(Value, StartKey)` 为键的 `BTreeMap` 二级索引。`MergeAll` 删除重叠旧索引、更新主树并插入新索引，`MinValue` 因而能读取全局最小水位。

公开 `Span` 使用拥有所有权的 `Vec<u8>`，遍历和合并路径会克隆 `Valued`。这避免借用生命周期泄露到 API 外，但大范围、高碎片场景中的复制和树节点数量是扩展时需要关注的性能成本。

## 依赖与调用关系

crate 边界以 `br/pkg/streamhelper/spans/Cargo.toml` 为准：该清单没有声明外部 Rust 依赖，核心实现只使用标准库 `BTreeMap`。`BUILD.bazel` 描述的是并存的 Go `spans` 库及其 Go 依赖，不是 Rust crate 的依赖清单。

内部依赖形成一个有意的三模块闭环：`sorted` 调用 `utils` 的折叠、比较和重叠判定；`utils` 使用 `sorted` 的数据类型并为 `Valued` 增补方法，同时让 `Debug` 接受 `value_sorted::ValueSortedFull`；`value_sorted` 组合 `sorted::ValuedFull` 并调用 `utils::Full`。crate 根统一声明这些模块后，Rust 能解析这种交叉引用。

主要上游如下：

- `br/pkg/streamhelper/advancer.rs` 是生产主链消费者：初始化 `ValueSortedFull`，合并 collector 返回的区间 TS，并查询最小水位。
- `br/pkg/streamhelper/subscription_test.rs` 通过真实 fake flush stream 收集 `Valued` 事件，用 `Sorted(NewFullWith(...))` 和 `MinValue` 验证多 store 检查点收敛。
- 同目录四个 Rust 测试文件通过 crate 根公开面验证区间算法和 Go/Rust 契约。

RustCodeGraph 对 `lib.rs` 本身只识别到模块入口而非运行期函数调用；真实调用边属于再导出符号对应的子模块实现。分析本文件时不能把“门面被某文件引用”误当成 `lib.rs` 中存在业务函数。

## 错误处理与边界

`lib.rs` 不返回 `Result`、不产生业务错误，也没有 `unsafe`。边界行为由被再导出的实现定义：

- `Merge` 在输入与现有覆盖没有重叠时直接不修改树，这也覆盖落在初始化子范围之外或空输入区间的情形。
- `Overlaps` 使用半开区间语义，有限区间端点相接不算重叠；空 `EndKey` 作为正无穷处理。
- `Collapse` 接受空集合，并合并重叠或相邻区间。
- Rust 的 `Min` 和 `MinValue` 返回 `Option`；这比 Go 版本在空 B-tree 上直接类型断言更明确地表达空树边界。调用方通常用 `unwrap_or(0)` 或传播 `Option` 处理无值状态。
- `ValuedSetEquals` 比较覆盖与取值语义，允许同值连续范围采用不同分段，但拒绝起点不齐、空洞或值不一致。

crate 级 lint allow 不是错误恢复机制。新增符号时仍应避免依赖这些豁免掩盖拼写、未接线逻辑或不必要的公开项。

## 并发与资源生命周期

本文件和三个生产子模块都不创建线程、异步任务、通道、锁、文件句柄或网络资源；所有树操作均要求调用方持有可变引用，因此 crate 内部没有共享可变状态或内部同步。

并发所有权位于上游。`CheckpointAdvancer` 将 `Option<ValueSortedFull>` 放在 `Mutex` 中，`WithCheckpoints` 在持锁期间把 `&mut ValueSortedFull` 交给闭包；collector 的成功钩子通过 `Arc` 捕获同一检查点锁并执行 `Merge`。因此扩展本 crate 时不得在回调内引入阻塞 I/O 或反向获取推进器其他锁，否则会延长临界区或制造锁顺序风险。

树和二级索引随 `ValueSortedFull` 一起按 Rust 所有权自动释放。`Traverse` 与 `TraverseValuesLessThan` 的回调返回 `false` 可提前结束扫描；这既是控制流契约，也是调用方限制持锁时间和遍历成本的手段。

## 与 Go 版本的对应关系

Rust crate 对齐同目录 Go 包 `br/pkg/streamhelper/spans`：

- `sorted.rs` 对应 `sorted.go`，保留 `Value`、`Span`、`Valued`、`ValuedFull`、`NewFullWith`、`Merge` 和 `Traverse` 等名称；重叠区的 `join` 都取两值最大值，保证 checkpoint 单调不降。
- `utils.rs` 对应 `utils.go`，保留 `Overlaps`、`Collapse`、`Full`、`ValuedSetEquals` 等区间语义。
- `value_sorted.rs` 对应 `value_sorted.go`，都维护键序主树和按 `(Value, StartKey)` 排序的二级索引。
- `lib.rs` 的扁平再导出模拟 Go 包级命名空间；Go 无需等价入口文件，因为同目录 `.go` 文件天然组成一个 package。

需要明确的实现差异有三点。第一，Go 的 `Span` 是 `kv.KeyRange` 类型别名，Rust 当前定义自有结构并在 `advancer.rs` 中与 `KeyRange` 显式转换。第二，Go 使用 `github.com/google/btree`，Rust 使用标准库 `BTreeMap`。第三，Rust 的 `Min`/`MinValue` 返回 `Option` 以安全表达空集合，而 Go API 假定树非空并直接返回值。这些差异不改变非空正常路径的排序、合并和最小值语义。

对应测试是 `sorted_test.rs`、`utils_test.rs`、`value_sorted_test.rs` 及补充公开契约的 `parity_test.rs`；Go 基线分别位于 `sorted_test.go`、`utils_test.go`、`value_sorted_test.go`。Rust 测试覆盖全键空间/子范围合并、区间空洞、无穷端点、等价分段、严格小于阈值的值序遍历、提前停止和空树 `Option` 行为。

## 扩展指南

新增区间能力时，应先判断其归属：键序覆盖与合并算法放入 `sorted.rs`，通用区间几何放入 `utils.rs`，依赖值序查询的能力放入 `value_sorted.rs`；`lib.rs` 只在确实需要新增模块或改变公开面时修改。不要把实现直接堆入 crate 根。

安全扩展应保持以下不变量：主树按 `StartKey` 有序且节点不重叠；空 `EndKey` 的正无穷语义一致；合并值只通过 `max` 单调推进；更新主树时同步删除和重建 `valueIdx`；相邻同值范围可合并但不能跨越初始化范围中的空洞。任何修改都应同时更新同目录独立 Rust 测试，并对照对应 Go 测试；不要把单元测试写回 `lib.rs`。

新增公开符号时需特别审查三条 glob re-export：子模块中的任意 `pub` 项都会自动暴露到 crate 根，可能产生重名或扩大兼容承诺。若加入新模块，应明确它是否需要 `pub mod`、是否需要根级再导出，以及 `Cargo.toml` 的 Go 包映射是否仍准确。若改变 `Span` 表示或端点语义，还必须同步检查 `br/pkg/streamhelper/advancer.rs` 的 `KeyRange` 转换、collector 成功钩子以及 subscription 测试。

性能风险主要来自区间碎片导致的 `BTreeMap` 节点增长、`Vec<u8>`/`Valued` 克隆和持锁遍历。新增批量 API 时优先复用 `MergeAll` 的索引同步路径，并用提前停止回调控制扫描范围；兼容风险则集中在 Go 风格公开名称、根级再导出路径和 Rust `Option` 空树契约。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph `status`：索引可用，包含 7032 个 Rust 文件；`files --filter br/pkg/streamhelper/spans` 确认 14 个 Rust/Go 源与测试文件。
- RustCodeGraph `node --file br/pkg/streamhelper/spans/lib.rs`：确认 3 个公开生产模块、4 个 `cfg(test)` 私有测试模块、3 条 glob re-export 以及 crate 级 lint 配置。
- RustCodeGraph 对 `sorted.rs`、`utils.rs`、`value_sorted.rs` 的节点读取，以及对 `ValuedFull`、`ValueSortedFull`、`Span`、`NewFullWith`、`WithCheckpoints` 等符号的 `explore/query`：确认数据结构、合并不变量、二级索引及上游用途。
- RustCodeGraph 对 `br/pkg/streamhelper/advancer.rs` 第 20—22、430—431、471—535 行的节点读取：确认生产主链的导入、初始化、锁内合并和最小 checkpoint 查询。
- `br/pkg/streamhelper/spans/Cargo.toml` 与 `BUILD.bazel`：确认 Rust crate 根、Go 包映射、库性质，以及 Rust/Go 构建边界的区别。
- Go 对照 `sorted.go`、`utils.go`、`value_sorted.go`：确认公开命名、最大值合并、半开区间/无穷端点、折叠与值序索引语义。
- Rust 独立测试 `parity_test.rs`、`sorted_test.rs`、`utils_test.rs`、`value_sorted_test.rs`，以及集成使用测试 `br/pkg/streamhelper/subscription_test.rs`：确认主要边界条件、提前停止、空树差异和真实 flush 事件收敛用途。

本任务为纯文档分析，按计划未运行 Cargo 或代码测试。结构校验要求本文恰好包含上述 11 个固定二级标题；事实复核范围限于该 crate 门面、其直接实现/调用边、Cargo/Go 对照和相关独立测试。
