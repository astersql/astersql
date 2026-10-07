# `br/pkg/gluetidb/infoschema_filter.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-gluetidb`（清单见 `br/pkg/gluetidb/Cargo.toml`），由 `br/pkg/gluetidb/lib.rs` 以 `pub mod infoschema_filter` 挂载，并通过 `pub use infoschema_filter::*` 扁平再导出。它是 Go 文件 `br/pkg/gluetidb/infoschema_filter.go` 的 Rust 对照实现，在 BR 创建或取得 TiDB Domain 时提供按数据库名裁剪 InfoSchema 加载范围的策略。

该实现目前采用本 crate 内定义的最小 `Filter`、`InfoSchema`、`SchemaDiff` 和 `DBInfo` 抽象，而不是直接实现 `pkg/infoschema/issyncer/filter.rs` 中的同名 trait。`br/pkg/gluetidb/glue.rs` 的 `Glue::InfoSchemaFilter` 和 `DomainHooks::GetOrCreateDomainWithFilter` 使用的是这里的 `Filter`。仓库引用搜索未发现 Rust 生产代码直接调用本文件的 `NewInfoSchemaFilter`；直接调用只出现在 `br/pkg/gluetidb/parity_test.rs`。`br/cmd/br` 当前使用 `br/cmd/br/stubs.rs` 中另一套 `NewInfoSchemaFilter`/`InfoSchemaFilter` 桩，因此不能据 Go 接线推断本 Rust 构造函数已经接入真实 CLI 主链。

## 核心职责

文件把“允许加载某个数据库”的谓词转换为“是否跳过某次加载”的 `Filter`：

- `NewInfoSchemaFilter` 在没有谓词时返回 `None`，让调用方保持默认全量加载；有谓词时封装为私有 `brInfoSchemaFilter`。
- `Filter::SkipLoadSchema` 在全量模式加载中按 `DBInfo.Name` 调用允许谓词，并对结果取反：允许即不跳过，拒绝即跳过。
- `Filter::SkipLoadDiff` 在增量 SchemaDiff 加载中先保护无法安全按库名过滤的 DDL，再用 `SchemaID` 经 `InfoSchema::SchemaByID` 反查库名并应用谓词。
- 建库、放置策略和资源组相关动作永不跳过，因为建库 diff 当下无法取得新库名，而后两类动作的 `SchemaID` 实际承载全局对象 ID，并非可据以过滤的数据库 ID。分支见 `brInfoSchemaFilter::skip_load_diff_inner`。

返回值遵守 issyncer 约定：`true` 表示跳过，`false` 表示继续加载；这一点由本文件 `Filter` 的注释及 Go `issyncer.Filter` 对照语义共同确认。

## 主要符号

- `ActionType`：`#[repr(i32)]` 的动作枚举，仅保留过滤决策所需的七种特殊 DDL 和兜底 `Other = 0`。数值 `1、51、52、53、68、69、70` 与 `pkg/meta/model/job.go` 中相应 Go 常量一致。`Default` 返回 `Other`。
- `SchemaDiff`：公开的最小变更摘要，保存 `Version`、`Type`、`SchemaID`、`TableID`、`OldSchemaID`。当前过滤判断只读取 `Type` 和 `SchemaID`；其余字段为 Go 日志字段的结构对齐数据。
- `DBInfo`：只保留 `Name: CIStr`，供全量过滤和反查结果使用。
- `InfoSchema: Send + Sync`：最小查询接口，唯一方法 `SchemaByID(i64) -> Option<DBInfo>`。`None` 同时表达未找到 schema。
- `Filter: Send + Sync`：公开策略接口，包含 `SkipLoadDiff` 和 `SkipLoadSchema`。两个方法都以 `true` 表示跳过。
- `AllowFn`：私有装箱闭包类型 `Box<dyn Fn(&CIStr) -> bool + Send + Sync>`；`Send + Sync` 使策略对象可跨线程共享，但不承诺闭包内部无同步成本。
- `brInfoSchemaFilter`：私有实现体，只持有一个非空 `allow`。因为只能由构造函数创建，Rust 实现不需要像 Go 接收者那样再次检查 `f == nil` 或 `f.allow == nil`。
- `NewInfoSchemaFilter`：公开构造入口，输入为可选允许谓词，输出为可选的 `Box<dyn Filter>`。
- `skip_load_diff_inner`：私有核心决策函数；公开 trait 方法 `SkipLoadDiff` 只委托给它并返回结果。

