# `pkg/session/nontransactional.rs`

## 文件定位

本文件属于 `astersql-session` crate：`pkg/session/Cargo.toml` 将 crate 根设为 `pkg/session/lib.rs`，后者通过 `pub mod nontransactional` 公开本模块。它是 Rust 侧非事务 DML（SQL 语法中的 `BATCH ... LIMIT ...`）核心算法实现：把一条可能影响大量行的 DML 按有序分片列切成多个闭区间作业，每个作业独立执行，从而避免形成一个超大事务。

文件公开了完整的算法入口 `HandleNonTransactionalDML`、运行时抽象 `NonTransactionalRuntime`、输入模型和结果模型，但仓库搜索没有找到 Rust 生产代码对入口的调用，也没有找到生产类型实现该 trait；直接实现者只有 `pkg/session/nontransactional_test.rs:ScanRuntime` 和 `pkg/session/test/nontransactionaltest/nontransactional_test.rs:StubRuntime`。因此当前事实是“算法和局部测试已存在并由 crate 导出”，不能断言它已经接入 Rust server 的 SQL 执行主链。对应的 Go 入口 `pkg/session/nontransactional.go:HandleNonTransactionalDML` 则由 `pkg/server/driver_tidb.go:TiDBContext.ExecuteStmt` 和 `pkg/testkit/testkit.go` 调用。

本文件没有条件编译项；`pkg/session/lib.rs` 仅用 `#[cfg(test)] mod nontransactional_test` 将独立单元测试接入测试构建。源码没有内嵌测试，符合生产逻辑与测试逻辑分文件的仓库约定。

## 核心职责

- 在 `HandleNonTransactionalDML` 中临时调整会话状态，执行预处理和约束校验，选择分片列，生成扫描 SQL，构建作业并执行或生成 dry-run 结果。
- 用 `checkConstraint`、`checkConstraintWithShardColumn` 等函数拒绝事务内执行、传统 batch DML、弱一致读、快照读、带 `LIMIT`/`ORDER BY` 的语句、非 `INSERT ... SELECT` 的 insert，以及修改分片列的赋值。
- 用 `selectShardColumn*` 从整数主键、单列聚簇主键、显式指定的公开可见索引首列或隐式 `_tidb_rowid` 中确定分片列；多表场景要求显式列具有完整的 schema/table/column 限定。
- 用 `buildSelectSQL` 生成按“NULL 在前、随后按分片值排序”的扫描 SQL，再由 `buildShardJobs` 按批大小切分连续闭区间。相等的边界值不会跨作业拆开，所以 `jobSize` 可超过 `limit`。
- 用 `runJobs` 串行处理作业；`doOneJob` 构造分片范围条件，与原始条件合取，恢复 DML SQL，并通过运行时执行。
- 用 `buildDryRunResults` 和 `buildExecuteResults` 返回稳定的结果列形状，并集中处理失败汇总、日志脱敏和用户可见错误预览。
- 用本地 `Datum`/`FieldType`、表和 AST 摘要类型隔离算法与真实 parser/session/storage 对象；所有生产副作用经 `NonTransactionalRuntime` 注入。

## 主要符号

- `EXTRA_HANDLE_NAME`、`DRY_RUN_NONE`、`DRY_RUN_QUERY`、`DRY_RUN_SPLIT_DML`：隐式行句柄名和三种执行模式。`ErrNonTransactionalJobFailure` 是与 Go 错误语义对应的文本常量，但当前 Rust 函数并未直接引用它。
- `NonTransactionalError` 与 `Result<T>`：模块自有的字符串错误类型和结果别名；不携带结构化错误码或错误链。
- `FieldType`、`Datum`：分片值类型系统。`Datum::compare` 支持 NULL、跨有符号/无符号整数、可选大小写不敏感文本、二进制和十进制比较；`to_sql_literal` 负责安全生成相应 SQL 字面量。
- `ColumnName`、`Assignment`、`ColumnInfo`、`IndexInfo`、`TableInfo`、`TableSource`、`ResultSetNode`：算法需要的最小名称、赋值、表/列/索引和 join 树模型。
- `DmlKind`、`DmlStatement`、`NonTransactionalDMLStmt`：支持的 delete、update、insert-select 输入。`DmlStatement::restore_with_condition` 要求 `sql_template` 中恰好有一个 `{WHERE}` 占位符。
- `SessionVars`：只包含本流程读取或临时修改的会话变量快照；它不是完整 session variables 类型。
- `NonTransactionalRuntime`：公开副作用接口。它要求实现会话状态访问、预处理、指标、内存跟踪、分片扫描/比较、DML 执行、取消检查和日志；注释明确禁止生产实现静默跳过这些副作用。
- `RuntimeRecordSet`：只要求 `close`，用于回收单个 DML 返回的可选结果集。
- `job`：一个 `[start, end]` 闭区间，包含 1-based `jobID`、估计行数、最终 SQL 和可选错误。名称与字段保留 Go 风格，因此模块启用了命名 lint 允许项。
- `statementBuildInfo`：向单作业 SQL 构造传递语句、副本化的分片列元数据和原始条件。
- `HandleNonTransactionalDML`：顶层编排入口。
- `checkConstraint*`、`checkTableRef`、`checkReadClauses`：静态及会话约束层。
- `buildSelectSQL`、`selectShardColumn*`、`collectTableSourcesInJoin`：表源收集、分片列选择及扫描语句生成层。
- `buildShardJobs`、`appendNewJob`：有序值到作业区间的切分层。
- `runJobs`、`doOneJob`、私有 `build_job_condition`：串行执行与 SQL 恢复层。
- `buildDryRunResults`、`buildExecuteResults`：外部结果集构造层。

