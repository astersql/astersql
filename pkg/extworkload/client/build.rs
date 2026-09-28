// Copyright 2026 AsterSQL.

// Cargo build 脚本：编译 external workload 的 protobuf/gRPC 代码。
//
// 使用 vendored protoc 生成 tonic 服务端与客户端桩代码，
// 并声明对 proto 文件的变更重跑依赖。

fn main() {
    // 定位内置 protoc 二进制，避免依赖系统安装。
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    // SAFETY: Cargo runs this package's build script as a dedicated process.
    // 仅在 build 脚本进程内设置 PROTOC，供 tonic_build 调用。
    unsafe { std::env::set_var("PROTOC", protoc) };
    // 生成服务端桩并编译 externalworkload.proto。
    tonic_build::configure()
        .build_server(true)
        .compile_protos(&["../proto/externalworkload.proto"], &["../proto"])
        .expect("compile external workload proto");
    println!("cargo:rerun-if-changed=../proto/externalworkload.proto");
}
