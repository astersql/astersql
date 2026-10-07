# `pkg/parser/mysql/privs.rs`

源文件：[privs.rs](privs.rs)

## 文件定位

本文件属于 `astersql-parser-mysql` crate，由同目录 `lib.rs` 以 `pub mod privs` 暴露。它位于 SQL 解析器与权限执行/缓存之间的公共元数据边界：为 AST 构造、`GRANT`/`REVOKE`、权限系统表编解码提供统一的静态权限位、显示名称、系统表列名和作用域集合。源码注释及实现表明它只构造和查询进程内数据，不访问 `mysql.user`、不执行 SQL，也不进行网络或磁盘 I/O。

`pkg/parser/mysql/Cargo.toml` 将该 crate 定义为 edition 2024、不可发布的 workspace library，入口是 `lib.rs`；crate 依赖 `astersql-errors`、`semver` 和 `unicode-general-category`，但本文件自身只使用标准库 `std::collections::HashMap`。

## 核心职责

1. 用 `PrivilegeType(pub u64)` 及一组单 bit 常量表示 MySQL/TiDB 静态权限，并固定与 Go `iota` 相同的位序。
2. 在三种名称空间之间转换：SQL/展示文本（`Priv2Str`）、`mysql.user`/`mysql.db` 列名（`Priv2UserCol`）和 `Table_priv`/`Column_priv` SET 枚举文本（`Priv2SetStr`、`SetStr2Priv`）。
3. 提供按全局、数据库、表、列以及“仅全局静态权限”分类的权限列表，供授权合法性判断、展开 `ALL` 和系统表读写使用。
4. 提供 `PrivilegesExt::Has` 这一精确成员查询；它不解释掩码、`AllPriv` 或动态权限语义。

本文件不负责用户/角色解析、授权决策、系统表事务或动态权限注册；这些行为位于上层 parser action、executor、session 和 privilege cache 中。

## 主要符号

- `AllPrivilegeLiteral: &str`：`AllPriv` 的展示文本 `ALL PRIVILEGES`。
- `PrivilegeType(pub u64)`：可复制、可哈希的权限位值包装；公开元组字段允许调用者读取或组合底层位值。
- `UsagePriv` 至 `OperateViewPriv`：bit 0 至 bit 33 的静态权限。`AllPriv` 是 bit 34 哨兵，`ExtendedPriv` 是 bit 35 的扩展/动态权限解析占位。
- `AllPrivMask`：值为 `AllPriv.0 - 1`，因此覆盖 `AllPriv` 之前的 bit 0..33，但不包含 `AllPriv` 和 `ExtendedPriv`。
- `Priv2Str()`：生成权限位到 SQL/展示文本的映射；包含 `UsagePriv` 和 `AllPriv`。
- `Priv2UserCol()` / `Col2PrivType()`：生成静态权限位与权限系统表列名的正反向映射；不为 `UsagePriv`、`AllPriv`、`ExtendedPriv` 提供列。
- `Priv2SetStr()` / `SetStr2Priv()`：生成表/列 SET 文本映射。两边并非覆盖所有静态权限；只覆盖适用于该 SET 表示的项。
- `NewPrivFromColumn(&str) -> Option<PrivilegeType>`、`NewPrivFromSetEnum(&str) -> Option<PrivilegeType>`：严格、区分大小写的反向查找；未知输入返回 `None`。
- `PrivilegeType::{String, ColumnString, SetString}`：分别查询三种正向文本；未映射的值返回空字符串。
- `Privileges = Vec<PrivilegeType>` 与 `PrivilegesExt::Has`：列表别名及基于 `Vec::contains` 的线性精确匹配。
- `AllGlobalPrivs()`、`AllDBPrivs()`、`AllTablePrivs()`、`AllColumnPrivs()`：按作用域返回新的权限列表。
- `StaticGlobalOnlyPrivs()`：返回不能下放到较小作用域的静态权限集合，不包含动态权限。

## 执行流程

解析权限名称时，上层先从 SQL 语法得到权限项，随后使用本文件的权限常量或反向映射形成 `PrivilegeType`。系统表列名走 `NewPrivFromColumn`：函数构造 `Col2PrivType`，按原始字符串精确查找并复制值；SET 枚举走 `NewPrivFromSetEnum` 和 `SetStr2Priv`，流程相同。失败不会纠正大小写或猜测最接近的权限，而是返回 `None`。

输出权限元数据时，`String`、`ColumnString` 或 `SetString` 构造对应正向映射并查值。已登记值返回静态字符串；不属于该名称空间的值返回 `""`。因此调用者必须根据场景选择映射，不能把空字符串解释为合法名称。