## 执行流程

1. `HandleNonTransactionalDML` 保存 `read_staleness`、`bulk_dml_enabled`、`in_non_transactional_dml`，随后分别设置为 `0`、`false`、`true`。无论内部闭包如何返回，函数末尾都会恢复三项原值。
2. 入口先调用运行时 `preprocess`，再由 `checkConstraint` 检查必须处于 autocommit 且不在显式事务中，并排除 batch DML、弱读、snapshot 以及不支持的 DML 形状；成功的 delete/update/insert-select 分别递增指标。
3. `buildSelectSQL` 深度优先、先左后右地收集 join 树中的 `TableSource`，用最左表或显式全限定列确定目标表和分片列。它将无原始条件视为 `TRUE`，生成 `SELECT <shard> FROM <schema>.<table> WHERE <condition> ORDER BY IF(ISNULL(<shard>),0,1),<shard>`。
4. `checkConstraintWithShardColumn` 检查 update 或 insert-select 的 on-duplicate 赋值。schema 比较允许省略当前数据库，单表 update 允许省略表名；命中分片列即拒绝。
5. `DRY_RUN_QUERY` 在这一点直接返回扫描 SELECT，不挂内存跟踪器、不扫描数据，也不生成作业。
6. 其他模式通过运行时挂载以内存配额为上限的跟踪器。`buildShardJobs` 临时把 `select_limit` 设为 `u64::MAX`、把 `max_execution_time` 设为 `0`，扫描有序值后立即恢复；即使扫描失败也先恢复，再传播错误。
7. 分片循环在当前作业达到 `limit` 且新值与前一值不相等时封口。这样所有相同分片键留在同一个作业，避免相邻闭区间重叠执行同一键。每次 `appendNewJob` 为起止 `Datum` 和固定 64 字节开销计入内存，并分配连续的 1-based ID。
8. `runJobs` 串行遍历作业。每个迭代先检查取消；split-DML dry-run 只为首尾作业调用 `doOneJob` 并收集示例，中间作业不构造 SQL；正常模式对每个作业调用执行路径。
9. `build_job_condition` 对全 NULL 区间生成 `IS NULL`，对从 NULL 开始的混合区间生成 `<= end OR IS NULL`，其他区间生成 `BETWEEN start AND end`，然后与原始条件用 `AND` 合取。`doOneJob` 替换唯一 `{WHERE}` 标记；正常执行时加上 `/* job i/n */` 注释，并关闭可选结果集。
10. 首作业失败总是提前返回；后续失败在 `ignore_error == false` 时立即返回，在 `true` 时继续并最终由 `buildExecuteResults` 汇总。全部成功则返回 `number of jobs` 和 `job status=all succeeded`；失败摘要完整写日志，用户错误只取前 500 个字符。
11. 内存跟踪器最后必定尝试 detach：主流程错误优先于 detach 错误，只有主流程成功而 detach 失败时才返回 detach 错误。随后入口恢复最外层三项会话状态。

## 数据与状态

`NonTransactionalDMLStmt` 是主要输入，其中 `selectShardColumnAutomatically` 会回填 `shard_column`，所以入口接受 `&mut`。`DmlStatement` 保存简化 DML 结构和原始 WHERE 字符串；执行期间不直接修改模板，而是为每个作业替换 `{WHERE}`，避免前一作业的范围条件泄漏到后一作业。

