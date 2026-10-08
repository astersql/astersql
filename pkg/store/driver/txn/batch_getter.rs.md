# `pkg/store/driver/txn/batch_getter.rs`

## 文件定位

该文件属于 `astersql-store-driver-txn` crate；crate 边界由 `pkg/store/driver/txn/Cargo.toml` 定义，入口 `pkg/store/driver/txn/lib.rs` 通过 `mod batch_getter` 装配并用 `pub use batch_getter::*` 导出本文件的公开项。它位于事务读路径中，负责把事务内存写缓冲、可选的语句级中间缓存和一致性快照组合成一个 `BatchGetter`。生产侧直接入口是 `pkg/store/driver/txn/txn_driver.rs::tikvTxn::BatchGet`：该方法把事务的 `mem_buffer` 和 `snapshot` 包装成 trait object，调用 `NewBufferBatchGetter(...).BatchGet(...)`，随后按批量选项统一处理 `commit_ts`。

本文件不是底层 TiKV RPC 实现，也不决定 MVCC 可见版本；实际读取由传入的 `BatchBufferGetter`、`Getter` 和 `BatchGetter` 实现完成。本文件只定义读取优先级、缺失键回落、墓碑遮蔽和错误传播规则。

## 核心职责

- `tikvBatchGetter` 把 `Arc<dyn BatchGetter>` 包成字节键批量读取适配器，其 `BatchGet` 原样委派给底层 `batch_get`。
- `tikvBatchBufferGetter` 合并事务内存缓冲与可选中间缓存：缓冲优先，中间缓存只补缓冲未返回的键。
- `BufferBatchGetter` 再把上述组合层与快照层叠加：只有前两层均未解析的键才进入快照批量读取。
- 空 `ValueEntry.value` 被视为删除墓碑。墓碑作为“已解析”条目阻止同键访问下层快照，但在最终公开结果中被删除。
- `BatchGetOption` 会原样传给批量读取层；中间缓存只能点查，因此通过 `BatchGetToGetOptions` 转成 `GetOption`，保留选项顺序和标签。

这些职责由 `pkg/store/driver/txn/batch_getter_test.rs::TestBufferBatchGetter` 直接验证：`a` 命中缓冲、`b` 被缓冲墓碑遮蔽、`c` 命中中间缓存、`d` 回落快照，完全缺失的 `xx` 不进入结果。

## 主要符号

- `tikvBatchGetter { tidb_batch_getter: Arc<dyn BatchGetter> }`：快照等底层批量读取器的内部适配器。`new` 保存共享引用；`BatchGet(&[Vec<u8>], &[BatchGetOption])` 返回 `HashMap<Key, ValueEntry>`。
- `tikvBatchBufferGetter { tidb_middle_cache, tidb_buffer }`：缓冲与可选中间缓存的内部组合器。`tidb_middle_cache` 是 `Option<Arc<dyn Getter>>`，`tidb_buffer` 是 `Arc<dyn BatchBufferGetter>`。
- `tikvBatchBufferGetter::Get`：单键适配入口。缓冲成功则直接返回；缓冲非“未找到”错误直接传播；缓冲缺失且没有中间缓存时返回 `DriverError::ClientNotExist`；存在中间缓存时查询它，并把其中任何错误规范化为 `ClientNotExist`。该行为用于兼容 Go/client-go 的不存在哨兵。
- `tikvBatchBufferGetter::BatchGet`：先调用 `BatchBufferGetter::batch_get_bytes`，再逐键用中间缓存补齐缺失项。中间缓存的 not-found 被忽略，其他错误终止并返回。
- `tikvBatchBufferGetter::Len`：仅返回底层缓冲的条目数，不包含中间缓存和快照。
- `BufferBatchGetter { buffer, snapshot }`：公开的三层读取器；字段保持私有以固定组合顺序。
- `NewBufferBatchGetter`：公开构造函数，接收缓冲、可选中间缓存和快照，分别构造成两个内部适配器。
- `BufferBatchGetter::BatchGet`：三层批量读取的主流程。
- `impl BatchGetter for BufferBatchGetter`：让组合读取器可继续作为统一 `BatchGetter` 使用；trait 方法只委派给固有方法 `BatchGet`。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

