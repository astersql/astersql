// Copyright 2026 AsterSQL.

//! TiDB Server 可执行文件的薄封装入口。
//!
//! 这里只负责把进程启动委托给库 crate，使二进制运行与库模式复用同一套服务初始化和退出处理。

/// 启动 TiDB Server 的共享主流程，不在二进制壳层重复实现服务逻辑。
#[global_allocator]
static HEAP_PROFILER: rpprof::alloc::AllocProfiler = rpprof::alloc::AllocProfiler::system();

fn main() {
    rpprof::alloc::start();
    astersql_cmd_tidb_server::main();
}
