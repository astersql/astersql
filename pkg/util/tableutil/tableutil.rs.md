# `pkg/util/tableutil/tableutil.rs`

## 文件定位

本文件是 `astersql-util-tableutil` crate 的核心实现，定义临时表的会话/事务态抽象以及一个可替换的构造工厂。crate 入口 `pkg/util/tableutil/lib.rs` 通过 `mod tableutil` 加载本文件并用 `pub use tableutil::*` 对外再导出；`pkg/util/tableutil/Cargo.toml` 将 crate 根指定为 `lib.rs`，只直接依赖 `astersql-meta-autoid` 和 `astersql-meta-model-group4`。

它不实现具体临时表，也不保存某个会话的临时表集合。具体状态由 `TempTable` 的实现者持有；本文件只规定跨模块契约，并保存进程级构造函数槽位。RustCodeGraph 将本文件列为被 `pkg/table/tblctx/table.rs` 和 `pkg/util/tableutil/migration_aster_unit_test.rs` 使用。

## 核心职责

1. `TempTable` trait 规定临时表副本必须提供独立的 autoID 分配器、修改标记、统计对象、大小和 `TableInfo` 元数据。其目的与同路径 Go 接口一致：全局/局部临时表的会话态不能直接共享普通表的相应状态。
2. `TempTableFactory` 把“由 `TableInfo` 构造临时表”的行为类型化，并要求工厂闭包、返回对象及接口均满足跨线程边界所需的 `Send + Sync`。
3. `TEMP_TABLE_FROM_META`、`SetTempTableFromMeta` 和 `TempTableFromMeta` 提供线程安全、可替换的包级工厂，模拟 Go 中可赋值的函数变量。
4. 本文件刻意用 `Any` 隔离统计模块：`GetStats` 不依赖具体 statistics crate，从而避免循环依赖；调用者须自行向下转型。

## 主要符号

- `pub trait TempTable: Send + Sync`：临时表对象的公开动态分派边界。
  - `GetAutoIDAllocator(&self) -> Arc<dyn autoid::Allocator>` 返回该临时表副本的共享分配器。
  - `SetModified(&mut self, bool)` / `GetModified(&self) -> bool` 写入和读取当前会话/事务中的修改标记。
  - `GetStats(&self) -> Arc<dyn Any + Send + Sync>` 返回类型擦除、可共享的统计对象。
  - `GetSize` / `SetSize` 维护调用者定义的会话视角大小；接口本身不校验正负值或上限。
  - `GetMeta(&self) -> &model::TableInfo` 借用表元数据，生命周期受临时表对象约束。
- `pub type TempTableFactory = Arc<dyn Fn(Arc<model::TableInfo>) -> Box<dyn TempTable> + Send + Sync>`：公开工厂类型。输入元数据可共享，输出由调用者独占持有的 trait object。
- `static TEMP_TABLE_FROM_META: LazyLock<RwLock<Option<TempTableFactory>>>`：首次访问时初始化为空的私有全局槽位；`Option::None` 表示尚未注册。
- `SetTempTableFromMeta(factory)`：取得写锁，以 `std::mem::replace` 安装新工厂并返回旧工厂，支持测试或嵌入方恢复原状态。
- `TempTableFromMeta(tblInfo)`：取得读锁、克隆 `Arc` 工厂、释放锁后调用；空槽位时按设计 panic。

文件还声明 `#![allow(dead_code)]` 和 `#![allow(non_snake_case)]`，后者保留 Go 风格公开名称，减少移植调用面差异。

## 执行流程

工厂路径按以下顺序运行：

1. 集成模块先构造一个 `TempTableFactory`，调用 `SetTempTableFromMeta(Some(factory))`。
2. setter 对 `TEMP_TABLE_FROM_META` 取写锁；锁中只替换 `Option`，旧 `Arc` 原样返回，调用方可以稍后恢复。
3. 需要实例时，调用 `TempTableFromMeta(Arc<TableInfo>)`。
4. getter 对槽位取读锁，将已注册工厂的 `Arc` 克隆到局部变量。临时读锁不会覆盖实际构造过程，因此工厂内部再次访问注册槽位不会因本次读锁而被长期阻塞。
5. 工厂接收共享元数据并返回 `Box<dyn TempTable>`；后续状态读写通过 trait 方法动态分派。

已接入的 Rust 消费链位于 `pkg/table/tblctx/table.rs`：该文件为所有 `T: tableutil::TempTable` 提供本地 `TemporaryTable` 的 blanket impl，转发 `GetMeta`、`GetSize`、`SetSize`，再由 `TemporaryTableHandler` 读取元数据/脏大小并累加事务 delta。RustCodeGraph 对 `NewTemporaryTableHandler` 的调用者只找到该模块的独立测试。对 `SetTempTableFromMeta` / Rust `TempTableFromMeta` 的调用方查询未返回生产调用边，仓库文本搜索也只发现 `migration_aster_unit_test.rs` 直接调用它们；因此当前证据不能声称 Rust 工厂已接入完整会话主链。

