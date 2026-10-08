# `pkg/util/filter/schema.rs`

## 文件定位

本文件属于 `astersql-util-filter` crate。crate 入口 `pkg/util/filter/lib.rs` 以 `pub mod schema` 装配该模块，并通过 `pub use schema::*` 把其中的公开项提升到 crate 根，因此外部代码可以直接从 `util_filter`/`astersql-util-filter` 使用这些符号。`pkg/util/filter/Cargo.toml` 表明本文件唯一直接依赖的业务 crate 是以 `metadef` 为本地别名引入的 `astersql-meta-metadef`；`regex`、`selector` 和 `tfilter` 是同 crate 中 `filter.rs` 的依赖，不参与这里的判定。

它位于复制过滤工具包的 schema 分类边界：不负责解释 Do/Ignore 过滤规则，也不维护过滤缓存，只回答一个已经规范化为小写的 schema 名是否属于 TiDB/MySQL、DM 或巡检使用的系统范围。当前 Rust 仓库中的生产代码没有直接调用 `IsSystemSchema`；该接口由 crate 根公开，并由独立测试验证，属于已移植但尚未在 Rust 生产主链发现直接接线的公共辅助 API。

## 核心职责

文件的职责只有两项：

1. 定义过滤层额外认识的两个特殊 schema 名：`DMHeartbeatSchema`（`dm_heartbeat`）和 `InspectionSchemaName`（`inspection_schema`）。
2. 由 `IsSystemSchema` 合并本地特殊名称与元数据层的通用分类：前两项直接比较，其他名称委托 `metadef::IsMemOrSysDB`。

因此完整的真值集合不只写在本文件中。由 `pkg/meta/metadef/db.rs` 可知，下游还覆盖三个内存 schema（`information_schema`、`performance_schema`、`metrics_schema`）以及系统相关库（`mysql`、`sys`、`workload_schema`）。本文件不会把 BR 临时库纳入结果，因为它调用的是 `IsMemOrSysDB`，而不是 `IsBRRelatedDB`。

## 主要符号

- `pub const DMHeartbeatSchema: &str`：值固定为 `"dm_heartbeat"`。它把 DM 心跳库扩展到通用内存/系统库集合之外，并可由 crate 使用者单独引用。
- `pub const InspectionSchemaName: &str`：值固定为 `"inspection_schema"`，表示巡检功能使用的 schema。
- `pub fn IsSystemSchema(schema: &str) -> bool`：文件唯一的执行入口。参数是借用字符串，不取得所有权；返回值只表示分类结果。函数先以 `debug_assert_eq!(schema, schema.to_lowercase())` 检查“小写输入”前置条件，再依次比较两个本地常量，最后调用 `metadef::IsMemOrSysDB(schema)`。

文件没有结构体、枚举、trait、`impl`、泛型、异步函数或条件编译项。命名保留 Go 导出符号风格；`pkg/util/filter/lib.rs` 在 crate 级允许 `non_snake_case` 和 `non_upper_case_globals`。

## 执行流程

调用 `IsSystemSchema(schema)` 时流程如下：

1. 调试构建计算 `schema.to_lowercase()`，并断言结果与原字符串相等；这用于尽早发现调用方没有执行大小写规范化。
2. 若输入等于 `DMHeartbeatSchema`，短路返回 `true`。
3. 否则若输入等于 `InspectionSchemaName`，短路返回 `true`。
4. 否则把同一字符串借用传给 `metadef::IsMemOrSysDB`。后者先执行 `IsMemDB`，再执行 `IsSystemRelatedDB`，最终把内存 schema 和 `mysql`/`sys`/`workload_schema` 分类合并为布尔结果。

`pkg/util/filter/schema_test.rs::TestIsSystemSchema` 体现正常入口用法：测试先用 `to_ascii_lowercase` 模拟大小写不敏感标识符的规范化，再调用函数。`pkg/util/filter/migration_aster_unit_test.rs::migration_nil_rules_cache_and_system_schemas_match_go` 则直接以已经小写的名称覆盖 DM、巡检和常见系统库。

## 数据与状态

两个名称使用编译期 `&'static str` 常量，不发生延迟初始化或运行时修改。函数自身是纯分类计算：它不读取环境、配置或存储，不分配持久状态，也不缓存结果。调试断言中的 `to_lowercase()` 会创建临时 `String`；实际分类只做字符串相等比较并调用无状态的 `metadef` 判定。

关键不变量是输入已经小写，而不是函数内部自动折叠大小写。测试中大写样例之所以能够命中，是因为 `schema_test.rs` 在调用前转换为小写；这不能解释为 `IsSystemSchema("MYSQL")` 本身支持大小写不敏感匹配。发布构建通常不执行 `debug_assert_eq!`，所以违反前置条件时不会在函数内纠正输入，只可能得到 `false`。

## 依赖与调用关系

- 模块装配与公开入口：`pkg/util/filter/lib.rs` 声明 `pub mod schema` 并 `pub use schema::*`，使三个公开符号成为 crate 公共 API。
- 下游调用：`IsSystemSchema` 调用 `pkg/meta/metadef/db.rs::IsMemOrSysDB`；后者继续调用 `IsMemDB` 和 `IsSystemRelatedDB`。这是本文件唯一的函数调用边。
- 直接 Rust 引用：仓库搜索只发现 `pkg/util/filter/schema_test.rs` 和 `pkg/util/filter/migration_aster_unit_test.rs` 调用 `IsSystemSchema`；两个常量在当前 Rust 源码中除定义和函数内部比较外没有独立消费者。
- 过滤器关系：`pkg/util/filter/filter.rs` 与本文件同属一个 crate，但其 `Filter` 规则匹配路径没有调用 `IsSystemSchema`。两者通过 crate 公共表面并列提供“规则过滤”和“系统 schema 分类”能力，而不是彼此嵌套执行。