作业边界是闭区间，并依赖扫描结果按分片列排序。NULL 必须排在最前；生成的扫描 SQL 显式保证这一点。`buildShardJobs` 假定运行时返回值保持该顺序，不自行重新排序。相同值不拆分是正确性不变量：若某个值出现次数超过批大小，作业会变大而不会把相同键分入相邻的两个 `BETWEEN` 区间。

`jobSize` 是扫描时的行数估计；与 Go 注释一致，并发写入可能使其与真正修改行数不同。空扫描返回零个作业，成功结果仍是 `0 / all succeeded`。`SimpleRecordSet.max_chunk_size` 来自会话变量，不参与切分。

会话状态有三层临时变更：入口级的 read staleness/bulk-DML/in-NT-DML 标志，扫描级的 select limit/max execution time，以及运行时拥有的内存跟踪器。恢复逻辑均在错误传播之前执行。指标在约束校验通过具体 DML 形状时递增，发生在表解析、分片列选择或实际执行之前，因此它表示通过基础语句约束的尝试，不等于成功执行次数。

## 依赖与调用关系

Rust 上游边界：

- `pkg/session/lib.rs` 公开 `nontransactional` 模块，并在测试构建中挂载 `pkg/session/nontransactional_test.rs`。
- RustCodeGraph 将 `pkg/session/nontransactional.rs` 标记为被测试和若干会话文件引用，但对 `nontransactional.rs::HandleNonTransactionalDML` 的 callers 查询没有给出可用生产调用边；仓库级精确搜索也只找到函数定义。
- `NonTransactionalRuntime` 的仓库级实现只有两个测试桩，因此当前没有可证实的 Rust session adapter 把真实 parser AST、存储扫描和 DML executor 接到入口。

模块内部主调用链为：

`HandleNonTransactionalDML -> preprocess/checkConstraint -> buildSelectSQL -> selectShardColumn* -> checkConstraintWithShardColumn -> buildShardJobs -> appendNewJob -> runJobs -> doOneJob -> execute_dml -> buildExecuteResults`。两条 dry-run 分支分别在 `buildSelectSQL` 后返回查询文本，或在 `runJobs` 中只恢复首尾拆分 SQL。

下游的唯一外部 Rust crate 调用是 `redact_sql -> astersql_util_redact::String`，对应 `pkg/session/Cargo.toml` 中的 `astersql-util-redact` 路径依赖。其余 parser、session、storage、metrics、memory tracker、日志和取消功能均由本地模型及 `NonTransactionalRuntime` 抽象表示；Cargo 中大量 session 依赖并不意味着本文件直接使用它们。本模块不受 `nextgen` feature 控制。

Go 生产调用链更完整：`pkg/server/driver_tidb.go:TiDBContext.ExecuteStmt` 和 `pkg/testkit/testkit.go` 识别 `*ast.NonTransactionalDMLStmt` 后调用 `pkg/session/nontransactional.go:HandleNonTransactionalDML`，后者直接使用 preprocess、InfoSchema、session execute、metrics、memory tracker 和 record set。

## 错误处理与边界

本模块采用遇错返回与作业内记错并存的策略。预处理、约束、元数据、扫描、比较、内存计量、取消和跟踪器 detach 错误通过 `Result` 返回；单作业的范围构造、模板恢复或 DML 执行错误先写入 `job.err`，再由 `runJobs` 根据作业位置和 `ignore_error` 决定停止或继续。

首作业失败无条件提前返回，因为代码假设后续作业很可能同样失败。非首作业在不忽略错误时返回带 job ID、总数、起止值、脱敏作业描述和底层错误的消息；忽略错误只允许继续尝试，最终仍由 `buildExecuteResults` 返回整体失败，不会把部分失败报告为成功。取消错误优先直接返回，同时用已经完成的作业生成告警摘要。

重要输入边界包括：批大小必须能转换为正 `usize`；table refs 必须存在；join 树只接受 join/table 节点；显式分片列必须存在且是整数 handle 或公开可见索引首列；复合聚簇主键无法自动选列；多表显式列必须全限定；分片列不能被本语句更新；SQL 模板必须恰有一个 `{WHERE}`。

