# `pkg/server/pg_oid.rs`

## 文件定位

本文件是 `astersql-server` crate 内部的 PostgreSQL 对象标识适配层。`pkg/server/lib.rs` 以私有模块 `mod pg_oid` 挂载它，因此这里的常量和函数即使声明为 `pub(crate)`，也只服务于 server crate 内的 PostgreSQL 协议、系统目录和查询适配代码，不构成跨 crate API。crate 边界由 `pkg/server/Cargo.toml` 的 `[package] name = "astersql-server"` 与 `[lib] path = "lib.rs"` 确认。

它解决两个相连的问题：一是把 AsterSQL 持久化的 schema、table、table-local index ID 稳定映射到 PostgreSQL `u32` OID；二是在 PostgreSQL `regclass` 的文本名称与这些 OID 之间转换。主要消费方是 `pkg/server/pg_catalog.rs`，后者用这些 OID 生成 `pg_namespace`、`pg_class`、`pg_index`、`pg_depend` 等目录行，并在 `oid`/`regclass` 强制转换时调用名称解析和反向显示。

## 核心职责

1. `namespace_oid`、`table_oid`、`index_oid` 将原生有符号 ID 映射到互不重叠的 OID 空间，而且不依赖进程内分配器，所以持久 ID 不变时，连接重建和进程重启不会改变结果。
2. `SYSTEM_RELATIONS` 保存 PostgreSQL 系统目录关系的固定 OID，既供 `regclass` 解析和显示，也被 `pkg/server/pg_catalog_query.rs::is_catalog_relation` 用于判定查询是否属于目录适配器。
3. `resolve`/`resolve_with_path` 解析数字 OID或一至两段关系名，按 PostgreSQL 未加引号折叠、加引号保留大小写的规则，在系统关系、当前数据库中的表和索引之间查找。
4. `display` 把已知 OID 还原为系统关系名或当前数据库内的表/索引名；无法识别时保留数值文本，避免伪造不存在的名称。
5. `OID_TYPE` 与 `REGCLASS_TYPE` 把 `pkg/server/pg_result.rs::CatalogColumnType::{Oid, Regclass}` 暴露为目录表达式使用的内部类型码。

## 主要符号

- `OID_TYPE: u8`、`REGCLASS_TYPE: u8`：目录列和强制转换的内部类型标记。`pkg/server/pg_catalog.rs` 用它们选择 OID 数组类型、识别 cast 结果以及构造列元数据。
- `NATIVE_BASE = 16384`、`TABLE_BASE = 0x4000_0000`、`INDEX_BASE = 0x8000_0000`：分别界定系统固定 OID 之后的 schema 区间、table 区间和 index 区间。上界采用排他语义。
- `SYSTEM_RELATIONS: &[(&str, u32)]`：系统目录名称到 PostgreSQL 固定 OID 的表，例如 `pg_class -> 1259`、`pg_namespace -> 2615`。这些值低于 `NATIVE_BASE`。
- `range_error() -> ConnError`：统一构造 `ConnError::Session("PG object ID exceeds the supported OID range")`。
- `signed_id(id: i64) -> ConnResult<u64>`：以正数 `2*id`、负数 `2*abs(id)-1` 的奇偶编码将非零有符号 ID 单射到正整数；零被明确拒绝。计算先提升到 `u128`，再做受检转换。
- `bounded(value, base, end) -> ConnResult<u32>`：把相对值平移到指定 OID 区间，并拒绝 `base + value >= end` 或无法转为 `u32` 的结果。
- `namespace_oid(id)`、`table_oid(id)`：分别调用 `signed_id` 和 `bounded`，输出 `[NATIVE_BASE, TABLE_BASE)` 与 `[TABLE_BASE, INDEX_BASE)` 中的 OID。
- `index_oid(table, index)`：分别编码 table ID 和 table-local index ID，再以 Cantor pairing `sum*(sum+1)/2+b` 保留二元身份，最后放入 `[INDEX_BASE, u32::MAX)`；它不会靠哈希或遍历顺序分配 OID。
- `name_parts(input)`：内部 `regclass` 名称词法解析器，返回一段或多段名称。未加引号部分只接受字母数字、`_`、`$` 并逐字符小写化；双引号部分保留大小写，`""` 解码为单个引号。
- `resolve(input, database, snapshot)`：`public_first = false` 的便捷入口。
- `resolve_with_path(input, database, snapshot, public_first)`：完整解析入口；`public_first` 控制无 schema 名称是先查当前数据库的 `public` 对象，还是先匹配系统关系。
- `display(oid, database, snapshot)`：反向扫描固定系统关系、当前数据库表及其索引，返回适合 `regclass` 文本输出的名称。
- `native_name`、`quoted`：确保反向输出可以再次安全解析；与系统关系同名的原生对象加 `public.`，不满足简单小写标识符规则的名称加双引号并转义内部双引号。