处理作用域时，上层调用 `All*Privs()` 得到保持 Go 顺序的新 `Vec`，再用于展开 `ALL`、校验权限级别或系统表编解码。`PrivilegesExt::Has` 只检查列表中是否存在完全相等的一个 `PrivilegeType`，不会把 `AllPriv` 展开为所有权限，也不会对底层 `u64` 做按位包含判断。

## 数据与状态

权限位是协议兼容数据。源码显式展开 Go 的 `iota` 顺序：`UsagePriv = 1 << 0`，`OperateViewPriv = 1 << 33`，`AllPriv = 1 << 34`，`ExtendedPriv = 1 << 35`。新增静态权限若要进入 `AllPrivMask`，必须插在 `AllPriv` 之前并同步 Go 位序、所有相关映射、作用域列表和独立测试；改变既有 bit 会破坏持久化掩码及跨版本兼容性。

映射和作用域函数每次调用都新建并返回拥有所有权的 `HashMap`/`Vec`，模块中没有全局可变状态或惰性缓存。键和值是值类型与 `&'static str`，返回集合可由调用者独立修改而不会影响后续调用。代价是每次映射查询都会重新分配并填充容器；当前实现以忠实对应 Go 表结构和简单 API 为主。

三套映射的覆盖范围有意不同。`Priv2Str` 比 `Priv2UserCol` 多 `UsagePriv` 与 `AllPriv`；SET 映射只服务表/列权限。`Priv2SetStr` 当前包含角色和 `ShutdownPriv -> "Shutdown Role"` 等条目，而 `SetStr2Priv` 不包含这些条目的全部反向映射；安全扩展不能假定整个 `Priv2SetStr` 都可往返，只能依赖相关作用域测试所覆盖的集合。

## 依赖与调用关系

下游依赖只有标准库 `HashMap`；所有业务语义来自本文件内的常量和表。crate 入口 `pkg/parser/mysql/lib.rs` 公开 `privs` 模块，使其他 workspace crate 通过 `astersql_parser_mysql::privs` 使用它。

RustCodeGraph 对目标文件的检索显示，直接或间接使用者覆盖解析、执行和权限缓存链路：

- `pkg/parser/parser_actions/security.rs` 的安全语法 action 在构造权限 AST 时使用权限常量。
- `pkg/session/runtime/control.rs` 的 `execute_grant`/`execute_revoke` 使用 `Priv2UserCol` 对应的列名和权限掩码，把解析结果接入会话级授权执行。
- `pkg/privilege/privileges/cache.rs` 的权限表 SET 解码/编码维护同一组文本语义；例如 `setStrToPrivilege`、`decodeSetToPrivilege` 和 `EncodePrivilegeSet` 使用 `Create`、`Grant`、`Operate View` 等枚举名。
- RustCodeGraph 的调用结果还列出 `pkg/executor/grant.rs`、`pkg/executor/infoschema_reader.rs`、`pkg/parser/ast/sql_restore.rs` 以及 `pkg/parser/mysql/privs_test.rs` 等使用者，说明本文件是共享元数据定义，不是完整授权实现。

这些调用关系形成“parser 权限项 -> 本文件的标准权限位/名称 -> executor/session 写入或 privilege cache 编解码”的边界；最终是否授权仍由权限管理器和执行层决定。

## 错误处理与边界

本文件没有 `Result`、自定义错误或 panic 路径。两个反向解析函数通过 `Option` 显式表示未知字符串；三个正向格式化方法则沿用 Go 行为，以空字符串表示未登记值。查找均区分大小写和空白，不做规范化。

`AllPriv` 是哨兵权限而非普通作用域成员，`AllPrivMask` 是“任意静态权限位”的掩码；`ExtendedPriv` 只表示解析到不在静态表中的扩展权限类别，具体动态权限名称和注册状态不存放在这里。`Has` 对组合掩码没有特殊含义，传入 `PrivilegeType(a | b)` 只有列表恰好保存同一组合值时才命中。

位宽边界为 `u64`。当前最高使用 bit 35，尚有空间，但任何新增位都必须保持 `AllPriv` 哨兵及 `AllPrivMask` 契约；不能只在末尾追加静态位，否则它不会被 `AllPrivMask` 包含。

## 并发与资源生命周期

模块不持有锁、事务、任务、通道、文件句柄或网络连接。常量和静态字符串只读且具有进程生命周期；函数返回的新 `HashMap`/`Vec` 由调用者拥有，在离开作用域时正常释放。因没有共享可变状态，各函数自身可并发调用。

