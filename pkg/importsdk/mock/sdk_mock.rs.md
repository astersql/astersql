# `pkg/importsdk/mock/sdk_mock.rs`

## 文件定位

本文件是独立 crate `astersql-importsdk-mock` 的核心实现。crate 入口 [`lib.rs`](lib.rs) 声明 `sdk_mock` 模块并重导出其公开项；[`Cargo.toml`](Cargo.toml) 表明它只直接依赖 `astersql-errors` 与 `astersql-importsdk`，后者提供被替代的 `FileScanner`、`JobManager`、`SQLGenerator`、`SDK` trait 及其数据类型。工作区根 `Cargo.toml` 将该 crate 注册为成员和 `facade_importsdk_mock` 依赖，`pkg/lib.rs` 再从 facade 模块重导出它。

它是测试替身而非生产导入实现：通过内存中的期望队列模拟导入 SDK 的扫描、作业管理、SQL 生成与关闭能力，使调用方无需对象存储或数据库即可验证编排。当前仓库中可确认的直接 Rust 行为验证位于 [`sdk_mock_test.rs`](sdk_mock_test.rs)；名称相同的 `lightning/pkg/importinto/stubs.rs::MockSDK` 是另一套局部 stub，不能视为本文件类型的调用者。文件没有条件编译项；仅测试模块由 `lib.rs` 的 `#[cfg(test)]` 接入。

## 核心职责

1. 用公开 `MockCall` 把 13 类可观察调用及其业务参数编码为可比较值，例如 `SubmitJob { query }`、`GetJobStatus { job_id }` 和 `GenerateImportSQL { table_meta, options }`。
2. 用私有 `MockQueue`/`QueueState` 保存已登记期望与实际调用，以 `Expectation` 表示匹配器、上下文匹配器、调用次数上下限和 `After` 依赖。
3. 提供四组 GoMock 风格替身：单职责的 `MockFileScanner`、`MockJobManager`、`MockSQLGenerator`，以及同时实现四个 trait 的聚合 `MockSDK`；每个主体和 recorder 共享同一队列。
4. 将 trait 方法调用派发到期望队列，校验响应变体，再返回预设成功值或 `SharedError`；同时提供 `*Parts` API，保留 Go 中“值与错误可同时存在”的双返回位语义。
5. 暴露 `Times`、`AnyTimes`、`MinTimes`、`MaxTimes`、`After`、`Matching`、`ContextMatching`、`ReturnParts`、`DoAndReturn` 等配置能力，并允许通过 `calls`、`pending_expectations`、`verify` 检查结果。

## 主要符号

- `MockCall`：公开的调用快照枚举；`Display` 只输出方法名，`Debug`/`Eq`/`PartialEq` 用于错误信息和测试断言。上下文本身不写入快照，只有业务参数会被拥有化保存。
- `MockResponse`：私有响应联合。除各接口的强类型 `Result` 变体外，`Dual` 保存类型擦除的值与独立错误位，`Dynamic` 保存接收调用及上下文的回调，`SQLCallback` 保存接收真实 `TableMeta`/`ImportOptions` 的回调。
- `Expectation`：单条期望的完整状态；`id` 用于依赖引用，`matcher`/`context_matcher` 决定是否命中，`min_calls`、`max_calls`、`called` 控制次数，`after` 保存前置期望 ID。
- `QueueState` 与 `MockQueue`：分别是队列数据和 `Arc<Mutex<_>>` 共享外壳。`expect_matching` 登记期望；`dispatch_internal` 记录并匹配调用；`verify` 检查最低调用次数是否满足。
- `MockExpectation`：登记后返回的配置句柄。其公开方法均按值接收并返回 `Self`，因而可链式配置；`configure` 通过 ID 找到尚在队列中的期望。
- `MockFileScanner`、`MockJobManager`、`MockSQLGenerator`：分别实现对应 trait；其 `*MockRecorder` 通过 `EXPECT()`/`expect()` 取得并登记方法期望。
- `MockSDK`：共享一条队列的聚合替身，同时实现 `FileScanner`、`JobManager`、`SQLGenerator`、`SDK`。同名 recorder 覆盖全部接口方法。
- `NewMock*` 与 `new_mock_*`：构造空队列并让主体与 recorder 共享队列；`Default` 也调用大写构造函数。
- `dispatch_*`、`dual_result`、`response_parts`、`dispatch_parts`：集中完成调用构造、队列派发、响应类型校验及 Go 双返回位转换。
- `mock_inspection_api!`、`recorder_inspection_api!`、各类 `*_parts_api!`：只减少四组替身之间的重复代码，不创建独立状态。