## 数据与状态

本文件自身唯一的可变状态是进程级 `TEMP_TABLE_FROM_META`。它保存的是工厂而非临时表实例，所以不同会话状态是否真正隔离，取决于工厂每次是否创建独立对象以及具体 `TempTable` 实现如何保存字段。

所有权设计如下：元数据以 `Arc<TableInfo>` 进入工厂；autoID 分配器和统计对象也以 `Arc` 返回，允许多处共享同一对象；临时表主体以 `Box<dyn TempTable>` 返回，修改标记和大小的 setter 需要 `&mut self`。`GetMeta` 只返回借用，不能脱离表对象长期保存。接口没有规定 `size` 的单位、是否可为负数、统计对象的具体类型，也没有在本层维护事务提交/回滚状态。

`pkg/util/tableutil/migration_aster_unit_test.rs` 的 `TestTempTable` 以字段方式实现上述契约，验证 modified 从 `false` 变为 `true`、size 从 `0` 变为 `68`、元数据 ID 保持为 `587`、分配器返回预期 next ID，并将 `Arc<dyn Any>` 下转型为 `String`。

## 依赖与调用关系

- 上游装配：`pkg/util/tableutil/lib.rs` 再导出本文件全部公开符号，并把 `autoid`、`model` 依赖一并再导出。
- 直接依赖：`std::any::Any` 用于统计类型擦除；`Arc` 管理共享工厂/元数据/分配器/统计对象；`LazyLock` 延迟建立全局槽位；`RwLock` 协调注册与读取；`crate::{autoid, model}` 提供分配器 trait 和 `TableInfo`。
- Rust 下游：`pkg/table/tblctx/table.rs` 的 blanket impl 消费 `TempTable` 的元数据和大小子集。其 Cargo manifest 以 `tableutil-dependency` 引用本 crate。
- Cargo 反向声明：除 tblctx 外，仓库搜索还找到 `pkg/sessiontxn/isolation/Cargo.toml`、`pkg/executor/Cargo.toml` 和根 workspace facade 对本 crate 的依赖；但依赖声明本身不能证明本文件全部 API 已被运行时调用。
- Go 主链对照：`pkg/table/tables/tables.go` 的 `init` 将 Go `tableutil.TempTableFromMeta` 赋为具体构造器；`pkg/sessionctx/variable/session.go::GetTemporaryTable` 在事务上下文 map 中按表 ID 懒创建并缓存；`pkg/table/tblsession/table.go` 读取 autoID、设置 modified，并通过 `pkg/table/tblctx/table.go` 更新大小。

RustCodeGraph 的精确查询确认本文件包含 11 个已索引符号；对 setter/getter 的 callee 查询未发现图内下游调用（锁、`expect`、`replace` 和闭包调用没有被索引成可展示调用边），所以这些内部步骤以本文件源码为依据。

## 错误处理与边界

本 API 没有 `Result` 返回值，失败采用 panic：

- `RwLock::write/read` 若因持锁线程 panic 而 poisoned，分别以 `temporary table factory lock poisoned` panic。
- `TempTableFromMeta` 在槽位为 `None` 时以 `TempTableFromMeta is not initialized` panic；这有意对齐 Go 调用 nil 函数变量的失败方式。
- 注册的工厂本身若 panic，异常继续向上传播；本层不捕获，也不恢复旧工厂。

trait 不验证实现者的业务不变量：允许任意 `i64` size、任意 `Any` 统计类型，也不能保证 `GetMeta` 与分配器确实属于同一张表。调用 `GetStats` 的代码在下转型失败时必须自行处理。由于全局槽位可被任意持有 API 的代码替换，生产注册应有明确的初始化顺序；并发测试若共同改写该槽位，也必须自行串行化或可靠恢复，避免相互污染。

## 并发与资源生命周期

`TempTable: Send + Sync`、`TempTableFactory: Send + Sync` 以及统计对象的 `Send + Sync` 约束允许这些对象跨线程传递或共享。`TEMP_TABLE_FROM_META` 的 `RwLock` 保证工厂替换与读取不存在数据竞争；读取路径先克隆 `Arc` 再调用工厂，使锁的临界区保持短小。已取得的旧工厂或局部克隆不会因另一线程替换全局槽位而失效：`Arc` 会把其生命周期延长到最后一个引用释放。

这些类型约束不等于临时表字段会被内部锁保护。可变 trait 方法要求调用者拿到 `&mut dyn TempTable`；共享分配器和统计对象的内部同步由各自实现负责。该文件不创建线程、异步任务、通道或事务，也不实现提交/回滚清理。全局工厂存活至进程结束，实例则随返回的 `Box` 及其内部 `Arc` 引用计数释放。