并发安全不等于零成本：高频调用 `String`、`ColumnString`、`SetString` 或反向解析会重复分配映射。若未来引入全局缓存，必须选择线程安全的只读初始化方式，并保持返回/借用 API 与调用者修改集合的现有隔离语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/privs.go`，Rust 基本逐项保留其结构：Go 的包级 map 对应 Rust 的同名构造函数；Go `PrivilegeType uint64` 对应元组结构；Go `(PrivilegeType, bool)` 查找对应 Rust `Option<PrivilegeType>`；Go slice 及 `slices.Contains` 对应 `Vec` 与 `contains`；Go 包级作用域变量对应返回新列表的函数。

两版的权限文本、列名、SET 文本、bit 位序和各作用域顺序一致，包括 `OperateViewPriv` 位于 `AllPriv` 之前。Rust 独立测试 `pkg/parser/mysql/privs_test.rs` 对照 `pkg/parser/mysql/privs_test.go`，并额外以 `go_merge_31_operate_view_privilege` 锁定 `OperateViewPriv = 1 << 33`、`AllPriv = 1 << 34`、`ExtendedPriv = 1 << 35`、`AllPrivMask = (1 << 34) - 1` 及其作用域归属。

需要注意实现形态差异：Go map/slice 是包级共享变量，Rust 为函数内重建并返回拥有所有权的容器；Rust 的 `PrivilegeType` 不是整数别名，按位运算需显式操作 `.0` 或由其他层转换。两者的可观察查找和列表内容由当前测试保持一致。

## 扩展指南

新增静态权限时，应把兼容性作为首要约束：先在 Go 与 Rust 中把新 bit 放在 `AllPriv` 之前，确认后续哨兵位迁移是否符合跨版本方案；随后同步 `Priv2Str`、需要的系统表列正反向映射、SET 正反向映射以及所有适用的 `All*Privs`/`StaticGlobalOnlyPrivs`。若只增加动态权限名称，不应占用这里的新静态 bit，而应接入上层动态权限注册与缓存路径。

修改映射时要分别验证三种名称空间，不要用展示文本替代 SET 文本或系统表列名。尤其应检查正反向映射是否在目标作用域内闭合，并评估 `pkg/privilege/privileges/cache.rs` 的重复文本表是否需要同步。

测试必须放在独立的 `pkg/parser/mysql/privs_test.rs`，不要嵌入生产文件。至少同步：所有 bit 到 `AllPriv` 的非空显示名；全局/DB 权限的列名往返；表/列权限的 SET 往返；作用域正负归属；`AllPrivMask` 与哨兵位；未知、大小写不同和不适用名称空间的回退。Go 行为变化时还应同步 `pkg/parser/mysql/privs_test.go` 的意图。

若要优化重复构造，可考虑不可变静态表或一次性初始化缓存，但应先用基准证明必要性，并避免把可变共享 map 暴露给调用者；这属于性能重构，不应顺带改变映射内容或错误回退。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/mysql` 确认目标 Rust/Go/测试/入口文件均已索引。
- RustCodeGraph `explore "pkg/parser/mysql/privs.rs privilege constants PrivilegeType PrivilegeSet"`：确认 `NewPrivFromColumn`、`NewPrivFromSetEnum`、`Priv2Str`、`Priv2SetStr`、`Priv2UserCol`、`Has` 等内部调用，以及 parser、executor、session、privilege cache 和测试侧使用者。
- RustCodeGraph `node --file pkg/parser/mysql/privs.rs --offset 1 --limit 500`：通读目标文件 437 行，核对全部公开常量、函数、类型、trait、impl 和作用域列表；文件无条件编译项。
- RustCodeGraph `query`：确认 Rust `AllGlobalPrivs() -> Privileges`、`NewPrivFromColumn(&str) -> Option<PrivilegeType>`、`StaticGlobalOnlyPrivs() -> Privileges` 的精确符号与签名。
- RustCodeGraph `node`：抽查 `pkg/parser/parser_actions/security.rs`、`pkg/session/runtime/control.rs`、`pkg/executor/grant.rs`、`pkg/privilege/privileges/cache.rs` 的直接边界与同名权限文本语义。
- 逐文件读取：`pkg/parser/mysql/Cargo.toml`、`pkg/parser/mysql/lib.rs`、`pkg/parser/mysql/privs.go`、`pkg/parser/mysql/privs_test.rs`、`pkg/parser/mysql/privs_test.go`。目录中不存在 `doc.go`，因此没有额外包契约可读。
- 人工复核：文档区分了元数据与授权执行，逐项说明位序、映射覆盖、失败回退、资源生命周期、Go 差异和安全扩展位置；未把未观察到的 I/O、锁或授权能力写成现状。