`Datum::to_sql_literal` 对文本单引号加倍、二进制编码为十六进制字面量、十进制先验证再原样输出，避免直接拼接未验证内容；标识符中的反引号也会加倍。十进制验证只接受可选正负号、非空整数部和可选非空小数部，不支持指数写法。大小写不敏感文本比较使用 Unicode `to_lowercase`，只是本地近似；真正的 Go 版本使用列 collation，生产 adapter 若出现必须保证 `compare_shard_values` 按数据库排序规则比较。

`RuntimeRecordSet::close` 的错误在 `doOneJob` 中被显式忽略，与 Go 行为一致。日志脱敏由 `OFF`/`ON`/`MARKER` 契约处理；错误摘要只按 Rust `chars()` 截取 500 个字符，而 Go 对字符串按字节切片，这对非 ASCII SQL 的截断边界可能不同。

## 并发与资源生命周期

`runJobs` 明确是单线程 worker，作业按 ID/切分顺序同步执行；没有线程、异步任务、锁或通道。这个顺序保证错误观察、首作业提前退出以及“忽略错误后继续”的行为确定。它也意味着总耗时是各作业执行时间之和，不能把文中的多个 job 理解成并行事务。

每个正常 DML 作业经 `execute_dml` 返回的可选 record set 会在同一迭代中关闭。扫描结果在 trait 边界已经物化成 `Vec<Datum>`，本文件不持有扫描 record set；真实运行时负责扫描资源的打开与关闭。入口挂载一次内存跟踪器，按作业边界值持续消费，并在所有作业或 dry-run split 结束后 detach。

取消在每个作业开始前检查，正在执行的单个 `execute_dml` 是否可被中止取决于运行时实现。`&mut dyn NonTransactionalRuntime` 排除了同一运行时被本函数并发可变访问，但不能替代外部数据库并发控制；扫描完成后发生的写入可能改变实际命中行数，这也是 `jobSize` 仅为估计值的原因。