## 与 Go 版本的对应关系

`pkg/util/tableutil/tableutil.go` 是直接语义来源。方法集合一一对应；主要表示差异是：

- Go `autoid.Allocator` 对应 Rust `Arc<dyn autoid::Allocator>`，显式表达共享所有权。
- Go `any` 对应 `Arc<dyn Any + Send + Sync>`；Rust 增加线程安全和生命周期约束，但仍避免 statistics 循环依赖。
- Go `*model.TableInfo` 对应工厂输入 `Arc<TableInfo>`，而 `GetMeta` 返回 `&TableInfo`。
- Go 的包级 `var TempTableFromMeta func(...)` 可直接赋值；Rust 用私有 `LazyLock<RwLock<Option<...>>>` 加公开 setter/getter 模拟，并额外提供旧值恢复能力。
- Go 具体实现 `pkg/table/tables/tables.go::TemporaryTable` 初始化 pseudo stats 和临时表 autoID allocator，并由 `init` 自动注册。当前 Rust 搜索只发现测试实现 `TestTempTable`，未发现本 crate 外的具体 `TempTable` impl 或与 Go `tables.init` 等价的生产工厂注册。因此接口与失败语义已有回归覆盖，但完整构造接线仍属于未验证的迁移状态。

Go 行为测试证据包括 `pkg/table/tblsession/table_test.go`：加入全局临时表后 modified 为真，committed size 来自会话数据，dirty size 可按正负 delta 更新。Rust 的直接接口测试在 `pkg/util/tableutil/migration_aster_unit_test.rs`；Rust tblctx 的大小转发另由 `pkg/table/tblctx/table_test.rs` 和 `migration_aster_unit_test.rs` 覆盖。

## 扩展指南

- 新增会话态字段时，应先在 `TempTable` 增加最小必要方法，再同步具体实现和独立测试；不要把测试放入本生产文件。至少同步 `pkg/util/tableutil/migration_aster_unit_test.rs`，并核对 Go `pkg/util/tableutil/tableutil.go`、`pkg/table/tables/tables.go` 及使用该字段的 tblctx/tblsession 路径。
- 若统计对象开始需要稳定的 Rust 类型，应先评估 crate 依赖环；直接把 statistics 类型引入本 crate 可能破坏当前以 `Any` 隔离循环依赖的设计。保留类型擦除时，调用方应集中封装下转型失败行为。
- 若改变工厂注册 API，必须保留并发替换、未初始化失败语义和旧值恢复测试；不要在持有 `RwLock` 时执行用户工厂。
- 若要完成生产接线，最可能的接入点是具体临时表实现的初始化/装配层，而不是在本接口 crate 中引入 tables/session 依赖。需增加独立集成测试，证明注册发生在首次 `TempTableFromMeta` 调用前，并证明同表不同会话不共享 modified、size、stats 或 autoID 状态。
- 性能风险主要来自热路径中的动态分派、`Arc` 克隆和全局读锁；兼容风险包括修改 Go 风格方法名、统计对象具体类型、panic 文本或 size 语义。优化前应先用真实调用链和基准确认瓶颈，不能以移除会话隔离换取简化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且 `pkg/util/tableutil/tableutil.rs` 已索引；`files --filter pkg/util/tableutil` 找到 `lib.rs`、本文件、Go 对照和迁移测试；`node --file ...` 展示本文件 1–93 行及两个直接使用文件；`query tableutil` 找到 trait、七个方法、setter/getter 与 Go 对照符号；`callers/callees` 查询未找到 Rust 工厂的生产调用边；`node/callers NewTemporaryTableHandler` 找到 tblctx 转发及其两个 Rust 测试调用者。
- 已读 Rust/Cargo：`pkg/util/tableutil/tableutil.rs`、`pkg/util/tableutil/lib.rs`、`pkg/util/tableutil/Cargo.toml`、`pkg/util/tableutil/migration_aster_unit_test.rs`、`pkg/table/tblctx/table.rs`、`pkg/table/tblctx/Cargo.toml`，并用仓库搜索核对 crate 依赖和所有相关 Rust 符号引用。
- 已读 Go：`pkg/util/tableutil/tableutil.go`、`pkg/table/tables/tables.go` 的注册与具体实现、`pkg/sessionctx/variable/session.go::GetTemporaryTable`、`pkg/table/tblctx/table.go`、`pkg/table/tblsession/table.go` 及 `pkg/table/tblsession/table_test.go` 的临时表断言。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令；其结果应为退出码 0，并人工复核本文明确区分源码事实、Go 对照和尚未发现的 Rust 生产接线。
