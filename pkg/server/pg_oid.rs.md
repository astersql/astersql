# `pkg/server/pg_oid.rs`

## 文件定位与职责

PG 目录与普通查询共享持久模型 ID；本模块生成 namespace/table/index OID，解析 regclass 名称并反向显示。调用方包括 pg_catalog、server runtime 和 session runtime；不存在独立 OID 分配器或可变全局缓存。

## 身份映射

`namespace_oid`、`table_oid`、`index_oid` 通过 `signed_id` 将原生正负非零 ID 区分编码，再使用受检区间映射。namespace 位于 `[16384,0x40000000)`，table 位于 `[0x40000000,0x80000000)`，index 位于 `[0x80000000,u32::MAX)`。索引 ID 是表局部身份，因此 Cantor 配对同时包含 table ID 和 index ID。越界拒绝，不哈希、不截断。系统关系复用 session runtime 的 `PG_SYSTEM_RELATIONS`。

## 名称输入

`name_parts` 保留双引号大小写、点号及双引号转义；未引号文本折叠为小写，非法语法返回 42602。`resolve_in_path` 用于 regclass 输入函数：数字直接表示 u32 OID，`-` 表示 OID 0；这不要求对象存在。`resolve_name_in_path` 则只按名称查找，供已声明为 text 的强制转换使用，数字文本也必须命中实际关系。

两条入口共用 `pg_catalog::class_rows` 的 OID、名称和 namespace 事实。未限定名称按有效路径选择首个对象；路径未显式包含 pg_catalog 时将它隐式置前。显式 schema 通过真实 `PgNamespaceFact.usage` 检查，无 USAGE 返回 42501；未知关系返回 42P01，无法区分的同名索引返回 42725。跨数据库输入保持 0A000。`resolve` / `resolve_with_path` 是既有默认路径包装。

## 显示与往返

`display_in_path` 从同一快照的 class_rows 反查身份。OID 0 显示 `-`；其他未知 OID 显示十进制数字。已存在对象先尝试未限定名称，只有按当前路径重新解析仍为同一 OID 才省略 schema；被遮蔽或不可见时输出 schema 限定名。显示不授予 schema USAGE，重新按名称读取仍执行权限检查。

`quoted` 将点号、大小写、嵌入双引号等不符合简单小写标识符规则的名称正确加双引号。`needs_keyword_quotes` 使用 PostgreSQL 18 kwlist.h 的非 UNRESERVED 关键字类别，避免把关键字对象名显示为 SQL 关键字；不能使用类别不同的 MySQL 词表。`display` 是默认路径包装。目录、regclass 和普通绑定均使用真实名称和持久身份，不按客户端构造结果。

## 验证边界

独立测试位于 `pg_catalog_test.rs::cat14_27`，经真实 Domain、ConcreteSessionDriver 和 PgService 验证路径、大小写与特殊字符、关键字、OID 显示往返、真实非 root USAGE 授撤、NULL、text/binary 扩展参数及事务恢复。动态 OID 与本端模型关联，不与参考端数值比较。完整客户端和真实 TiKV 不由此测试替代。