## 执行流程

OID 生成流程如下：调用者传入原生 ID；`signed_id` 先拒绝零并把正负数映射为不同正整数；schema/table 分支直接通过 `bounded` 加对应基址；index 分支先对 table 与 local index 的编码值做 Cantor 配对，再加 `INDEX_BASE`。每一步都用扩大后的整数和受检转换，越界即返回错误，不截断。

`resolve_with_path` 先对输入 `trim`。若非空且全为 ASCII 数字，直接解析为 `u32`；此分支允许 PostgreSQL OID 数值域中的任意值，包括尚未对应真实对象的值。否则由 `name_parts` 解析：一段视为未限定名称，两段视为 `schema.name`，三段或更多返回 `UnsupportedCommand(0)`。

对于未限定名称且 `public_first = true` 的调用，函数先获取当前数据库 `SchemaTableInfos`，精确匹配表名，再遍历每张表经 `pkg/server/pg_catalog.rs::catalog_indexes` 得到的索引。索引名命中一次即记下 OID，第二次命中报歧义。随后才匹配系统关系。默认 `resolve` 或 `public_first = false` 时顺序相反：未限定名称优先匹配 `SYSTEM_RELATIONS`。

显式 `pg_catalog.name` 只查询系统固定关系，未命中立即返回不存在；显式 `public.name` 或未限定且尚未命中的名称继续查询当前数据库。其他 schema 返回 `UnsupportedCommand(0)`。当前数据库为空时返回不存在。表名与索引名都在 PostgreSQL 折叠/引号处理之后做精确比较，刻意不使用原生大小写不敏感解析器。

`display` 先反查 `SYSTEM_RELATIONS`，再取得当前数据库的表列表，逐表比较 `table_oid`，继而逐索引比较 `index_oid`。匹配到原生名称后用 `native_name` 生成可回读文本；遍历结束仍未命中则返回十进制 OID 字符串。`pkg/server/pg_catalog.rs` 在 `regclass -> varchar` 路径调用它。

## 数据与状态

本文件自身没有可变全局状态。`SYSTEM_RELATIONS` 是编译期只读切片，三个区间基址也是常量；OID 结果只由输入持久 ID 决定。

OID 空间的不变量是：系统固定 OID 小于 `16384`；schema OID 位于 `[0x0000_4000, 0x4000_0000)`；table OID 位于 `[0x4000_0000, 0x8000_0000)`；index OID 位于 `[0x8000_0000, 0xffff_ffff)`。`u32::MAX` 是 `index_oid` 的排他上界，但数字文本解析仍可接受该数值，因为数字 cast 不要求对象实际存在。

