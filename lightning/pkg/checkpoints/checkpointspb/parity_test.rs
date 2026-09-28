// Copyright 2026 AsterSQL.

//! 中文总览：该测试文件只验证 `file_checkpoints.pb.rs` 暴露出来的 protobuf 公共契约。
//! 这里不关心业务层检查点调度，而是聚焦生成代码是否继续与 Go `checkpointspb` 包保持同一份线协议。
//! 测试主体把多个兼容性场景串成一次大回归，便于在生成器或手工修补后尽快发现序列化协议漂移。
//! 阅读时可按「正常 round-trip」「空输入边界」「异常输入」「兼容辅助接口」四段理解。
//! 这些中文注释只解释断言意图、边界条件和 Go 语义参照，不改变任何测试步骤或结果。
//! 由于目标是生成 protobuf 代码的对齐性，最重要的观察点是编码长度、未知字段处理和旧接口兼容面。
//! 如果这里回归，通常意味着 Rust 侧序列化字节流已不再能与 Go 侧稳定互通。

use super::*;
use std::collections::HashMap;

#[test]
// 该回归入口覆盖 Rust 侧对外仍需承诺的最小公共面，避免与 Go 生成物在 wire format 上悄悄分叉。
// 之所以把多个子场景放在同一个测试里，是因为它们共同依赖同一组 protobuf 类型和辅助函数。
fn go_rust_public_contract_matches() {
    // Normal: full nested checkpoint round-trip.
    // 先构造一个字段尽量齐全的 chunk，确保 varint、fixed64、sfixed64、字符串与 packed repeated 会一起经过编码路径。
    // `ColumnPermutation` 里混入 `-1`，是为了验证 Rust 侧对 Go zigzag/int32 列置换语义没有做额外收窄。
    let mut chunk = ChunkCheckpointModel {
        Path: "data/a.csv".into(),
        Offset: 100,
        ColumnPermutation: vec![2, 0, 1, -1],
        EndOffset: 500,
        Pos: 42,
        PrevRowidMax: 10,
        RowidMax: 99,
        KvcBytes: 1024,
        KvcKvs: 8,
        KvcChecksum: 0xdead_beef_cafe_u64,
        Timestamp: -123456789,
        Type: 1,
        Compression: 2,
        SortKey: "sk".into(),
        FileSize: 4096,
        RealPos: 77,
    };
    // engine 只放一个 chunk 键值对，足以覆盖 map entry 编解码，同时让后面的 table 可以复用同一个嵌套结构。
    let mut engine = EngineCheckpointModel {
        Status: 30,
        Chunks: HashMap::new(),
    };
    engine.Chunks.insert("data/a.csv:100".into(), chunk.clone());

    // table 同时放入负数、零和正数 engine key，目的是覆盖 Go 生成代码里 `zigzag32` map key 的兼容性。
    // 这些统计字段都取非零值，避免 proto3 的“零值省略”把目标字段从字节流里完全优化掉。
    let mut table = TableCheckpointModel {
        Hash: b"hash-bytes".to_vec(),
        Status: 60,
        Engines: HashMap::new(),
        TableID: 1001,
        KvBytes: 2048,
        KvKvs: 16,
        KvChecksum: 0x1122_3344_5566_7788,
        TableInfo: b"{\"cols\":3}".to_vec(),
        AutoRandBase: 7,
        AutoIncrBase: 8,
        AutoRowIDBase: 9,
    };
    table.Engines.insert(-3, engine.clone());
    table.Engines.insert(0, EngineCheckpointModel::default());
    table.Engines.insert(5, engine);

    // task 代表顶层可选消息，覆盖字符串、端口号和版本号等常见 Lightning 元数据字段。
    let task = TaskCheckpointModel {
        TaskId: 42,
        SourceDir: "/data/src".into(),
        Backend: "local".into(),
        ImporterAddr: "127.0.0.1:8287".into(),
        TidbHost: "127.0.0.1".into(),
        TidbPort: 4000,
        PdAddr: "127.0.0.1:2379".into(),
        SortedKvDir: "/tmp/sorted".into(),
        LightningVer: "v8.0.0".into(),
    };

    // 顶层模型同时带 table map 与可选 task，用来验证完整嵌套消息 round-trip 后仍与原始结构完全相等。
    let mut model = CheckpointsModel {
        Checkpoints: HashMap::new(),
        TaskCheckpoint: Some(task.clone()),
    };
    model.Checkpoints.insert("`db`.`t`".into(), table.clone());

    let bytes = model.Marshal().expect("marshal");
    assert!(!bytes.is_empty());
    // `Size()` 必须与真实编码长度一致，这是生成代码对上层调用方的基础承诺。
    // 上层通常会预分配缓冲区，因此这里一旦不相等，就会把问题扩散成截断写入或多余零值字节。
    assert_eq!(bytes.len(), model.Size());

    let mut decoded = CheckpointsModel::default();
    decoded.Unmarshal(&bytes).expect("unmarshal");
    // 直接做结构相等比较，能一次性确认 map、嵌套消息和可选字段都按 Go 语义回来了。
    assert_eq!(decoded, model);

    // Boundary: empty / zero-value messages omit all fields.
    // proto3 下全零值消息应编码为空字节串；这里验证 Rust 生成代码没有错误写出默认值字段。
    let empty = TaskCheckpointModel::default();
    let empty_bytes = empty.Marshal().unwrap();
    assert!(empty_bytes.is_empty());
    assert_eq!(empty.Size(), 0);
    // 对空切片执行 Unmarshal 时，Go gogo/protobuf 的语义是“保持已有字段不变”，这里要求 Rust 保持一致。
    let mut empty2 = TaskCheckpointModel {
        TaskId: 1,
        ..Default::default()
    };
    empty2.Unmarshal(&[]).unwrap();
    assert_eq!(empty2.TaskId, 1); // empty input leaves prior values (Go merge semantics on empty)

    // Actually Go Unmarshal on empty data keeps existing fields — verify reset+empty.
    // 如果接收方本来就是零值，那么空输入后仍应保持零值，避免把“保持已有值”误实现成“总是重置”。
    let mut cleared = TaskCheckpointModel::default();
    cleared.Unmarshal(&[]).unwrap();
    assert_eq!(cleared, TaskCheckpointModel::default());

    // Zigzag engine keys (negative/zero/positive) survive round-trip.
    // 单独拆一个更小的 table，是为了把断言焦点锁定在 map key 的编码方式，而不是其他字段干扰。
    let mut t2 = TableCheckpointModel::default();
    t2.Engines.insert(
        -1,
        EngineCheckpointModel {
            Status: 1,
            Chunks: HashMap::new(),
        },
    );
    t2.Engines.insert(
        0,
        EngineCheckpointModel {
            Status: 2,
            Chunks: HashMap::new(),
        },
    );
    t2.Engines.insert(
        1,
        EngineCheckpointModel {
            Status: 3,
            Chunks: HashMap::new(),
        },
    );
    let tb = t2.Marshal().unwrap();
    let mut t2b = TableCheckpointModel::default();
    t2b.Unmarshal(&tb).unwrap();
    assert_eq!(t2b.Engines.get(&-1).unwrap().Status, 1);
    assert_eq!(t2b.Engines.get(&0).unwrap().Status, 2);
    assert_eq!(t2b.Engines.get(&1).unwrap().Status, 3);

    // fixed64 / sfixed64 non-zero and packed repeated.
    // 这里再次直接 round-trip chunk，专门确认固定宽度整数和 packed repeated 列表没有被顶层结构掩盖问题。
    let cb = chunk.Marshal().unwrap();
    let mut chunk2 = ChunkCheckpointModel::default();
    chunk2.Unmarshal(&cb).unwrap();
    assert_eq!(chunk2, chunk);
    // 清空 repeated 字段后重新编码，验证“空列表省略”不会在解码端伪造出旧值或占位元素。
    chunk.ColumnPermutation.clear();
    let cb0 = chunk.Marshal().unwrap();
    let mut chunk3 = ChunkCheckpointModel::default();
    chunk3.Unmarshal(&cb0).unwrap();
    assert!(chunk3.ColumnPermutation.is_empty());

    // Error: truncated length-delimited field.
    // 截断合法消息前几个字节，要求解码失败；这里不把错误文本写死，是为了兼容 Go/Rust 在细节文案上的可接受差异。
    let good = task.Marshal().unwrap();
    assert!(good.len() > 4);
    let truncated = &good[..3];
    let mut bad = TaskCheckpointModel::default();
    let err = bad.Unmarshal(truncated).unwrap_err();
    assert!(
        matches!(err, Error::UnexpectedEof)
            || err.to_string().contains("EOF")
            || err.to_string().contains("wrong wireType")
            || err.to_string().contains("integer overflow")
            || err.to_string().contains("negative length"),
        "unexpected err: {err}"
    );

    // Error: illegal wire type via skip helper.
    // 直接调用 skip helper 构造非法 wire type，可绕开具体消息分支，单测公共跳过逻辑是否仍遵循生成器约定。
    let illegal = skipFileCheckpoints(&[0x07]); // tag field=0 wire=7 illegal after field check...
    // wire 7 with field 0: tag 0x07 = field 0 wire 7 — skip reads wire type 7
    assert!(matches!(
        illegal,
        Err(Error::IllegalWireType(7)) | Err(Error::IllegalTag { .. }) | Err(Error::UnexpectedEof)
    ));

    // Resource / descriptor identity (no leak of stub FD).
    // `Descriptor()` 返回的是嵌入式 gzip 描述符元数据；长度、魔数和索引稳定，才能与 Go 反射层保持同一入口。
    let (fd, idx) = model.Descriptor();
    assert_eq!(fd.len(), 939);
    assert_eq!(fd[0], 0x1f); // gzip magic
    assert_eq!(idx, vec![0]);
    // 版本常量与 `init()` 可调用性都属于 Go 生成接口遗留面，删除后会破坏上层依赖或 parity 假设。
    assert_eq!(PROTO_GOGO_PACKAGE_IS_VERSION_3, 3);
    init(); // no-op, must be callable

    // XXX_* compatibility surface used by Go callers.
    // `XXX_*` 是 gogo/protobuf 时代的兼容封装，哪怕新代码少用，也必须证明 Rust 侧没有把它们实现坏掉。
    let mut xxx = TaskCheckpointModel::default();
    xxx.XXX_Unmarshal(&good).unwrap();
    assert_eq!(xxx.TaskId, 42);
    assert_eq!(xxx.XXX_Size(), xxx.Size());
    // `deterministic = false` 也必须返回与普通 `Marshal` 一致的结果，避免旧调用方仅因入口不同就得到不同字节流。
    let remarl = xxx.XXX_Marshal(Vec::new(), false).unwrap();
    assert_eq!(remarl, good);

    // Unknown field skipped (reserved table field 4 style: synthetic unknown).
    // Append an unknown varint field 99 = tag (99<<3)|0 to a task message.
    // 末尾拼接未知字段后仍能正常解码，是 proto 向前兼容的基本要求；旧代码必须忽略新字段而不是拒绝整条消息。
    let mut with_unknown = good.clone();
    // field 99 wire 0 value 1
    // 这里本地写一个最小 varint 编码器，只服务测试造数，避免把断言依赖扩展到额外工具函数。
    fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            buf.push((v as u8) | 0x80);
            v >>= 7;
        }
        buf.push(v as u8);
    }
    put_varint(&mut with_unknown, (99 << 3) | 0);
    put_varint(&mut with_unknown, 1);
    let mut skipped = TaskCheckpointModel::default();
    skipped.Unmarshal(&with_unknown).unwrap();
    assert_eq!(skipped.TaskId, 42);
    assert_eq!(skipped.LightningVer, "v8.0.0");
}