以 `BufferBatchGetter::BatchGet(keys, options)` 为主线：

1. 调用 `self.buffer.BatchGet`。该方法先执行事务缓冲的 `batch_get_bytes`；底层批量错误立即通过 `?` 返回。
2. 若没有中间缓存，缓冲结果直接进入下一层组合逻辑。若有中间缓存，则先用 `BatchGetToGetOptions` 转换选项，再按输入键顺序检查结果表。
3. 已在缓冲结果中的键不再查询中间缓存，包括值为空的墓碑键。未命中的键逐个调用 `middle_cache.get`：成功值写入结果；not-found 被视为正常缺失；其他错误立即返回。
4. 外层从原始输入键中过滤出结果表仍不包含的键，形成 `unresolved`。这一步同样把墓碑视为已解析，从而避免被旧快照值“复活”。
5. `unresolved` 非空时一次性调用 `self.snapshot.BatchGet`。快照返回的每个键值用 `entry(...).or_insert(...)` 合并，保证即使异常实现返回重复层级数据，也不会覆盖已存在的上层值。
6. 对合并结果执行 `retain`，删除所有 `ValueEntry::is_value_empty()` 的条目。最终映射只包含可见的非空值。
7. 生产入口 `tikvTxn::BatchGet` 收到结果后，再通过 `apply_commit_ts_option_batch` 确保未请求 commit timestamp 时将其清零。

结果类型是 `HashMap`，因此本文件不承诺输出顺序；重复输入键也只会在映射中保留一个条目。空输入会完成一次缓冲批量调用，但不会访问中间缓存或快照。

## 数据与状态

输入键的公共类型 `Key` 是 `Vec<u8>`，值由 `ValueEntry { value: Vec<u8>, commit_ts: u64 }` 表示，定义均在 `pkg/store/driver/txn/lib.rs`。本文件在调用期间维护两个局部映射/集合形态的数据：逐层累积的 `HashMap<Key, ValueEntry>`，以及需要访问快照的 `Vec<Key>`。键在生成 `unresolved` 和插入中间缓存结果时会被克隆，空间复杂度与输入键及返回项总字节数相关。

三个读取层通过 `Arc<dyn ...>` 持有；构造器不复制底层存储状态。`BufferBatchGetter` 自身没有可变字段、锁、缓存失效策略或事务时间戳状态。层级优先级是核心不变量：缓冲 > 中间缓存 > 快照；“结果映射中是否已有该键”同时表示该键已经由更高优先级层裁决，无论其值是否为空。

`commit_ts` 不在本文件内计算或改写。本文件只透传选项和命中层返回的 `ValueEntry`；独立测试用不同的 `commitTSBase` 证明请求 `WithReturnCommitTSBatch` 时，时间戳来自实际命中层。

## 依赖与调用关系

上游关系：

- `pkg/store/driver/txn/txn_driver.rs::tikvTxn::BatchGet` 是 Rust 生产代码中的直接构造调用者；它当前传入 `None` 作为中间缓存，因此生产事务路径实际使用“缓冲 → 快照”两层，中间缓存能力由通用适配器保留并由单元测试覆盖。
- `pkg/store/driver/txn/lib.rs` 导出 `NewBufferBatchGetter`、`BufferBatchGetter` 及其 trait 实现，并在独立文件 `batch_getter_test.rs` 中装配测试模块。

下游关系：