状态恢复由入口与扫描函数手工保存/恢复实现，不依赖 Rust `Drop` guard。当前同步闭包的所有普通 `Result` 路径都会恢复，但若运行时方法 panic，状态和 tracker 不保证恢复；未来接入生产 runtime 时这是值得评估的可靠性风险。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/session/nontransactional.go`。两版的核心结构和顺序基本一致：入口临时关闭 read staleness/bulk DML；预处理与约束；选分片列并生成 NULL-first 扫描；忽略 select limit/max execution time；相等键不跨 batch；作业串行执行；首作业失败提前退出；ignore-error 继续后汇总；dry-run query/split 两种结果；成功返回 job 数和 `all succeeded`。

Rust 用显式数据模型和 trait 取代 Go 的真实对象：Go 输入是 parser AST，借助 resolve context、InfoSchema 和 `table.Table` 获取表信息，通过 `sessionapi.Session.Execute/ExecuteStmt` 执行，并使用 session metrics、memory tracker、logutil、context 和 failpoint；Rust 输入是已经转换的 `DmlStatement`/`TableInfo`，副作用全部由 `NonTransactionalRuntime` 提供。这使算法可独立测试，但生产 adapter 尚未找到。

可见差异与迁移限制包括：

- Go 给 context 增加 `NTDML-<label>` statement label；Rust 模型没有 context/label。
- Go 使用 parser AST 重建 WHERE/DML，可保留完整语法语义；Rust 依赖调用者提供唯一 `{WHERE}` 模板和字符串条件。
- Go 从真实 record set 分 chunk 流式扫描；Rust trait 一次返回全部 `Vec<Datum>`，因此除作业边界计量外，扫描值本身的内存归属和峰值由 runtime 决定。
- Go 用数据库 `FieldType`、`Datum.Compare` 和 collator；Rust 本地枚举只覆盖 signed/unsigned/text/binary/decimal，并把真实 collation 委托给 `compare_shard_values`。
- Go 的标准 session 错误包含错误类/码与堆栈；Rust 只有消息字符串，`ErrNonTransactionalJobFailure` 也只是未使用的字符串常量。
- Go 通过 failpoint 覆盖 batch DML 错误和 max-execution-time 恢复；Rust 源码不含 failpoint，独立测试用 stub 验证局部契约。
- Go 的错误预览按字节截取；Rust 按 Unicode 字符截取。Rust 在 detach tracker 失败时可返回错误，而 Go 的 `Detach` 没有返回值。

Go 测试 `pkg/session/test/nontransactionaltest/nontransactional_test.go` 覆盖真实 SQL 分片、错误文本、check constraint、外键、指标和 max execution time。对应 Rust 文件开头把大部分 Go 测试保存在 `_GO_DRAFT_ARCHIVE` 字符串中；真正可执行的 Rust 用例从文件后半开始，主要验证纯函数/桩逻辑，并用逐点 update/delete 模拟可见性，不能等同于完整 batch SQL 端到端覆盖。

## 扩展指南

若新增 DML 种类或约束，应同步修改 `DmlKind`、`checkConstraint`、表引用/读子句提取和指标分支，并在独立文件 `pkg/session/nontransactional_test.rs` 或 `pkg/session/test/nontransactionaltest/nontransactional_test.rs` 增加成功及拒绝路径；不要把测试放入生产源文件。新增分片类型必须成对扩展 `FieldType`、`Datum::compare`、`Datum::to_sql_literal`、内存估算和真实 runtime 的 collation/type 转换，否则扫描顺序与 SQL 边界可能不一致。

若接入 Rust 生产主链，最关键的新组件是实现 `NonTransactionalRuntime` 的 session adapter，并在 Rust server 的非事务 AST 分发点调用 `HandleNonTransactionalDML`。adapter 必须证明：预处理和元数据解析真实、扫描按数据库 collation 排序、扫描 record set 始终关闭、DML 每作业独立提交、取消可传播、指标/日志/内存跟踪不是空操作、parser AST 能无损产生唯一 `{WHERE}` 模板。接入前应补真正调用入口的端到端 SQL 测试，而不是只依赖 `StubRuntime`。

修改切分算法时必须保留三个不变量：NULL 排序和范围覆盖完整、相等分片值不得跨相邻闭区间、原始 WHERE 对每个作业都只合取一次。性能优化若改为流式扫描，可避免 `Vec<Datum>` 峰值，但需要把 record set 生命周期、chunk 边界 clone 和错误时关闭资源纳入 trait；若尝试并行执行，则会改变当前确定的错误顺序、首作业短路和 session 可变状态模型，不能作为局部优化直接加入。

兼容性风险集中在 SQL 恢复格式、错误消息、结果字段名、脱敏方式以及 Go/Rust collation 差异；性能风险集中在全量值物化、单线程作业执行和大量失败字符串汇总。回归至少应覆盖空表、NULL、重复键超过 limit、文本/二进制/十进制边界、多表别名、复合聚簇键、取消、首/中间作业失败、两种 dry-run 和所有状态恢复路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/session/nontransactional.rs --offset ...` 分段读取并核对了 1,330 行完整实现；`query HandleNonTransactionalDML --kind function --json` 精确定位 Rust 与 Go 两个同名入口。`callers nontransactional.rs::HandleNonTransactionalDML` 未返回可用调用边，因此又用仓库精确搜索核对接线状态。
- Rust 模块与 crate：`pkg/session/lib.rs`、`pkg/session/Cargo.toml`。目标包没有 `doc.go`；因此以 crate 根注释和 Cargo 元数据作为最近的 Rust 包契约。
- Rust 直接测试：`pkg/session/nontransactional_test.rs:invalid_batch_size_is_checked_after_the_go_equivalent_scan` 验证零 batch 在扫描后才报错，且扫描期间临时变量与返回后的恢复值正确；`pkg/session/test/nontransactionaltest/nontransactional_test.rs` 的可执行用例验证切分数量公式、读子句拒绝、Datum 比较/字面量、执行与 dry-run 结果形状、脱敏错误及轻量存储可见性。
- Go 对照：`pkg/session/nontransactional.go` 的 `HandleNonTransactionalDML`、`runJobs`、`doOneJob`、`buildShardJobs`、`buildSelectSQL`、`selectShardColumn*`、结果构造函数；生产调用者为 `pkg/server/driver_tidb.go:TiDBContext.ExecuteStmt` 和 `pkg/testkit/testkit.go`。
- Go 测试：`pkg/session/test/nontransactionaltest/nontransactional_test.go` 的 `TestNonTransactionalDMLSharding`、`TestNonTransactionalDMLErrorMessage`、`TestNonTransactionalWithCheckConstraint`、`TestNonTransactionalDMLWorkWithForeignKey`、`TestNonTransactionalMetrics`、`TestNonTransactionalDmlIgnoreMaxExecutionTime`。
- 本任务仅新增说明文档，按计划没有运行 Cargo。交付前执行任务指定的结构命令，要求文件存在且固定二级章节恰好为 11 个；另人工核对本文明确回答了文件为何存在、当前如何运行、真实接线限制以及如何安全扩展。