RustCodeGraph 的文件节点确认 `schema.rs` 有两个索引符号，并识别到测试侧使用；精确 `callers`/`callees` 命令在本次环境中超时且没有输出，因此上述直接引用和唯一调用边又以 `rg` 和源码读取复核，不把宽泛的同名 `schema` 搜索结果当作调用证据。

## 错误处理与边界

该 API 返回 `bool`，没有 `Result`、错误码或日志路径。唯一显式失败机制是调试断言：传入包含大写字符且其 Unicode 小写结果不同的字符串时，启用调试断言的构建会 panic；发布构建则继续比较。空字符串、普通用户库名和未知名称自然返回 `false`。

边界还包括：

- 比较是精确字符串比较，不做空白裁剪、标识符反引号剥离、Unicode 规范化或前后缀匹配。
- `dm_heartbeat` 与 `inspection_schema` 只接受这里声明的小写拼写。
- `metadef::IsMemOrSysDB` 的集合决定通用系统库范围；修改其分类会间接改变本函数行为。
- 本函数不判断表名，也不应用 `Filter` 的 Do/Ignore 规则。
- `debug_assert_eq!` 是开发期契约检查，不应作为面向不可信输入的运行时校验。

## 并发与资源生命周期

文件没有可变全局状态、锁、原子变量、线程、任务、通道、事务、文件句柄或网络资源。常量具有整个进程生命周期；`&str` 参数只在调用期间借用。分类过程只读，因此多个线程可以并发调用而无需同步。

资源成本为常数级字符串比较；调试构建还承担一次小写转换及临时字符串分配。新增高频调用点时应由调用方在既有名称规范化阶段统一转小写，避免为了每次分类重复分配。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/filter/schema.go`。Rust 保留了 Go 的两个导出名称、相同字符串值、相同的短路顺序，以及对 `metadef.IsMemOrSysDB` 的委托。Go 的包级 `var` 在 Rust 中收紧为不可变 `const &str`；当前仓库没有修改这些变量的用法，因此分类语义一致，同时 Rust API 不允许运行时替换名称。

小写契约也对应：Go 用 `intest.AssertFunc` 检查 `schema == strings.ToLower(schema)`，Rust 用 `debug_assert_eq!` 表达开发期断言。两者都要求调用方先规范化名称，而不是在分类函数里静默转换。`pkg/util/filter/schema_test.go::TestIsSystemSchema` 通过 `ast.NewCIStr(tt.name).L` 取得小写形式；Rust 对应测试用 `to_ascii_lowercase()` 后再调用。当前测试样例均为 ASCII，因此两条测试路径在覆盖范围内一致；它们没有验证非 ASCII schema 的大小写规范化差异。

Go 和 Rust 测试都覆盖 information/performance/mysql/sys、普通库、metrics 与 inspection；Rust 的迁移测试额外直接覆盖 `dm_heartbeat`。通用集合的权威实现分别位于 `pkg/meta/metadef/db.go` 与 `pkg/meta/metadef/db.rs`。

## 扩展指南

若要增加过滤层特有的系统 schema，应修改 `schema.rs` 中的常量/`IsSystemSchema` 分支，并同步独立测试 `pkg/util/filter/schema_test.rs`；若要求 Go/Rust 对齐，还应核对 `schema.go` 与 `schema_test.go`。不要把测试嵌入生产源文件。

若新增名称本质上属于全局内存库或系统相关库，应优先评估修改 `pkg/meta/metadef/db.rs` 的分类函数，而不是只在过滤层打补丁，因为其他子系统也可能依赖全局分类。反之，DM 心跳或巡检这类过滤域特有名称应继续留在本层，避免扩大 `metadef` 的语义。

扩展时需要特别检查三类风险：调用方是否统一传入小写名称；新分类是否会意外排除用户同名库；Go/Rust 的集合和断言语义是否仍一致。若改变公开常量类型、名称或 crate 根再导出，还要检查所有外部 crate 消费者。性能方面应保持无状态、短路、常数级比较；若集合明显扩大，可再基于基准和真实调用频率评估静态集合，而不应先引入锁或运行时缓存。

## 验证依据

- Rust 源码与模块：`pkg/util/filter/schema.rs`、`pkg/util/filter/lib.rs`。
- crate 边界：`pkg/util/filter/Cargo.toml`，确认包名、`lib.rs` 入口以及 `metadef` 本地依赖。
- Go 对照：`pkg/util/filter/schema.go`、`pkg/meta/metadef/db.go`。
- 下游实现：`pkg/meta/metadef/db.rs::{IsMemOrSysDB, IsMemDB, IsSystemRelatedDB, IsSystemDB}`。
- 独立测试：`pkg/util/filter/schema_test.rs::TestIsSystemSchema`、`pkg/util/filter/migration_aster_unit_test.rs::migration_nil_rules_cache_and_system_schemas_match_go`、`pkg/util/filter/schema_test.go::TestIsSystemSchema`。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件；`files --filter pkg/util/filter` 找到本 crate 的 Rust/Go 源与测试；`query IsSystemSchema --kind function --json` 精确定位 Rust、Go 及两侧测试符号；`node --file pkg/util/filter/schema.rs --offset 1 --limit 400` 读取完整 43 行并确认公开常量和函数。精确调用边命令超时无输出，已用仓库文本引用和源码短路表达式交叉核对。
- 人工事实复核：本文件存在是为了补充过滤域特殊库名并复用元数据层系统库分类；执行路径、输入小写不变量、无状态边界和安全扩展位置均可由上述符号反向验证。
- 本任务是纯文档分析，依照任务要求未运行 Cargo；交付结构使用任务规定的 11 个固定二级标题验证。