## 执行流程

构造流程如下：调用方把库名谓词交给 `NewInfoSchemaFilter`；`None` 立即返回 `None`，`Some(allow)` 则形成 `brInfoSchemaFilter { allow }` 并擦除为 `Box<dyn Filter>`。若调用方把它安装到 `Glue::InfoSchemaFilter`，`Glue::getDomainInner` 会通过 `as_deref()` 将 `Option<&dyn Filter>` 传给 `DomainHooks::GetOrCreateDomainWithFilter`。

`SkipLoadSchema` 的决策顺序是：

1. `dbInfo` 为 `None` 时返回 `false`，保守地不跳过。
2. 有库信息时以 `dbInfo.Name` 调用 `allow`。
3. 对允许结果取反后返回，因此只有被谓词拒绝的库会被跳过。

`SkipLoadDiff` 委托 `skip_load_diff_inner`，后者严格按以下优先级返回：

1. `ActionCreateSchema`、三种 placement policy 动作、三种 resource group 动作直接返回 `false`。
2. 其他动作若 `SchemaID == 0`，视为与数据库无关或无法按库识别，返回 `false`。
3. `SchemaID != 0` 但 `latestIS == None` 时无法反查库名，返回 `true`。
4. 有 `latestIS` 时调用 `SchemaByID`。查到库且 `allow(Name)` 为真才得到 `selected = true`；查不到或谓词拒绝都得到假。
5. 返回 `!selected`：选中库继续加载，未选中或不存在的库跳过。

这个顺序是重要不变量：全局/特殊动作和零 SchemaID 必须在缺少 `latestIS` 的分支之前处理，否则它们会被错误跳过。

## 数据与状态

过滤器自身只有不可变的 `allow` 闭包，没有缓存、计数器或可变 schema 状态。每次调用都使用传入的 `SchemaDiff`、`DBInfo` 或 `InfoSchema` 快照作即时判断。`SchemaByID` 返回拥有所有权的最小 `DBInfo`，随后只借用其中 `Name` 调用谓词。

`CIStr` 来自直接 Cargo 依赖 `astersql-br-pkg-glue`。与 `br/pkg/gluetidb/glue.rs` 的过滤辅助函数配合时，通常由 `CIStr.L` 做大小写折叠后的匹配，并用 `CIStr.O` 识别大小写敏感的 BR 临时库前缀；本文件本身不解释或正规化名称，只把完整 `CIStr` 交给调用者提供的闭包。

`SchemaDiff` 中的 `Version`、`TableID` 和 `OldSchemaID` 当前不影响 Rust 决策，也未被输出；它们不应被误认为已有日志或审计副作用。

## 依赖与调用关系

直接编译依赖只有 `astersql_br_pkg_glue::CIStr`；`br/pkg/gluetidb/Cargo.toml` 还声明了 `astersql-br-pkg-gluetikv` 和 `astersql-errors`，但那两项由同 crate 的 `glue.rs` 使用，不是本文件的直接依赖。

RustCodeGraph 对目标文件识别出 15 个符号，并确认关键内部边为 `Filter::SkipLoadDiff` 实现调用 `brInfoSchemaFilter::skip_load_diff_inner`，后者调用 `InfoSchema::SchemaByID`；`Filter::SkipLoadSchema` 实现直接调用保存的允许闭包。模块入口和扁平再导出来自 `br/pkg/gluetidb/lib.rs`。

当前可核实的上游分为两类：

- crate 内运行边：`Glue::getDomainInner` 把 `Glue::InfoSchemaFilter.as_deref()` 交给 `DomainHooks::GetOrCreateDomainWithFilter`，说明安装后的策略会参与 Domain 获取/创建。
- 测试边：`br/pkg/gluetidb/parity_test.rs` 直接构造过滤器并调用两个 trait 方法。

