// Copyright 2026 AsterSQL.

// 真实二进制入口只做一次转发，避免把主流程硬编码在 Cargo bin 壳层。
fn main() {
    astersql_dumpling_cmd_dumpling::main();
}
