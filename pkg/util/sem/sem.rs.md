# `pkg/util/sem/sem.rs` 逻辑说明

## 文件定位

`pkg/util/sem/sem.rs` 是 `astersql-util-sem` crate 的 SEM v1（Security Enhanced Mode，安全增强模式）实现文件。crate 入口 `pkg/util/sem/lib.rs` 以私有 `mod sem` 装载本文件，再用 `pub use sem::*` 公开这里的八个函数；因此调用方看到的是 crate 根 API，而不是 `sem` 子模块。

该文件负责保存 v1 的进程级启用状态，并提供固定规则来判定需要隐藏的 schema、表、状态变量、系统变量以及不能由 `SUPER` 兜底的动态权限。它不负责从 SQL 或配置解析 SEM，也不会主动把隐藏规则施加到查询结果上；实际调用方先检查 `IsEnabled`，再在规划、表达式、权限和兼容层路径中使用各个判定函数。`pkg/util/sem/compat/sem.rs` 是 v1/v2 的统一门面，也是多数上层 Rust 模块依赖的入口。

## 核心职责

- `Enable` / `Disable` / `IsEnabled` 管理并读取进程级 v1 开关，同时同步 `tidb_enable_enhanced_security` 和 `hostname` 系统变量（`sem.rs:67-103`）。
- `IsInvisibleSchema`、`IsInvisibleTable`、`IsInvisibleStatusVar`、`IsInvisibleSysVar` 实现硬编码的元数据可见性黑名单（`sem.rs:142-259`）。这些函数本身不检查 SEM 是否启用，调用方必须负责启用态门控。
- `IsRestrictedPrivilege` 识别以 `RESTRICTED_` 开头且前缀之后仍有内容的全大写动态权限名（`sem.rs:261-276`）。
- `unicode_simple_fold_to_ascii` 和 `equal_fold_ascii` 为 schema 名比较补足 Go `strings.EqualFold` 对 ASCII 常量的 Unicode simple-fold 语义（`sem.rs:105-140`）。

本文件没有类型、trait、`impl` 或条件编译项；模块级状态只有 `semEnabled: AtomicI32`，其余模块级项目是隐藏名单常量和一个权限前缀常量。

## 主要符号

| 符号 | 可见性 | 语义 |
| --- | --- | --- |
| `semEnabled` | 私有静态变量 | `AtomicI32` 进程级开关，`0` 表示关闭、`1` 表示开启。 |
| `Enable()` | `pub` | 原子写入 `1`，把增强安全变量设为 `ON`，把 `hostname` 设为 `vardef::DefHostname`，并写一条后台日志。 |
| `Disable()` | `pub` | 原子写入 `0`，把增强安全变量设为 `OFF`；若能读取操作系统主机名，则恢复 `hostname`。 |
| `IsEnabled() -> bool` | `pub` | 原子读取开关并判断是否为 `1`。 |
| `unicode_simple_fold_to_ascii(char) -> Option<u8>` | 私有 | 将 ASCII、Kelvin sign `K` 和 long-s `ſ` 映射到对应 ASCII 小写字节；其他非 ASCII 字符拒绝映射。 |
| `equal_fold_ascii(&str, &str) -> bool` | 私有 | 逐 Unicode scalar 与逐 ASCII 字节比较，模拟本文件所需的 Go simple-fold 等价类；长度不同或任一字符无法映射时返回 `false`。 |
| `IsInvisibleSchema(&str) -> bool` | `pub` | 对 `metrics_schema` 做大小写不敏感的 simple-fold 比较。 |
| `IsInvisibleTable(&str, &str) -> bool` | `pub` | 根据已小写的库名选择固定表黑名单；`metrics_schema` 下所有表都隐藏。 |
| `IsInvisibleStatusVar(&str) -> bool` | `pub` | 仅匹配 `tidb_gc_leader_desc`。 |
| `IsInvisibleSysVar(&str) -> bool` | `pub` | 在固定 `vardef` 常量集合及插件变量 `tidb_audit_redact_log` 中做精确匹配。 |
| `IsRestrictedPrivilege(&str) -> bool` | `pub` | 断言输入已大写，长度至少为 12，再检查 `RESTRICTED_` 前缀。 |

表隐藏常量按四组使用：`mysql` 六张内部表、`information_schema` 十二张集群/巡检/metrics 表、`performance_schema` 十三张 profiling 表，以及整库隐藏的 `metrics_schema`（`sem.rs:30-61,153-212`）。系统变量名单在 `IsInvisibleSysVar` 内保持 Go 源码顺序（`sem.rs:224-257`）。

## 执行流程

