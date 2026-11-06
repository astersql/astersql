# pg_function.rs

函数 EXECUTE 的共享持久授权消费者，范围限 PG_INTROSPECTION_FUNCTION_IDENTITIES 三条实际注册原语。沿 PgStandardSchema 的初始化来源标志、逐数据库显式 ACL 与 owner 构造 PgDatabaseFact，目录 proacl、has_function_privilege 和实际执行使用同一事实。默认 owner/PUBLIC EXECUTE 的 ACL 投影为 SQL NULL；显式空 ACL 为 {}。旧记录缺字段保留未知并返回 0A000。

完整 text 输入按函数名与 oid 输入签名解析，拒绝不同重载；OID 输入按本端注册身份查找。权限仅 EXECUTE，逗号项全部验证后按任一匹配。GRANT/REVOKE 复用 update_object_acl 的 grantor、成员继承、RESTRICT/CASCADE 与 grant option 规则；不引入第二套授权引擎。角色删除检查 owner/grantor/grantee 引用，owner 转移改写并合并显式 ACL。

独立测试：pg_catalog_test::cat14_23（真实非root服务协议）与 pg_schema_test::cat14_23_historical_function_acl_is_not_backfilled（真实KV缺源）。未开放任意用户函数、扩展例程或完整 CREATE FUNCTION。

## 2026-11-07 目录任务26：真实函数签名可见性

注册验证真实namespace CREATE/owner与callback类型，复用PgStandardSchema和NextGlobalID事务保存稳定身份；临时声明归原会话。卸载具体OID并移除ACL，重建新OID，缺工厂可按原实现ID显式重连。普通调用与可见性共用namespace/path/OID声明，回调前检查真实EXECUTE并注入当前认证身份/活跃角色及共享权限；运行时scope以weak Domain生命周期区分，防止地址复用命中旧工厂。

动态权限的principal遵循PG SET ROLE，并复用pg_inherited_privileges及显式声明的继承闭包；认证User与ActiveRoles提供者仍保持真实连接身份/激活列表。INHERIT FALSE不借用角色授权，显式SET允许使用该角色直接授权；没有新建授权系统。

普通解析、Parse/Describe与协议拆句仅接收当前数据库/连接登记名；namespace、完整输入OID及EXECUTE仍在共享事实解析。正式回调不缓存对象绑定；PG在prepared计划退到另一schema后新增同名函数的依赖失效时机不在此局部实现范围，参考夹具以实际改变路径并执行一次后重验。