仓库搜索未证明 Rust CLI 会用本文件的 `NewInfoSchemaFilter` 设置 `Glue::InfoSchemaFilter`。Go 上游 `br/cmd/br/cmd.go` 明确调用 Go 版本构造函数；Rust 对应 `br/cmd/br/cmd.rs` 则操作 `br/cmd/br/stubs.rs` 的独立桩。扩展或接线时必须先解决这两个边界类型的归属，不能把同名函数当成同一个符号。

## 错误处理与边界

本文件没有 `Result`、panic 或显式错误类型，所有不可判定情形都编码为布尔决策：

- 没有允许谓词：不创建过滤器，而非创建一个始终允许的对象。
- 全量加载缺少 `DBInfo`：返回“不跳过”，选择保守加载。
- 增量加载有非零 `SchemaID` 但没有 `latestIS`：返回“跳过”，与 Go 行为一致。
- `SchemaByID` 找不到库：当作未选中并跳过。
- 零 `SchemaID`：不查询 `InfoSchema`，直接继续加载。

Rust 类型系统排除了 Go 中的两个 nil 边界：已构造的 `brInfoSchemaFilter` 不可能有空 `allow`，trait 方法的 `&self` 也不可能是 nil 接收者。相应兼容行为被前移到 `NewInfoSchemaFilter(None) -> None`。

Go `SkipLoadDiff` 在最终决定跳过时记录动作类型、schema/table/old-schema ID 和版本；Rust `SkipLoadDiff` 只保留了计算位置，没有日志输出。因此过滤结果对齐，但可观测性并不完全等价。传入谓词或 `InfoSchema::SchemaByID` 若由实现自行 panic，本文件不会捕获。

## 并发与资源生命周期

`InfoSchema`、`Filter` 和 `AllowFn` 都要求 `Send + Sync`，允许过滤器对象被 `Arc<dyn Filter>` 持有并从多个执行线程借用。方法只接收 `&self`，本实现不做内部可变操作，也不持有锁、任务、通道、事务、文件或网络资源。

构造函数返回的 `Box<dyn Filter>` 拥有闭包；上层若要共享，可像 `Glue::InfoSchemaFilter` 一样转换并存入 `Arc<dyn Filter>`。对象销毁时由 Rust 自动释放闭包。`latestIS` 和 `DBInfo` 都只是调用期间的借用，不会被保存，因此本文件不延长 Domain 或 InfoSchema 生命周期。

并发安全最终还依赖调用者闭包和 `InfoSchema` 实现兑现各自的 `Send + Sync` 契约。若闭包内部使用互斥量或执行昂贵匹配，调用延迟与锁竞争会直接落在 schema 加载路径上。

## 与 Go 版本的对应关系

主要语义与 `br/pkg/gluetidb/infoschema_filter.go` 对齐：空谓词不安装过滤器；特殊动作、零 SchemaID、缺少 latest InfoSchema、反查失败以及 allow 取反的分支顺序一致；`SkipLoadSchema(nil)` 也都不跳过。`ActionType` 判别值已用 `pkg/meta/model/job.go` 的常量核对。

已确认的差异包括：

- Go 直接实现 `issyncer.Filter` 并使用完整的 `model.SchemaDiff`、`model.DBInfo` 和 `infoschema.InfoSchema`；Rust 使用 gluetidb crate 私有边界上的最小替身类型，与 `pkg/infoschema/issyncer/filter.rs` 的 Rust trait 不是同一接口。
- Go 允许 nil 接收者或内部 nil `allow` 并返回不跳过；Rust 的已构造对象不存在这些状态，只有构造输入 `None`。
- Go 对每个被跳过的 diff 写 info 日志；Rust 明确保持静默，所以 `Version`、`TableID`、`OldSchemaID` 尚无运行时用途。
- Go 的 CLI 接线在 `br/cmd/br/cmd.go` 可见；当前 Rust CLI 使用另一套本地桩，尚未验证到本文件构造函数的生产调用。

相关 Rust 独立测试为 `br/pkg/gluetidb/parity_test.rs`，覆盖空谓词、建库动作、零 SchemaID、缺失 InfoSchema、允许库反查、空 DBInfo 和拒绝库。它没有逐项覆盖六种 placement/resource-group 动作、`SchemaByID` 未命中、已允许库的 `SkipLoadSchema`，也没有验证跳过日志（当前实现本就没有日志）。Go 同路径没有独立 `infoschema_filter_test.go`；`pkg/infoschema/issyncer/loader_test.go` 定义了模拟 BR 语义的 `testNameFilter`，属于 loader 层的间接对照而非本类型的直接测试。

