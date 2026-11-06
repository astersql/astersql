# pg_function.rs

函数 EXECUTE 的共享持久授权消费者，范围限 PG_INTROSPECTION_FUNCTION_IDENTITIES 三条实际注册原语。沿 PgStandardSchema 的初始化来源标志、逐数据库显式 ACL 与 owner 构造 PgDatabaseFact，目录 proacl、has_function_privilege 和实际执行使用同一事实。默认 owner/PUBLIC EXECUTE 的 ACL 投影为 SQL NULL；显式空 ACL 为 {}。旧记录缺字段保留未知并返回 0A000。

完整 text 输入按函数名与 oid 输入签名解析，拒绝不同重载；OID 输入按本端注册身份查找。权限仅 EXECUTE，逗号项全部验证后按任一匹配。GRANT/REVOKE 复用 update_object_acl 的 grantor、成员继承、RESTRICT/CASCADE 与 grant option 规则；不引入第二套授权引擎。角色删除检查 owner/grantor/grantee 引用，owner 转移改写并合并显式 ACL。

独立测试：pg_catalog_test::cat14_23（真实非root服务协议）与 pg_schema_test::cat14_23_historical_function_acl_is_not_backfilled（真实KV缺源）。未开放任意用户函数、扩展例程或完整 CREATE FUNCTION。
