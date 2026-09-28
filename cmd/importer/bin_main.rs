// Copyright 2026 AsterSQL.

//! importer 可执行文件的最外层入口。
//! 这里只把控制权转交给库 crate，避免在二进制包装层重复参数解析、退出码处理和导入流程，
//! 确保所有调用都经过同一份主流程实现。

/// 启动 importer crate 提供的共享进程入口。
fn main() {
    astersql_cmd_importer::main();
}