## 扩展指南

新增或修改过滤规则时，应首先改 `brInfoSchemaFilter::skip_load_diff_inner`，保持“特殊动作 → 零 SchemaID → latestIS 可用性 → 名称谓词”的优先级，并同步核对 `br/pkg/gluetidb/infoschema_filter.go` 与 `pkg/meta/model/job.go`。若新增特殊 `ActionType`，必须核对 Go 的真实数值及 `SchemaID` 语义，避免把全局对象 ID 当数据库 ID；同时扩展 `br/pkg/gluetidb/parity_test.rs` 的独立测试，不要把测试写入生产 `.rs` 文件。

若要补齐生产接线，最可能涉及 `NewInfoSchemaFilter`、`Glue::InfoSchemaFilter`、`DomainHooks::GetOrCreateDomainWithFilter` 以及 `br/cmd/br` 的桩/真实边界。应先决定复用本 crate trait，还是适配 canonical `pkg/infoschema/issyncer/filter.rs`；直接复制第三套类型会继续扩大接口分裂。此类修改需要验证 Domain loader 确实消费过滤器，而不能只以构造成功或 trait 编译成功作为完成证据。

若补齐 Go 的日志行为，应在 `SkipLoadDiff` 的最终 `skip == true` 路径记录 `Type`、`SchemaID`、`TableID`、`OldSchemaID`、`Version`，并评估高频 schema 同步时的日志量。若更改 `AllowFn` 的并发约束或引入缓存，还需评估共享方式、锁顺序、失效时机和 Domain 生命周期。

建议补充的边界测试包括：六个全局对象动作分别不跳过；`SchemaByID` 返回 `None` 时跳过；允许库与拒绝库的 `SkipLoadSchema` 对称断言；谓词调用次数与特殊分支不调用谓词；通过 `Glue::getDomainInner` 将安装的过滤器传给 hook 的接线测试。

## 验证依据

- 源码与模块：`br/pkg/gluetidb/infoschema_filter.rs`、`br/pkg/gluetidb/lib.rs`、`br/pkg/gluetidb/glue.rs`。
- crate 边界：`br/pkg/gluetidb/Cargo.toml`；确认包名、`lib.rs` 入口及 `astersql-br-pkg-glue` 路径依赖。
- Go 对照：`br/pkg/gluetidb/infoschema_filter.go`、`br/cmd/br/cmd.go`、`pkg/meta/model/job.go`。
- Rust 与 Go 测试证据：`br/pkg/gluetidb/parity_test.rs`、`pkg/infoschema/issyncer/loader_test.go`；同目录未发现独立的 `infoschema_filter_test.go` 或 `infoschema_filter_test.rs`。
- RustCodeGraph：`status` 报告索引包含 7032 个 Rust 文件；`files --filter br/pkg/gluetidb` 收录目标实现、Go 对照和 `parity_test.rs`；`explore "br/pkg/gluetidb/infoschema_filter.rs NewInfoSchemaFilter SkipLoadDiff SkipLoadSchema"` 与目标文件 `node` 查询确认 15 个符号及 `SkipLoadDiff -> skip_load_diff_inner -> SchemaByID` 调用链。图对 Go/Rust 同名符号会合并候选，因此上游接线另以精确路径 `rg` 复核。
- 引用搜索：`rg -n "NewInfoSchemaFilter\\(|InfoSchemaFilter\\s*[:=]" --glob '*.rs' --glob '*.go' br`，确认本 Rust 构造函数的直接调用仅在 `br/pkg/gluetidb/parity_test.rs`，并识别 `br/cmd/br/stubs.rs` 的同名独立桩。
- 人工复核结论：文件存在的原因是把 BR 的库名 allow 规则转换为 issyncer 风格的 skip 规则；运行核心是上述有序短路决策；安全扩展必须同步动作语义、独立测试和实际 Domain/CLI 接线。
