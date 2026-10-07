# `pkg/config/const.rs`

## 文件定位

`pkg/config/const.rs` 属于 Cargo 包 `astersql-config`（见 `pkg/config/Cargo.toml`），集中保存配置域中与统计信息采样相关的公共常量。由于 `const` 是 Rust 关键字，crate 根模块 `pkg/config/lib.rs` 使用 `#[path = "const.rs"] mod config_const;` 装载该文件，再通过 `pub use config_const::*;` 把其中的公开项重导出到 `astersql_config` 顶层。因此调用者的稳定入口是 `astersql_config::DefRowsForSampleRate`，而不是公开的 `config_const` 模块路径。

当前文件不是配置文件解析器，也不保存可在运行时修改的配置；它只定义一个编译期数值。其 Go 对应文件是 `pkg/config/const.go`。在 Go 的完整 ANALYZE 链路中，该值由 `pkg/executor/builder.go::getAdjustedSampleRate` 用于自动计算统计信息采样率，并由 `pkg/executor/analyze_utils.go` 写入内存配额错误提示。

## 核心职责

- 用 `DefRowsForSampleRate` 为统计信息自动采样提供统一的目标行数基准，当前值为 `110_000`。
- 保持 Rust 配置 crate 的公开常量与 Go `pkg/config/const.go` 中的 `DefRowsForSampleRate = 110000` 数值和命名一致。
- 通过 `astersql-config` 顶层重导出，为后续 Rust 调用方提供不依赖私有模块名的公共 API。

需要区分“常量声明”和“运行时接线”：当前 Rust `pkg/executor/builder.rs::getAdjustedSampleRate` 使用函数内局部常量 `DESIRED_ROWS: f64 = 110_000.0`，没有引用本文件的 `DefRowsForSampleRate`；因此本文件现阶段在 Rust 生产代码中提供公共配置语义，但尚不是 Rust ANALYZE 实现的单一事实来源。仓库内直接 Rust 引用仅见 `pkg/config/const_3_aster_unit_test.rs`。

## 主要符号

### `pub const DefRowsForSampleRate: i64 = 110000`

这是文件内唯一的程序符号，也是公开 API。

- `pub const`：值在编译期确定，无惰性初始化、堆分配或运行时读取。
- `i64`：与 Go 无类型整数常量的使用方式不同，Rust 调用者若参与浮点除法，需要显式转换为 `f64`；若参与无符号计数运算，也必须先完成安全的类型转换。
- `110_000` 的语义：目标是自动 ANALYZE 时采样约十一万行。Go `getAdjustedSampleRate` 的注释说明期望约十万行，并用额外的一万行抵消统计元数据行数的轻微误差。
- 值本身不表示采样率；真正的采样率要由调用者依据表行数计算，并限制上界为 `1`。

文件中没有函数、类型、trait、`impl`、可变静态变量或条件编译项。

## 执行流程

本文件没有可执行控制流。围绕该常量的预期数据流可分为两层：

1. 编译模块时，`pkg/config/lib.rs` 将 `const.rs` 作为私有模块 `config_const` 装载。
2. `pub use config_const::*` 将 `DefRowsForSampleRate` 提升为 `astersql_config::DefRowsForSampleRate`。
3. Rust 单元测试 `constants_and_tiflash_mapping_match_go` 从 crate 顶层导入该常量并断言其值为 `110_000`。
4. Go 运行时的 `executorBuilder.getAdjustedSampleRate` 在已获得 `statsTbl.RealtimeCount`、未进入 PD 近似行数异常分支且实时行数非零时，计算 `min(1, DefRowsForSampleRate / RealtimeCount)`，并把同一常量写入采样原因字符串。
5. 当前 Rust 运行时 `ExecutorBuilder::getAdjustedSampleRate` 实现了相同的主要分支，但第 4 步使用局部 `DESIRED_ROWS: f64`，不是本文件的常量。相关行为测试位于 `pkg/executor/test/analyzetest/columns/analyze_columns_with_test.rs` 和 `pkg/executor/test/analyzetest/analyze_test.rs`。

因此，修改本文件会立即改变 Rust 公共 API 和常量单测的结果，却不会自动改变当前 Rust 执行器的采样行为；若要调整行为，必须同时审查并消除或同步 `builder.rs` 中的重复值。