#[test]
fn xxx_merge_preserves_destination_defaults_and_merges_nested_messages() {
    let mut destination = CheckpointsModel {
        Checkpoints: HashMap::from([(
            "existing".into(),
            TableCheckpointModel {
                Status: 10,
                KvBytes: 20,
                ..Default::default()
            },
        )]),
        TaskCheckpoint: Some(TaskCheckpointModel {
            TaskId: 7,
            SourceDir: "/destination".into(),
            ..Default::default()
        }),
    };
    let source = CheckpointsModel {
        Checkpoints: HashMap::from([(
            "added".into(),
            TableCheckpointModel {
                Status: 30,
                ..Default::default()
            },
        )]),
        TaskCheckpoint: Some(TaskCheckpointModel {
            Backend: "local".into(),
            ..Default::default()
        }),
    };

    destination.XXX_Merge(&source);

    assert_eq!(destination.Checkpoints["existing"].KvBytes, 20);
    assert_eq!(destination.Checkpoints["added"].Status, 30);
    let task = destination.TaskCheckpoint.unwrap();
    assert_eq!(task.TaskId, 7);
    assert_eq!(task.SourceDir, "/destination");
    assert_eq!(task.Backend, "local");

    let mut chunk = ChunkCheckpointModel {
        Path: "keep.csv".into(),
        Offset: 11,
        ColumnPermutation: vec![1, 2],
        ..Default::default()
    };
    chunk.XXX_Merge(&ChunkCheckpointModel {
        Offset: 0,
        ColumnPermutation: vec![3, 4],
        RealPos: 99,
        ..Default::default()
    });
    assert_eq!(chunk.Path, "keep.csv");
    assert_eq!(chunk.Offset, 11);
    assert_eq!(chunk.ColumnPermutation, vec![1, 2, 3, 4]);
    assert_eq!(chunk.RealPos, 99);
}

#[test]
fn marshal_to_and_sized_buffer_match_generated_buffer_placement() {
    let model = TaskCheckpointModel {
        TaskId: 42,
        Backend: "local".into(),
        ..Default::default()
    };
    let encoded = model.Marshal().unwrap();
    let padding = 5;

    let mut sized = vec![0xa5; encoded.len() + padding];
    let n = model.MarshalToSizedBuffer(&mut sized).unwrap();
    assert_eq!(n, encoded.len());
    assert_eq!(&sized[..padding], vec![0xa5; padding].as_slice());
    assert_eq!(&sized[padding..], encoded.as_slice());

    let mut direct = vec![0xa5; encoded.len() + padding];
    let n = model.MarshalTo(&mut direct).unwrap();
    assert_eq!(n, encoded.len());
    assert_eq!(&direct[..encoded.len()], encoded.as_slice());
    assert_eq!(&direct[encoded.len()..], vec![0xa5; padding].as_slice());
}