## 执行流程

典型路径如下：

1. 测试通过 `NewMockJobManager()` 等构造函数取得 mock；构造函数建立一个 `MockQueue`，并将其克隆给 recorder。克隆只增加 `Arc` 引用计数，状态仍是同一份。
2. `mock.EXPECT().SubmitJob("...", Ok(id))` 等 recorder 方法把预期的 `MockCall` 与 `MockResponse` 交给 `MockQueue::expect`；默认次数为恰好一次，即 `min_calls = 1`、`max_calls = Some(1)`。
3. 可在返回的 `MockExpectation` 上追加次数、前置依赖、业务参数匹配器、上下文匹配器或动态响应。`After` 先要求两个期望属于同一队列，再检查自依赖与可达环。
4. trait 方法将收到的参数转换为拥有的 `MockCall`，然后调用 `dispatch` 或 `dispatch_with_context`。`dispatch_internal` 先把实际调用追加到 `calls`，再从头查找第一个同时满足业务参数、上下文、次数上限与前置依赖的期望；因此未设置 `After` 时不同方法/参数可以乱序调用，而多个同样的期望按登记顺序消费。
5. 命中后增加 `called`。达到有限 `max_calls` 时从队列移除该期望；不限次期望保留。动态回调在释放互斥锁后执行，避免用户回调占用队列锁。
6. 外层方法检查 `MockResponse` 变体并返回对应值。`GenerateImportSQL` 既能返回固定字符串，也能调用接收真实强类型参数的 `SQLCallback`；`*Parts` 方法通过 `response_parts` 保留独立的值和错误。
7. 测试最后调用 `verify()`；它只把 `called < min_calls` 的剩余项视为未满足，已达到最低次数但因无限/较大上限仍留在队列中的期望不会导致失败。

## 数据与状态

核心可变状态只存在于 `QueueState`：`expectations` 是 `VecDeque<Expectation>`，保留登记顺序；`calls` 是完整实际调用时间序列，包括最终未匹配的调用；`next_id` 单调分配期望 ID。`dispatch_internal` 的失败不会回滚已经写入 `calls`，也不会消费不匹配的期望，这一点由 `mismatch_keeps_expectation_and_matching_uses_first_registered_response` 覆盖。

业务参数都复制为拥有值：字符串参数转成 `String`，数值直接保存。`GenerateImportSQL` 的固定匹配不是保存 `TableMeta`/`ImportOptions`，而是经 `table_meta_signature`、`import_options_signature` 转成 `Debug` 字符串；动态 SQL 回调仍收到原始强类型引用。上下文使用 `&(dyn Any + Send + Sync)` 临时传给匹配器或动态回调，不进入调用历史。

`MockResponse::Dual` 与 `Dynamic` 通过 `Arc<dyn Any + Send + Sync>` 类型擦除返回值，读取时按调用所需类型 downcast 并克隆；类型不符会生成 `wrong_response`。因此放入这些响应的值必须满足 `Any + Clone + Send + Sync`。

## 依赖与调用关系

- 上游接口来自 `astersql_importsdk`：[`file_scanner.rs`](../file_scanner.rs) 定义 `FileScanner`，要求创建 schema/table、查询元数据、估算大小和关闭；[`job_manager.rs`](../job_manager.rs) 定义作业提交、查询、取消及分组查询；[`sql_generator.rs`](../sql_generator.rs) 定义 SQL 生成；[`sdk.rs`](../sdk.rs) 将前三者组合为 `SDK`。
- `MockFileScanner` 直接实现 `FileScanner`；`MockJobManager` 直接实现 `JobManager`；`MockSQLGenerator` 直接实现 `SQLGenerator`；`MockSDK` 实现全部四个 trait。trait 对象调用的 `*Parts` 方法由 `*_parts_trait_api!` 显式转发到 mock 的固有方法，而不是使用 trait 默认实现，从而保留双返回位。
- 下游只依赖 `astersql_errors::SharedError`/`New` 来传递预设错误、意外调用和响应类型错误，不访问真实数据库、对象存储、线程运行时或网络。
- RustCodeGraph 将文件索引为 164 个符号，并报告它被 31 个文件“使用”；但精确 `callers`/`callees` 查询在当前索引上超时。结合仓库文本检索，只能确认该 crate 经 `pkg/lib.rs` facade 暴露，且本文件的构造函数在独立测试中直接使用；不能把图的名称级“used by”列表当作已验证的真实调用边。
- Go 对照 [`sdk_mock.go`](sdk_mock.go) 明确由 MockGen 从 `pkg/importsdk` 四个接口生成，其可确认调用者是 `lightning/pkg/importinto/job_orchestrator_test.go`。