动态元数据不被缓存。`resolve_with_path` 和 `display` 每次都通过传入的 `&dyn InfoSchema` 快照读取指定数据库的表，再从每个表的 `model_meta` 推导目录索引。因此同一次调用观察的是调用者提供的快照；文件不持有 snapshot，也不延长其生命周期。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::conn::{ConnError, ConnResult}`、`astersql_infoschema::{CiString, InfoSchema}`、`crate::pg_result::CatalogColumnType`，以及 `crate::pg_catalog::catalog_indexes`。`pkg/server/Cargo.toml` 将 `astersql-infoschema` 声明为同 workspace 路径依赖，`conn`、`pg_result`、`pg_catalog` 则是同一 crate 的模块。

RustCodeGraph 对 `table_oid` 的下游边为 `signed_id`、`bounded`、`TABLE_BASE`、`INDEX_BASE`；对 `index_oid` 的下游边为 `signed_id`、`bounded`、`range_error`、`INDEX_BASE`；对 `resolve_with_path` 的下游边包括 `name_parts`、`table_oid`、`index_oid`、`missing`、`range_error` 与 `SYSTEM_RELATIONS`。图的 `callers` 结果为空，因此上游以精确源码引用补证。

上游主要集中在 `pkg/server/pg_catalog.rs`：cast 执行路径调用 `resolve_with_path` 和 `display`；系统目录行生成调用三种 OID 函数；`pg_class`/`pg_namespace`/`pg_index`/`pg_depend` 等筛选也用同一映射比较输入 OID。`pkg/server/pg_catalog_query.rs::is_catalog_relation` 读取 `SYSTEM_RELATIONS`。`pkg/server/pg_catalog_test.rs` 与 `pkg/server/pg_catalog_query_test.rs` 还直接使用 OID 函数计算断言期望值。

## 错误处理与边界

- 原生 ID 为零、schema/table 映射越过所属区间、Cantor 配对前的和超过 `u32::MAX`、或最终 index OID 达到排他上界时，返回统一的 `ConnError::Session` 范围错误。
- 空名称、非法未引用字符、缺失结束引号、空名称段、尾随点、额外文本均由 `name_parts` 返回 `ConnError::Session("invalid PG regclass name")`。
- 数字输入只接受全 ASCII 数字；超出 `u32` 返回范围错误。负号不是数字快捷路径，会进入名称解析并作为非法字符处理。
- 名称超过 `schema.name` 两段时返回 `ConnError::UnsupportedCommand(0)`；非 `public`/`pg_catalog` schema 同样不支持。
- 名称不存在时 `missing` 产生 `Table '<input>' doesn't exist`。显式 `pg_catalog` 未命中不会回退到原生表。
- 同一数据库内多个表拥有同名索引时，名称解析返回 `ambiguous PG index relation name`，避免依赖遍历顺序选取对象。
- `SchemaTableInfos` 错误被转换为 `ConnError::Session(e.to_string())`；表缺少 `model_meta` 时返回 `UnsupportedCommand(0)`；`catalog_indexes` 的错误用 `?` 原样传播。
- `display` 对未知 OID不是错误，而是返回数值字符串。反向扫描过程中若任一原生 ID 无法合法映射，则错误会提前传播，而不是跳过该对象。

## 并发与资源生命周期

所有函数均为同步纯计算或只读快照查询，没有锁、通道、后台任务、网络 I/O、事务或显式资源释放。共享的 `SYSTEM_RELATIONS` 是不可变静态数据，可以并发读取。

`InfoSchema` 通过共享借用传入；本文件不克隆、不保存也不修改它。并发一致性因此由调用者选择的快照保证。`SchemaTableInfos` 返回的集合只在当前调用栈内遍历，索引 OID和名称字符串按需创建并在返回后正常释放。性能成本主要是名称解析/显示时对当前数据库表与索引的线性扫描；没有跨调用缓存失效问题，但扩展到超大 schema 时需关注扫描成本。

## 与 Go 版本的对应关系

`pkg/server/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/server"` 表明整个 crate 对齐 Go 的 `pkg/server` 包，但仓库中不存在 `pkg/server/pg_oid.go`；在 `pkg/server/**/*.go` 中也未找到 `regclass`、本文件范围错误文本或同名 OID 映射实现。因此，本文件没有可声明的一一对应 Go 源文件，当前直接证据表明它是 Rust PostgreSQL 适配层为 PG 系统目录和 `regclass` 行为新增的实现。