启用流程从 `Enable` 开始：先以顺序一致原子写入启用态，再通过 `variable::SetSysVar` 设置增强安全开关与默认主机名，最后通过 `BgLogger().info` 记录启用事件。关闭流程先清除原子状态，再关闭增强安全系统变量；只有 `hostname::get()` 成功时才恢复真实主机名，失败时保持当前主机名。

可见性查询是纯判定流程。schema 查询经 `equal_fold_ascii` 只匹配 `metadef::MetricSchemaName.L`。表查询依次按 `mysql`、`information_schema`、`performance_schema` 和 `metrics_schema` 分支：前三者检查各自黑名单，最后一类无条件隐藏任意表，其他库返回 `false`。状态变量和系统变量分别对单值与数组做精确、大小写敏感匹配。

权限查询先用 `intest::Assert` 检查调用约定：权限名必须已大写。随后长度小于 12 的字符串直接返回 `false`，所以仅有 11 字节的 `RESTRICTED_` 不算受限权限；其余输入由 `starts_with("RESTRICTED_")` 决定。

## 数据与状态

唯一可变状态是 `semEnabled`。它不按 session、租户或事务隔离，而是当前进程内所有线程共享。开关值与系统变量是两个不同的数据面：原子值供 `IsEnabled` 快速读取，`variable::SetSysVar` 修改全局系统变量注册表；代码没有把两者封装为可回滚事务。

隐藏名单全部编译进二进制，不从磁盘或网络加载。调用约定也构成数据不变量：`IsInvisibleTable` 的库名和表名应已小写，`IsInvisibleSysVar` 的参数应是小写系统变量名，`IsRestrictedPrivilege` 的参数应已大写。只有 `IsInvisibleSchema` 明确执行 Unicode simple-fold 比较。

## 依赖与调用关系

`pkg/util/sem/Cargo.toml` 声明 crate 名为 `astersql-util-sem`，并依赖：`variable`/`vardef` 读写系统变量及名称常量，`metadef` 提供 schema 名，`parser_mysql` 提供 `mysql` 库名，`hostname` 获取 OS 主机名，`logutil` 写启用日志，`intest` 执行开发/测试期断言。`testsetup` 仅是 dev-dependency。

下游直接调用边包括：

- `pkg/util/sem/compat/sem.rs` 将本 crate 引入为 `sem`，所有兼容查询先检查 v1/v2 互斥，再以 `sem::IsEnabled()` 门控并转发到对应 v1 判定函数。
- `pkg/expression/extension.rs:201` 通过 Cargo 别名 `sem-dependency` 直接读取 v1 `IsEnabled`，用于表达式扩展行为门控。
- `pkg/planner/core/expression_rewriter.rs:3396` 通过名为 `sem` 的兼容依赖组合 `IsEnabled` 与 `IsInvisibleSysVar`，阻止规划器暴露被隐藏系统变量。
- `pkg/planner/core/planbuilder.rs:1141` 通过兼容依赖查询 SEM 启用态。
- `pkg/privilege/privileges/privileges.rs:272-276,655-658` 和 `cache.rs:1046` 通过兼容 crate 将启用态与表/schema 隐藏、受限权限规则组合起来。

RustCodeGraph 对 `equal_fold_ascii -> unicode_simple_fold_to_ascii` 和 `IsInvisibleSchema -> equal_fold_ascii` 给出了内部调用边；对若干常见 PascalCase 名称的调用方查询混入了同名 Go/Rust 符号，因此上述跨 crate 边以精确 `rg` 结果和对应 Cargo 依赖再次核对。

## 错误处理与边界

`Enable` 和 `Disable` 对 `variable::SetSysVar` 使用 `expect`：依赖的系统变量未注册时会 panic，而不是返回错误。`pkg/util/sem/migration_aster_unit_test.rs` 明确指出 Go 包依靠 `init` 注册这些变量，而独立 Rust 测试必须先补注册；这说明注册表初始化是调用前置条件。

`Disable` 将 OS 主机名获取失败视为可忽略情况，不恢复 `hostname`，与 Go 的 `if err == nil` 行为一致。主机名用 `to_string_lossy` 转为字符串，因此非 UTF-8 字节可能被替换字符表示。

名称匹配除 `IsInvisibleSchema` 外均大小写敏感；例如测试确认 `IsInvisibleTable("MYSQL", "tidb")` 和 `IsInvisibleSysVar("TIDB_CONFIG")` 不命中。simple-fold 实现只服务于 ASCII 目标常量：支持 `K`/`k` 和 `ſ`/`s` 的 Go 等价类，但明确不把 dotless i `ı` 当作 `i`。权限函数的“大写”约束由 `intest::Assert` 表达，其实际失败行为取决于 `intest` 构建配置；无论断言是否启用，最终前缀判断仍区分大小写。

## 并发与资源生命周期

