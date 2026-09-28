// Copyright 2026 AsterSQL.

// `util/resourcegrouptag` 的 build 脚本：从官方 proto 生成 kvproto / tipb 绑定。
//
// 在编译期用 protobuf-codegen-pure 生成 Rust 代码，供资源组标签解码依赖的
// kvrpcpb、coprocessor、ResourceGroupTag 等消息类型使用。

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let proto_root = manifest_dir.join("proto");
    let proto = proto_root.join("kvproto");
    let includes = proto_root.join("include");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("kvproto");
    std::fs::create_dir_all(&out).expect("create generated kvproto directory");

    // 与资源组标签/RPC 相关的官方 kvproto 子集。
    let inputs = [
        "coprocessor.proto",
        "deadlock.proto",
        "encryptionpb.proto",
        "errorpb.proto",
        "kvrpcpb.proto",
        "metapb.proto",
        "resource_manager.proto",
        "tracepb.proto",
    ]
    .into_iter()
    .map(|name| proto.join(name))
    .collect::<Vec<_>>();
    let input_strings = inputs
        .iter()
        .map(|path| path.to_str().unwrap())
        .collect::<Vec<_>>();
    let include_paths = [&proto, &includes];
    let include_strings = include_paths
        .iter()
        .map(|path| path.to_str().unwrap())
        .collect::<Vec<_>>();
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: out.to_str().unwrap(),
        includes: &include_strings,
        input: &input_strings,
        customize: Default::default(),
    })
    .expect("generate bindings from official kvproto schemas");
    // 部分 codegen 版本生成 RUV2，补兼容别名 Ruv2，避免下游拼写不一致。
    let kvrpcpb = out.join("kvrpcpb.rs");
    let mut kvrpcpb_source = std::fs::read_to_string(&kvrpcpb).expect("read generated kvrpcpb");
    if kvrpcpb_source.contains("pub struct RUV2")
        && !kvrpcpb_source.contains("pub type Ruv2 = RUV2;")
    {
        kvrpcpb_source.push_str("\n// Stable spelling across protobuf-codegen 2.8 sources.\n");
        kvrpcpb_source.push_str("pub type Ruv2 = RUV2;\n");
        std::fs::write(&kvrpcpb, kvrpcpb_source).expect("write Ruv2 compatibility alias");
    }
    // 写出按模块名排序的 kvproto/mod.rs，供 lib.rs include!。
    let mut modules = inputs
        .iter()
        .map(|path| path.file_stem().unwrap().to_str().unwrap())
        .collect::<Vec<_>>();
    modules.sort_unstable();
    std::fs::write(
        out.join("mod.rs"),
        modules
            .into_iter()
            .map(|module| format!("pub mod {module};\n"))
            .collect::<String>(),
    )
    .expect("write generated kvproto module list");

    // tipb 仅生成资源组标签相关的 resourcetag.proto。
    let tipb_out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tipb");
    std::fs::create_dir_all(&tipb_out).expect("create generated tipb directory");
    let tipb_proto = proto_root.join("tipb");
    let tipb_input = tipb_proto.join("resourcetag.proto");
    let tipb_includes = [&tipb_proto, &includes];
    let tipb_include_strings = tipb_includes
        .iter()
        .map(|path| path.to_str().unwrap())
        .collect::<Vec<_>>();
    protobuf_codegen_pure::run(protobuf_codegen_pure::Args {
        out_dir: tipb_out.to_str().unwrap(),
        includes: &tipb_include_strings,
        input: &[tipb_input.to_str().unwrap()],
        customize: Default::default(),
    })
    .expect("generate binding from official tipb resource tag schema");
    std::fs::write(
        tipb_out.join("mod.rs"),
        "pub mod resourcetag;\npub use resourcetag::*;\n",
    )
    .expect("write generated tipb module list");

    println!("cargo:rerun-if-changed={}", proto_root.display());
}
