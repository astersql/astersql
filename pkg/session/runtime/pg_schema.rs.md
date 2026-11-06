# `pkg/session/runtime/pg_schema.rs`

数据库内PostgreSQL namespace的持久身份、owner、USAGE/CREATE事实、search_path绑定与原生关系名桥接。Namespace记录保存在现有共享KV，临时关系身份只归当前连接；不为PG namespace另建原生数据库。

## 数据库授权消费者

CREATE SCHEMA在写入namespace前调用`pg_require_database_privilege(4)`，与共享数据库CREATE查询一致；非root CREATE SCHEMA AUTHORIZATION其他角色仍保留现有局部门禁。`pg_allocate_temporary_identity`在每次临时关系分配前调用`pg_require_database_privilege(2)`，撤销TEMPORARY会拒绝新的临时对象，不删除已有临时表。

`pg_bind_relation`先处理当前连接未限定schema的本地临时表，保持其遮蔽永久关系和撤权后的已有表可用；永久对象仍按有效search_path与namespace USAGE/CREATE事实绑定。ACL事实未知不会转换成无权限false，bootstrap管理路径保留历史兼容边界。

聚焦回归：`pg_catalog_test::cat14_17`；namespace owner/绑定既有回归分别由cat14_13、cat14_14及pg_schema_test维护。类型/函数owner类别不属于数据库ACL事实扩展。