- `BatchBufferGetter::batch_get_bytes` 提供事务缓冲批量数据，`BatchBufferGetter::len` 支撑 `Len`。
- `Getter::get` 提供中间缓存点查；`BatchGetToGetOptions` 负责批量选项到点查选项的转换。
- `BatchGetter::batch_get` 提供快照批量数据，也构成本文件对外实现的统一接口。
- `DriverError::is_not_found`（`pkg/store/driver/txn/error.rs`）把 `NotFound` 和 `ClientNotExist` 都识别为缺失类错误；`ClientNotExist` 是单键适配时使用的 client-go 兼容哨兵。
- `ValueEntry::is_value_empty`（`pkg/store/driver/txn/lib.rs`）定义墓碑判断。

`pkg/store/driver/txn/Cargo.toml` 将该文件放在 `astersql-store-driver-txn` crate 内，`[lib] path = "lib.rs"`。本文件直接使用的核心类型均来自本 crate；manifest 中声明的 `astersql-kv`、`astersql-tablecodec`、`errors` 和带 tag 的 `tikv-client` 是整个事务 crate 的依赖，而不能仅凭本文件断言它逐一直接调用这些外部 crate。

## 错误处理与边界

- 缓冲批量读取错误、快照批量读取错误均立即传播，调用者得不到部分成功结果。
- 中间缓存批量补齐时，缺失类错误被忽略，其他错误原样传播；此前已累积的映射不会随错误返回给调用者，因为返回类型为单一 `Result<HashMap<...>, DriverError>`。
- `tikvBatchBufferGetter::Get` 的兼容规则更强：中间缓存返回的任何错误都被转换为 `ClientNotExist`，而不是只转换 not-found。此处与 `BatchGet` 的错误策略不同，扩展时不能误认为两者等价。
- 缓冲返回空值不是缺失，而是明确删除。它必须留在内部映射中直至快照回落结束，否则会错误读取旧快照值；最后才从公开结果中剔除。
- 快照合并使用 `or_insert`，不会覆盖上层已有值。正常流程中快照只收到 `unresolved`，该防御仍维持了层级优先不变量。
- 对中间缓存的逐键读取没有去重：若输入含重复且该键持续缺失，会重复点查；快照 `unresolved` 也可能包含重复键。当前代码保证结果正确，但调用放大是潜在性能边界。
- 本文件不做键格式验证、超时控制、重试或取消；这些能力取决于注入的读取实现和选项。

## 并发与资源生命周期

三个底层 trait 都要求 `Send + Sync`（`BatchBufferGetter` 继承 `Getter`），并通过 `Arc` 共享所有权，因此 `BufferBatchGetter` 可以安全持有可能被其他组件共享的读取器。方法只借用 `&self`，本文件没有显式可变共享状态、互斥锁、任务、线程、future 或 channel；真实并发控制由底层实现负责。