`semEnabled` 的读写统一使用 `Ordering::SeqCst`，为并发线程提供单一全序，语义对应 Go `sync/atomic` 的顺序一致操作。判定函数只读取不可变常量和输入，不分配长期资源，也不持锁。

启用/关闭不是一个跨原子状态、系统变量和日志的原子事务：并发观察者可能在 `semEnabled` 已变化而系统变量尚未全部更新时读取到中间状态。代码注释将动态配置描述为安全风险，并表明 `Enable`/`Disable` 主要供测试套件使用；安全扩展不应假设这些多步副作用具备事务性。

本文件不创建线程、异步任务、通道、文件句柄或网络连接。`BgLogger` 和系统变量注册表由依赖 crate 管理生命周期；OS 主机名只在 `Disable` 调用期间读取。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/sem/sem.go`。Rust 保留了 Go 的公开函数集合、常量顺序、四组隐藏规则、原子 `int32` 开关、系统变量副作用、启用日志、主机名恢复条件和 `RESTRICTED_` 的“长度至少 12”约束。

主要语言映射为：Go `atomic.StoreInt32/LoadInt32` 对应 Rust `AtomicI32` 的 `SeqCst` 操作；Go `switch` 名单对应 Rust 数组 `.contains()`；Go `os.Hostname` 的成功分支对应 `hostname::get()`；Go `strings.EqualFold` 由两个私有 helper 局部实现，而不是简单 ASCII 忽略大小写。Rust 的 `SetSysVar(...).expect(...)` 显式处理依赖 API 的 `Result`，Go 调用没有可见返回值。

`pkg/util/sem/sem_test.go` 与 `sem_test.rs` 覆盖相同的基础黑名单和权限前缀语义；Rust 测试额外覆盖 long-s 和 dotless-i，确保 helper 与 Go simple folding 对齐。`pkg/util/sem/migration_aster_unit_test.rs` 进一步覆盖启用/关闭副作用、日志、默认/真实主机名、全部名单、大小写约定以及裸 `RESTRICTED_` 边界。

## 扩展指南

新增 v1 隐藏规则时，应修改最窄的现有符号：表规则更新对应常量及 `IsInvisibleTable` 分支，系统变量规则更新 `IsInvisibleSysVar` 数组，状态变量更新 `IsInvisibleStatusVar`，权限语义更新 `IsRestrictedPrivilege`。同时必须与 `pkg/util/sem/sem.go` 保持行为一致，并同步 `pkg/util/sem/sem_test.rs` 和 `pkg/util/sem/migration_aster_unit_test.rs`；Go 侧行为变化还应同步 `pkg/util/sem/sem_test.go`。

若扩展 schema 比较，不应直接用 Unicode 大小写转换替代 `equal_fold_ascii`，因为完整大小写映射与 Go simple folding 不同。若目标不再是 ASCII 常量，需要先证明 helper 的适用范围或替换为完整且等价的 simple-fold 实现，并增加非 ASCII 回归用例。

若扩展开关生命周期，应注意 `Enable`/`Disable` 的多步更新目前不是事务；增加可并发动态切换可能需要显式序列化和失败恢复。新增系统变量副作用还要确保变量在调用前注册。兼容性方面，应检查 `pkg/util/sem/compat/sem.rs` 的 v1/v2 互斥与转发；性能方面，当前名单均很小，线性查找成本有限，但大幅增长时应评估静态集合结构而不能擅自改变大小写语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含本仓库；`files --filter pkg/util/sem` 确认目标、入口、测试和 v1/v2/compat 文件；`node --file pkg/util/sem/sem.rs --offset 1 --limit 400` 读取全部 276 行；`query` 核对八个公开函数和两个私有 helper；`callers`/`callees` 核对内部调用边。常见符号的跨语言同名结果不够精确，跨 crate 调用改用精确文本搜索复核。
- 源码与边界：`pkg/util/sem/sem.rs`、`pkg/util/sem/lib.rs`、`pkg/util/sem/Cargo.toml`、根 `Cargo.toml`。
- Go 对照：`pkg/util/sem/sem.go`、`pkg/util/sem/sem_test.go`。
- Rust 测试：`pkg/util/sem/sem_test.rs`、`pkg/util/sem/migration_aster_unit_test.rs`、`pkg/util/sem/main_test.rs`；兼容调用证据来自 `pkg/util/sem/compat/sem.rs` 及其 `Cargo.toml`。
- 生产调用点：`pkg/expression/extension.rs`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/planbuilder.rs`、`pkg/privilege/privileges/privileges.rs`、`pkg/privilege/privileges/cache.rs`。
- 本任务为纯文档分析，按计划未运行 Cargo。结构验证要求本文恰好包含上述 11 个固定二级标题。
