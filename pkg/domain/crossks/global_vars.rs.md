# `pkg/domain/crossks/global_vars.rs`

## 文件定位

跨 keyspace runtime 的只读全局变量缓存，对应 Go f95ec32c8c038267a99abab991e9f754f8b49ba2 新增 global_vars.go。

## 核心职责

通过目标系统会话池读取 mysql.global_variables 和 mysql.tidb，缓存前者并拒绝所有写操作，不调用实例 SetGlobal 钩子。

## 主要符号

GlobalVarCache 是共享状态句柄；Query 是池查询边界；SYS_VAR_CACHE_KEY 是 /tidb/sysvars 通知键。reload、refresh_started、close 管理刷新生命周期。

## 执行流程

已注册变量首次读取时串行加载表，调用 SysVarCacheValues 计算默认值与作用域，再 ValidateFromType 规范化；非法值回退定义默认值。成功后启动刷新，监听目标 etcd 命名空间的通知和 30 秒周期。mysql.tidb 不缓存，逐次查询；未知变量报错，晚注册变量回退默认值。

## 数据与状态

RwLock 保存完整全局快照，load Mutex 串行化读取和发布，AtomicBool 标记首次加载。缓存克隆共享状态。读失败不发布部分结果，刷新失败保留旧快照。

## 依赖与调用关系

KeyspaceSessionFactory 创建并安装缓存，SystemSessionPool 为真实会话安装 GlobalVarsAccessor。SQL 使用实际池 lease，自动归还资源。etcd 复用 astersql-ddl-schemaver 的真实 watch 接口。

## 错误处理与边界

未知系统变量、SQL 错误、缺失 mysql.tidb 行与写操作拒绝均传播 VariableError。写错误包括变量名和目标 keyspace。类型校验仅用 ValidateFromType，不应用 SetGlobal。GetGlobal 钩子仍由上层变量读取入口决定，缓存访问器本身返回表值。

## 并发与资源生命周期

冷缓存双重检查避免重复加载。SessionManager::close 先取消并 join 刷新，再关闭池和 Store。查询闭包弱引用池，避免引用环。首次读取前不访问 etcd 或执行 SQL；watch 断开重新订阅，连续超过 10 次时可取消退避。

## 与 Go 版本的对应关系

对应 get、loadIfEmpty、loadLocked、query、refreshLoop 和 GlobalVarAccessor 的全部读写接口。Rust 使用线程、可取消 Context 与 watch recv_timeout；Go 使用 goroutine 和 channel。真实查询由 session crate 连接，跨 crate 不复制执行器。Go 的 InternalTxnSysVar 在当前 Rust kv 中是 InternalTxnOthers 别名；查询复用既有受限系统池，不另建上下文/请求来源框架。线上 RU/metrics 标签未作为本地验证结论。

## 扩展指南

新增变量沿用统一注册表和类型校验，不在缓存内复制默认值。不添加实例设置副作用。新增缓存资源需保持取消、join、关闭 pool 的顺序，并用真实池 SQL 回归覆盖。

真实池 SessionVars::GetGlobalSystemVar 通过 get_global_sys_var_from_hook 使用既有 canonical SysVar::GetGlobalFromHook；直接 GlobalVarsAccessor::get_global_sys_var 仍返回目标缓存表值。只有当前已验证为实例状态/全局访问器读取的 GetGlobal 钩子经该适配器分派；不复制任意会话局部状态。watch 重订阅和退避后重新计算 30 秒期限，匹配 Go 每轮 select 的 time.After。

refresh_started 表示首次成功读取的历史，不因取消而清除；关闭后不再查询或 watch，但保留已加载快照。watch 断开记录目标 keyspace 告警后重订阅，沿用 Go 的连续失败退避。