## 数据与状态

`DefRowsForSampleRate` 是不可变的进程级编译期数据，不读取 TOML、环境变量、全局配置实例或数据库状态。它没有所有权转移问题，也不会产生缓存或持久化状态。

在算法语义上，调用方需要提供表行数：

- 当行数小于或等于 `110_000` 时，`min(1, 110_000 / row_count)` 得到全量采样率 `1`。
- 当行数大于 `110_000` 时，采样率随行数反比下降，使预期采样行数保持在约十一万。
- 零行数不能直接作为除数。Go 和 Rust 执行器都在进入该公式前单独处理零值。
- Rust 常量类型是 `i64`，而 Rust 执行器的行数计算使用 `f64`；接线时必须显式转换，并应先保持现有的零值与异常分支，避免除零或负数语义泄漏。

## 依赖与调用关系

上游装配关系：

- `pkg/config/lib.rs`：以 `config_const` 名称装载文件并重导出全部公开项。
- `pkg/config/Cargo.toml`：声明 crate 名为 `astersql-config`，库入口是 `lib.rs`。本常量只使用 Rust 原生整数类型，不直接依赖清单中的任何第三方 crate。

已确认的 Rust 直接使用者：

- `pkg/config/const_3_aster_unit_test.rs::constants_and_tiflash_mapping_match_go`：从 `astersql_config` 顶层导入并断言值为 `110_000`。

相关但尚未接线的 Rust 生产路径：

- `pkg/executor/builder.rs::ExecutorBuilder::getAdjustedSampleRate`：以局部 `DESIRED_ROWS` 实现自动采样公式和原因文本，行为上对应本常量，但没有形成源码依赖边。

Go 生产调用关系：

- `pkg/executor/builder.go::executorBuilder.getAdjustedSampleRate`：直接读取 `config.DefRowsForSampleRate`，计算正常统计元数据分支的采样率并构造原因文本。
- `pkg/executor/analyze_utils.go::errAnalyzeOOM`：把该值嵌入 OOM 建议，提示使用更小的 `samplerate`。

RustCodeGraph 能定位 Go `builder.go` 使用点和 Rust 单测使用点，但当前索引未把 `pkg/config/const.rs` 的 Rust `pub const` 正确返回为常量定义；因此生产引用全集又用仓库级 `rg` 交叉核验。

## 错误处理与边界

本文件不返回 `Result`、不构造错误，也不执行输入校验。边界责任都在消费该值的算法中：

- 表行数未知时不能凭此常量推导采样率；Go/Rust 执行器使用默认率或存储层近似计数。
- 表行数为零时必须先返回全量采样率，不能直接除零。
- 当统计元数据计数远小于 PD 近似计数时，现有实现走独立的 `150_000 / approximate_count` 修正分支，本常量不控制该分支。
- `min(1, ...)` 是小表不产生大于 `1` 的采样率这一关键不变量。
- 修改数值会改变用户可见的 ANALYZE warning 原因字符串及采样成本；Go 测试中存在包含 `110000/...` 的精确字符串断言，Rust 测试也有相同文本期望。

当前最重要的迁移边界是双重常量：只修改 `DefRowsForSampleRate` 会造成公共常量、Go 行为与 Rust 执行器行为不一致，而编译器不会对此报错。

## 并发与资源生命周期

该编译期常量没有初始化顺序、锁、原子变量、通道、异步任务或析构过程。任何线程都可以无同步成本读取它，读取不会产生副作用。

资源影响发生在下游而非本文件：目标行数越大，ANALYZE 通常读取、传输和处理的数据越多，可能增加 CPU、内存、网络和存储扫描开销；目标行数越小则可能降低统计质量并影响优化器估算。`pkg/executor/analyze_utils.go` 的 OOM 提示也表明采样规模与内存压力直接相关。调整该常量应被视为查询规划质量与资源消耗之间的兼容性决策，而不是纯格式变更。

## 与 Go 版本的对应关系

`pkg/config/const.go` 同样只声明 `DefRowsForSampleRate = 110000`，Rust 保留了名称和值。主要差异如下：