## 错误处理与边界

- 无剩余期望或没有任何可匹配期望时，`dispatch_internal` 返回 `SharedError`，消息包含实际调用以及当前期望列表；不匹配项保持未消费。
- 响应变体或类型擦除值与调用返回类型不一致时，`wrong_response` 返回类型不匹配错误。`GetTotalSize` 的 trait 签名不能返回错误，因此意外调用或错误响应会 `panic!`；其他返回 `Result` 的方法传播错误。
- `MockExpectation::configure` 在期望已经达到上限并从队列删除后再次配置会 panic；`last_expectation` 在没有登记期望时也会 panic。这些 API 适合测试配置阶段，不是容错运行时接口。
- `After` 拒绝跨 mock 队列依赖、自依赖和可检测的依赖环。前置条件只有在对应期望已从队列移除（即达到有限最大次数）时才满足；对 `AnyTimes` 或无上限的前置期望设置 `After` 会使后继永远无法满足，这是扩展或使用时必须避免的组合。
- `MinTimes`/`MaxTimes` 模拟 GoMock 的默认值调整：从默认恰好一次切换时会分别解除默认上限或把默认最低次数降为零；调用者应注意配置顺序决定最终区间。
- `dual_result` 遇到独立错误位时优先返回错误，因此普通 `Result` API 无法同时观察值；需要保留二者时必须调用相应 `*Parts` API。
- 锁中毒时 `MockQueue::lock` 取回内部状态继续工作，不让先前 panic 永久废弃 mock；这并不恢复 panic 前可能已发生的部分状态修改。

## 并发与资源生命周期

`MockQueue` 使用 `Arc<Mutex<QueueState>>`，所以 mock、recorder 和 `MockExpectation` 克隆后仍共享同一有序状态，并满足被模拟 trait 的 `Send + Sync` 约束。登记、匹配、调用历史读取与验证都在互斥锁保护下完成；实际调用顺序以获取锁并写入 `calls` 的顺序为准，而不是线程启动顺序。

`dispatch_internal` 会在执行 `Dynamic` 回调前显式释放锁，`SQLCallback` 也在派发返回后执行；用户回调不会持有队列互斥锁，可避免回调再次访问 mock 时的直接死锁。匹配器和上下文匹配器则在锁内运行，因此应保持快速、无阻塞，并避免重入同一 mock。

本文件不创建线程、异步任务、通道、事务或外部资源。`Close` 只是一个可预期调用，没有真实资源释放；生命周期结束时也没有自动 `verify`，测试必须显式调用 `verify()`。所有 `Arc` 在 mock、recorder、期望句柄和回调释放后按 Rust 引用计数正常回收。

## 与 Go 版本的对应关系

[`sdk_mock.go`](sdk_mock.go) 是 MockGen 生成文件，包含同名的四组 mock/recorder 和相同接口方法。Rust 版本保留了 `NewMock*`、`EXPECT` 以及方法的大写命名，以降低 Go 测试迁移成本，同时提供 snake_case 构造与小写 `expect` 别名。

Go 版本把匹配、次数、顺序、回调和完成时验证交给 `gomock.Controller`/`gomock.Call`；Rust 版本没有引入 gomock 依赖，而是在 `MockQueue`、`Expectation`、`MockExpectation` 中实现等价的聚焦子集。Go recorder 把 context 也作为普通参数交给 gomock；Rust 的 `MockCall` 刻意不保存 context，而由可选 `ContextMatching` 单独校验。