不能据此推断 Go server 拥有相同的三段 OID布局、Cantor pairing 或搜索路径语义。可验证的对齐对象是 PostgreSQL 客户端可见契约，以及 Rust 侧 `pg_catalog` 生产者与测试，而不是缺失的 Go 代码。若以后 Go 侧加入对应实现，应单独核对固定系统 OID、范围边界、大小写/引号规则、搜索顺序和同名索引歧义处理。

## 扩展指南

- 增加系统目录关系时，修改 `SYSTEM_RELATIONS`，选用与 PostgreSQL 兼容且小于 `NATIVE_BASE`、不重复的固定 OID；同时检查 `pg_catalog_query.rs::is_catalog_relation` 的自动纳入效果，并扩展 `pg_oid_test.rs` 的唯一性断言及相关目录查询测试。
- 调整对象种类或 OID 编码时，应优先修改 `signed_id`、`bounded` 或新增独立区间函数，保持跨重启稳定、不同对象种类不碰撞、table-local index 身份含 table ID、所有转换受检。修改既有公式属于持久兼容性变更，会改变客户端保存的 OID，风险很高。
- 扩展名称语法或搜索路径时，入口是 `name_parts` 与 `resolve_with_path`。必须同步验证未引用折叠、引用标识符、转义双引号、显式 schema、系统关系遮蔽、`public_first` 顺序、同名索引歧义和无当前数据库行为。
- 扩展反向文本表示时，修改 `display`、`native_name`、`quoted`，并保证输出能够由解析路径安全回读；尤其不能让 `public` 中与系统目录同名的表显示成无前缀名称。
- 测试逻辑应继续放在独立文件 `pkg/server/pg_oid_test.rs`，端到端 cast/查询覆盖放在现有 `pkg/server/pg_catalog_query_test.rs`、`pkg/server/pg_catalog_test.rs` 或其他相应独立测试文件，不要把测试内嵌进生产源文件。
- 当前解析和显示是线性扫描。若引入缓存，必须明确 snapshot/DDL 版本作为失效键，并保持同一快照内一致性；否则宁可保留现有无状态实现。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；通过 `node --file pkg/server/pg_oid.rs` 阅读了完整 266 行生产源码。
- RustCodeGraph 符号/调用查询：查询了 `namespace_oid`、`table_oid`、`index_oid`、`resolve`、`resolve_with_path`、`display`；并对 `table_oid`、`index_oid`、`resolve_with_path` 执行 `callers`/`callees`。调用者图为空的部分以 `rg` 精确引用结果补齐，未将空图误写成“无人调用”。
- crate 与模块证据：读取 `pkg/server/Cargo.toml` 和 `pkg/server/lib.rs`；确认 `astersql-server`、`astersql-infoschema` 路径依赖、私有 `pg_oid` 模块及独立 `pg_oid_test.rs` 挂载。
- 直接实现/调用证据：读取 `pkg/server/pg_oid.rs`、`pkg/server/pg_catalog.rs` 的 cast 与目录行生成片段、`pkg/server/pg_catalog_query.rs::is_catalog_relation`，以及 `pkg/server/pg_result.rs::CatalogColumnType` 的符号位置。
- 测试证据：读取 `pkg/server/pg_oid_test.rs`，其验证区间隔离、零值与极值拒绝、负 ID 区分、系统 OID及小范围原生 OID唯一性；读取 `pkg/server/pg_query_test.rs::namespace_catalog_live_metadata` 的边界和跨连接稳定性断言；搜索并核对 `pkg/server/pg_catalog_query_test.rs` 与 `pkg/server/pg_catalog_test.rs` 中数字 OID、系统/原生表、索引和 `regclass` cast 的调用场景。
- Go 对照证据：检查确认 `pkg/server/pg_oid.go` 不存在，并在 `pkg/server/**/*.go` 搜索 `regclass`、范围错误和歧义错误文本无结果；因此只记录“无直接 Go 对照”，不推测未实现语义。
- 本任务是纯文档分析，按计划不运行 Cargo。最终还需以任务指定命令确认目标文档存在且恰有十一个固定二级标题。