构造时增加相应 `Arc` 的所有权计数，`BufferBatchGetter` 被释放时其字段正常 drop 并减少计数。本文件没有显式 `close`/取消协议，也不启动后台资源。一次调用内的 `values`、`get_options` 和 `unresolved` 都是栈上所有权变量，错误提前返回时自动释放。由于快照引用只在构造器中固定，本对象不会在调用间更换读取视图；快照一致性本身由传入对象保证。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/store/driver/txn/batch_getter.go`，测试对照为 `pkg/store/driver/txn/batch_getter_test.go`。

- Rust 的 `tikvBatchGetter`、`tikvBatchBufferGetter`、`BufferBatchGetter`、`NewBufferBatchGetter` 和三个 `BatchGet`/`Get`/`Len` 方法逐项镜像 Go 同名结构与方法。
- Go 版用 `unsafe` 在 `[][]byte` 与 `[]kv.Key` 间转换，并把字符串作为 map key；Rust 版统一使用拥有所有权的 `Vec<u8>`/`Key` 和 `HashMap<Key, ...>`，通过必要克隆完成边界转换，没有 `unsafe`。
- Go 的公开 `BufferBatchGetter` 包装 client-go 的 `transaction.BufferBatchGetter`；Rust 将对应三层算法直接实现于本 crate，因此不依赖外部对象完成合并，但保持相同的优先级和墓碑语义。
- Go 的 `context.Context` 与可变参数 options 在 Rust 中分别没有直接参数和改为切片；取消/截止时间若需要支持，必须经 trait/option 或底层实现传递，当前文件没有等价 context 通道。
- Go 测试与 Rust `TestBufferBatchGetter` 使用相同的 a/b/c/d 数据、1000/2000/3000 三层时间戳基数、墓碑和缺失键断言，说明移植保留了原测试意图而非缩减场景。
- Rust 生产入口当前传入 `middle_cache = None`；不能据此声称实际事务主链已经使用语句级中间缓存。

## 扩展指南

- 新增读取层或调整优先级时，主要修改点是 `NewBufferBatchGetter` 的装配字段和 `BufferBatchGetter::BatchGet` 的 unresolved/合并流程。必须保持“上层存在即阻止下探”以及墓碑最后过滤两个不变量。
- 扩展批量选项时，应同步检查 `pkg/store/driver/txn/lib.rs::BatchGetToGetOptions`、各底层 trait 实现和 `txn_driver.rs::BatchGet` 的最终规范化；不能只在本文件增加标签透传。
- 改变错误分类时，应同时审查 `tikvBatchBufferGetter::Get` 与 `BatchGet`。前者为 client-go 哨兵兼容会吞并中间缓存错误，后者会传播非 not-found，任何统一行为的改动都可能产生兼容性变化。
- 优化重复键或逐键中间缓存访问时，可在 `tikvBatchBufferGetter::BatchGet` 增加去重或批量接口，但应明确输入重复时的调用次数、错误出现顺序和结果优先级；`HashMap` 结果本身不提供顺序保证。
- 所有行为变更都应更新独立测试 `pkg/store/driver/txn/batch_getter_test.rs`，不要把测试嵌入生产源文件。至少保留缓冲覆盖、墓碑遮蔽、中间缓存命中、快照回落、全层缺失和 commit_ts 来源断言；错误路径或重复键优化应增加对应回归用例，并与 Go 文件/测试语义核对。
- 若让生产事务启用中间缓存，还需修改直接接线点 `pkg/store/driver/txn/txn_driver.rs::BatchGet`，并评估缓存生命周期、快照一致性和陈旧值风险。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust 文件可按文件节点读取。
- RustCodeGraph `node --file pkg/store/driver/txn/batch_getter.rs`：核对了 187 行完整实现，以及 `tikvBatchGetter`、`tikvBatchBufferGetter`、`BufferBatchGetter`、`NewBufferBatchGetter` 和 trait impl。
- RustCodeGraph `explore`/调用边：确认 `NewBufferBatchGetter` 的 Rust 调用者为 `pkg/store/driver/txn/txn_driver.rs::BatchGet`；文件节点进一步核对其构造参数和 commit_ts 后处理。
- `pkg/store/driver/txn/lib.rs`：核对模块导出、`Key`/`ValueEntry`、选项转换、三个读取 trait 及独立测试模块声明。
- `pkg/store/driver/txn/error.rs::DriverError::is_not_found`：核对两种缺失错误的分类。
- `pkg/store/driver/txn/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go 包映射和依赖边界。
- `pkg/store/driver/txn/batch_getter.go` 与 `batch_getter_test.go`：核对 Go 原实现、client-go 包装方式和原测试意图。
- `pkg/store/driver/txn/batch_getter_test.rs::TestBufferBatchGetter`：核对 Rust 中的层级优先、墓碑、完全缺失和 commit_ts 行为；测试位于独立文件，没有内嵌到生产源文件。
- 本任务是只读行为分析与文档新增；按计划不运行 Cargo。交付前以任务指定命令验证文档恰含十一个固定二级章节，并人工复核唯一产物和引用路径。