Go 的指针返回与 `(value, error)` 双返回可同时携带非空值和错误。Rust trait 的主方法通常使用拥有值和 `Result`，同时通过 `GetTableMetasParts`、`SubmitJobParts`、`GenerateImportSQLParts` 等兼容方法及 `ReturnParts`/`DoAndReturn` 保留迁移测试需要的双返回位。独立测试 `go_return_slots_preserve_value_even_when_error_is_present`、`table_metas_can_return_value_and_error_together`、`trait_objects_forward_go_return_parts` 和 `callback_can_return_nonzero_id_with_error_through_trait` 验证了这一差异。

GoMock 支持更广的 matcher/action 与 controller 生命周期行为；本文件只能声称支持源码中明确实现的次数、依赖、参数/context 匹配、固定/动态响应和显式验证，不能推断为完整 GoMock 兼容实现。

## 扩展指南

- 给 `astersql-importsdk` trait 新增方法时，应同步增加 `MockCall` 变体、合适的 `MockResponse` 载荷、单职责 recorder、聚合 `MockSDKMockRecorder`、对应 trait impl，以及必要的共享 `dispatch_*` 辅助。若有 `*Parts` 兼容方法，还需更新固有方法与 trait 转发宏。
- 新方法的业务参数应进入 `MockCall`，context 继续通过 `ContextMatching` 处理；不要把不可长期持有的引用写进队列。复杂参数若沿用签名字符串匹配，应明确 `Debug` 表示变化带来的兼容风险；优先为动态回调保留强类型参数。
- 新响应必须在所有相关解包路径中做显式变体检查；不能用默认值掩盖类型错误。对于 trait 签名无法承载错误的方法，需要决定并测试是 panic 还是提供额外兼容 API。
- 调整次数或 `After` 语义时重点维护“不匹配不消费”“重复相同期望按登记顺序”“前置达到完成条件后才放行”“可选重复期望不导致 verify 失败”等不变量。
- 测试逻辑必须继续放在独立 [`sdk_mock_test.rs`](sdk_mock_test.rs)，不要内嵌回生产源文件。至少为新方法覆盖固定成功/错误响应、参数不匹配、聚合 `MockSDK` trait 转发；涉及 Go 双返回、context、动态回调或并发时增加相应专项用例。
- 兼容风险主要来自公开 Go 风格 API、返回位形状和 panic 边界；性能风险主要来自锁内 matcher、线性扫描期望队列、克隆调用/响应以及 `Debug` 字符串签名。该 mock 面向测试，应优先保证确定性与诊断质量，但仍应避免让用户回调在锁内执行。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件和 307,296 个节点；`files --filter pkg/importsdk/mock` 找到 `lib.rs`、`sdk_mock.rs`、`sdk_mock_test.rs` 与 Go 对照；`node --file pkg/importsdk/mock/sdk_mock.rs` 完整读取 1–1745 行并报告 164 个符号；另用 `node` 读取 `lib.rs`、四个上游 trait 文件、独立 Rust 测试和 Go 对照。精确 `callers`/`callees` 查询超时，无结果未被当作调用关系证据。
- 源与边界：[`sdk_mock.rs`](sdk_mock.rs) 的 `MockCall`、`MockResponse`、`Expectation`、`MockQueue::dispatch_internal`、四组 mock/recorder、trait impl 和 `dispatch_*`；[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs) 的 crate 依赖、模块与重导出；工作区根 `Cargo.toml` 和 `pkg/lib.rs` 的成员/facade 接线。
- 上游接口：[`file_scanner.rs`](../file_scanner.rs) 的 `FileScanner`、[`job_manager.rs`](../job_manager.rs) 的 `JobManager`、[`sql_generator.rs`](../sql_generator.rs) 的 `SQLGenerator`、[`sdk.rs`](../sdk.rs) 的 `SDK`。
- Go 对照：[`sdk_mock.go`](sdk_mock.go) 的 MockGen 来源注释、四组同名 mock/recorder、controller 派发和双返回位。
- 独立测试：[`sdk_mock_test.rs`](sdk_mock_test.rs) 覆盖乱序参数匹配、错误调用不消费期望、双返回位、次数与 `After`、`AnyTimes`、强类型 SQL 回调、自定义/context matcher、trait 对象转发及动态作业回调。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令检查目标存在且恰有 11 个固定二级标题，并人工复核链接、当前实现边界与扩展入口。