- Go 常量没有固定整数类型，可直接在 `config.DefRowsForSampleRate / float64(statsTbl.RealtimeCount)` 中按上下文转换；Rust 固定为 `i64`，浮点运算必须显式转换。
- Go 生产执行器直接引用配置包常量，且 OOM 错误提示也引用同一常量；Rust 生产执行器当前复制为局部 `f64` 常量，Rust 侧尚无对应的 OOM 引用边。
- Go `analyze_test.go` 对大表 `220000` 验证采样率 `0.5`，对小表 `3` 验证采样率封顶为 `1`，并精确核对原因字符串。Rust 的 `analyze_columns_with_test.rs` 核对 `110000/10000` 得到 `1` 的提示，`analyze_test.rs` 还验证会产生采样率说明；常量本身则由独立文件 `const_3_aster_unit_test.rs` 校验。

所以“数值移植”已经完成，“Rust 生产代码统一通过配置常量取值”尚未完成。文档不把后者描述为已支持。

## 扩展指南

若新增同类配置常量，应放在语义所属的最小模块中，并通过 `pkg/config/lib.rs` 的既有重导出机制暴露；不要把运行时可变配置误建模为 `const`。新增 Rust 测试应继续放在独立测试文件中，不要嵌入 `const.rs`。

若修改 `DefRowsForSampleRate`：

1. 先确认 Go 对照值是否同时变化，以及是否仍要求 Go/Rust 行为一致。
2. 同步审查 `pkg/executor/builder.rs` 的 `DESIRED_ROWS`；更安全的长期接线方式是由执行器引用 `astersql_config::DefRowsForSampleRate` 并在运算点显式转为 `f64`，从而删除重复事实来源。
3. 更新 `pkg/config/const_3_aster_unit_test.rs` 的常量断言。
4. 更新 Rust `pkg/executor/test/analyzetest/columns/analyze_columns_with_test.rs` 中的精确原因文本，并补充/保持大表降采样、小表封顶、零行和 PD 近似计数异常分支测试。
5. 若 Go 值也变更，同步更新 `pkg/config/const.go`、`pkg/executor/test/analyzetest/analyze_test.go` 及其他包含精确 `110000/...` 输出的 Go 测试。
6. 评估统计质量、ANALYZE 时间、内存峰值和用户可见 warning 文本的兼容性；常量名属于公开 Rust API，也应避免无迁移方案的重命名或类型变更。

## 验证依据

本说明基于以下直接证据：

- 源文件：`pkg/config/const.rs`，确认唯一符号为 `pub const DefRowsForSampleRate: i64 = 110000`。
- crate 边界：`pkg/config/Cargo.toml` 与 `pkg/config/lib.rs`，确认包名、库入口、私有模块装载及顶层重导出。
- Rust 独立测试：`pkg/config/const_3_aster_unit_test.rs::constants_and_tiflash_mapping_match_go`，确认公开导入路径和值断言。
- Rust 相关实现：`pkg/executor/builder.rs::ExecutorBuilder::getAdjustedSampleRate`，确认当前使用局部 `DESIRED_ROWS`；`pkg/executor/test/analyzetest/columns/analyze_columns_with_test.rs` 与 `pkg/executor/test/analyzetest/analyze_test.rs`，确认采样率提示行为。
- Go 对照：`pkg/config/const.go`、`pkg/executor/builder.go::executorBuilder.getAdjustedSampleRate`、`pkg/executor/analyze_utils.go::errAnalyzeOOM`，确认数值、正常分支公式和错误提示用途。
- Go 测试：`pkg/executor/test/analyzetest/analyze_test.go` 及 `columns/analyze_columns_with_test.go`，确认大表反比采样、小表封顶和用户可见原因文本。
- RustCodeGraph 查询：运行了 `status`、`explore`、`files --filter pkg/config`、`node --file pkg/config/const.rs`、`query/node/callers/callees DefRowsForSampleRate`。索引能找到 Go 使用点与 Rust 单测使用点，但未正确识别 Rust 常量定义，故用 `rg -n '\bDefRowsForSampleRate\b'` 补足并交叉核验引用。

本任务是纯文档分析，没有运行 Cargo，也没有声称执行器测试已在本次会话中执行。结构验证应保证文档存在且恰含任务要求的十一个固定二级标题；人工复核重点是公开 API、当前未接线事实和 Go/Rust 边界均有明确源码依据。
