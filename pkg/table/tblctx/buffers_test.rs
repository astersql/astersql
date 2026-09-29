// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `buffers` 模块单测：行编码写入、容量保留、检查缓冲与 ensureCapacityAndReset。

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::tablecodec::mysql;

/// 记录写入的 MemBuffer 桩。
#[derive(Default)]
struct MockMemBuffer {
    writes: Vec<(kv::Key, Vec<u8>, Vec<kv::FlagsOp>)>,
}

impl kv::MemBuffer for MockMemBuffer {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), errors::SharedError> {
        self.writes.push((key, value, Vec::new()));
        Ok(())
    }

    fn SetWithFlags(
        &mut self,
        key: kv::Key,
        value: Vec<u8>,
        flags: &[kv::FlagsOp],
    ) -> Result<(), errors::SharedError> {
        self.writes.push((key, value, flags.to_vec()));
        Ok(())
    }
}

/// 仅暴露 MutateBuffers 的最小突变上下文桩。
struct MockMutateCtx {
    buffers: MutateBuffers,
}

impl MockMutateCtx {
    fn GetMutateBuffers(&mut self) -> &mut MutateBuffers {
        &mut self.buffers
    }
}

/// 构造共享 WriteStmtBufs 与 MockMutateCtx。
fn newMockMutateCtx() -> (Rc<RefCell<variable::WriteStmtBufs>>, MockMutateCtx) {
    let buffers = NewMutateBuffers(variable::WriteStmtBufs::default());
    let stmt_bufs = Rc::clone(&buffers.stmtBufs);
    (stmt_bufs, MockMutateCtx { buffers })
}

/// 编码路径用例参数：时区、校验和、新旧格式与写入标志。
struct EncodeCase {
    loc: time::Location,
    row_level_checksum: bool,
    old_format: bool,
    flags: Vec<kv::FlagsOp>,
}

/// 覆盖 WriteMemBufferEncoded / EncodeBinlogRowData 在多种配置下的编码结果。
#[test]
fn TestEncodeRow() {
    let (stmt_bufs, mut ctx) = newMockMutateCtx();
    let tm = types::NewTime(
        types::FromDate(2021, 1, 1, 1, 2, 3, 4),
        mysql::TypeTimestamp,
        6,
    );
    let d1 = types::NewBytesDatum(vec![1, 2, 3]);
    let d2 = types::NewIntDatum(20);
    let d3 = types::NewTimeDatum(tm);
    let buffer = ctx.GetMutateBuffers().GetEncodeRowBufferWithCap(3);

    assert!(Rc::ptr_eq(&stmt_bufs, &buffer.writeStmtBufs));
    buffer.AddColVal(1, d1.clone());
    buffer.AddColVal(2, d2.clone());
    buffer.AddColVal(3, d3.clone());
    assert_eq!(buffer.colIDs, vec![1, 2, 3]);
    assert_eq!(buffer.row.len(), 3);
    assert_eq!(buffer.row[0].GetBytes(), d1.GetBytes());
    assert_eq!(buffer.row[1].GetInt64(), d2.GetInt64());
    assert_eq!(
        buffer.row[2].GetMysqlTime().String(),
        d3.GetMysqlTime().String()
    );

    let cases = [
        EncodeCase {
            loc: time::UTC,
            row_level_checksum: false,
            old_format: false,
            flags: vec![],
        },
        EncodeCase {
            loc: "Etc/GMT-1".parse().unwrap(),
            row_level_checksum: true,
            old_format: false,
            flags: vec![kv::FlagsOp::SetPresumeKeyNotExists],
        },
        EncodeCase {
            loc: "Etc/GMT-2".parse().unwrap(),
            row_level_checksum: false,
            old_format: true,
            flags: vec![],
        },
    ];

    for case in cases {
        // 直接调用 tablecodec 得到期望编码，与缓冲路径对比。
        let checksum = case.row_level_checksum.then(|| {
            Box::new(rowcodec::RawChecksum {
                Handle: Box::new(kv::IntHandle(1)),
            }) as Box<dyn rowcodec::Checksum>
        });
        let expected_val = tablecodec::EncodeRow(
            Some(case.loc),
            vec![d1.clone(), d2.clone(), d3.clone()],
            vec![1, 2, 3],
            Vec::new(),
            None,
            checksum,
            rowcodec::Encoder::new(!case.old_format),
        )
        .unwrap();

        let mut mem_buffer = MockMemBuffer::default();
        buffer
            .WriteMemBufferEncoded(
                codec::NewEncoder(collate::NewCollationEnabled()),
                RowEncodingConfig {
                    RowEncoder: Some(rowcodec::Encoder::new(!case.old_format)),
                    IsRowLevelChecksumEnabled: case.row_level_checksum,
                },
                Some(case.loc),
                (*errctx::StrictNoWarningContext).clone(),
                &mut mem_buffer,
                kv::Key(b"key1".to_vec()),
                Box::new(kv::IntHandle(1)),
                &case.flags,
            )
            .unwrap();

        assert_eq!(mem_buffer.writes.len(), 1);
        assert_eq!(mem_buffer.writes[0].0, kv::Key(b"key1".to_vec()));
        assert_eq!(mem_buffer.writes[0].1, expected_val);
        assert_eq!(mem_buffer.writes[0].2, case.flags);
        assert_eq!(buffer.writeStmtBufs.borrow().RowValBuf, expected_val);

        // binlog 使用旧行格式，且不得与 RowValBuf/IndexKeyBuf 共享底层缓冲。
        let expected_binlog = tablecodec::EncodeOldRow(
            Some(case.loc),
            vec![d1.clone(), d2.clone(), d3.clone()],
            vec![1, 2, 3],
            Vec::new(),
            None,
        )
        .unwrap();
        let encoded = buffer
            .EncodeBinlogRowData(Some(case.loc), (*errctx::StrictNoWarningContext).clone())
            .unwrap();
        assert_eq!(encoded, expected_binlog);

        let stmt_bufs = buffer.writeStmtBufs.borrow();
        assert_ne!(encoded.as_ptr(), stmt_bufs.RowValBuf.as_ptr());
        assert_ne!(encoded.as_ptr(), stmt_bufs.IndexKeyBuf.as_ptr());
    }
}

/// 验证 Reset/编码后容量不被收缩，且仍共享同一 WriteStmtBufs。
#[test]
fn TestEncodeBufferReserve() {
    let (stmt_bufs, mut ctx) = newMockMutateCtx();
    let encode_row_ptr = &ctx.buffers.encodeRow as *const EncodeRowBuffer;
    let buffer = ctx.GetMutateBuffers().GetEncodeRowBufferWithCap(6);

    assert_eq!(encode_row_ptr, buffer as *const EncodeRowBuffer);
    assert!(Rc::ptr_eq(&stmt_bufs, &buffer.writeStmtBufs));
    assert_eq!((buffer.colIDs.len(), buffer.colIDs.capacity()), (0, 6));
    assert_eq!((buffer.row.len(), buffer.row.capacity()), (0, 6));

    buffer.AddColVal(1, types::NewIntDatum(1));
    buffer.AddColVal(2, types::NewIntDatum(2));
    assert_eq!(buffer.colIDs.len(), 2);
    assert_eq!(buffer.row.len(), 2);

    let mut mem_buffer = MockMemBuffer::default();
    buffer
        .WriteMemBufferEncoded(
            codec::NewEncoder(collate::NewCollationEnabled()),
            RowEncodingConfig {
                RowEncoder: Some(rowcodec::Encoder::new(true)),
                IsRowLevelChecksumEnabled: false,
            },
            Some(time::UTC),
            (*errctx::StrictNoWarningContext).clone(),
            &mut mem_buffer,
            kv::Key(b"key1".to_vec()),
            Box::new(kv::IntHandle(1)),
            &[],
        )
        .unwrap();
    assert_eq!(mem_buffer.writes.len(), 1);

    let (encoded_cap, add_row_values_cap) = {
        let stmt_bufs = buffer.writeStmtBufs.borrow();
        assert!(!stmt_bufs.RowValBuf.is_empty());
        assert_eq!(stmt_bufs.AddRowValues.len(), 4);
        (
            stmt_bufs.RowValBuf.capacity(),
            stmt_bufs.AddRowValues.capacity(),
        )
    };

    // Reset 到更小 size 后，原 capacity 应保留。
    buffer.Reset(2);
    assert_eq!((buffer.colIDs.len(), buffer.colIDs.capacity()), (0, 6));
    assert_eq!((buffer.row.len(), buffer.row.capacity()), (0, 6));
    let stmt_bufs = buffer.writeStmtBufs.borrow();
    assert_eq!(stmt_bufs.AddRowValues.capacity(), add_row_values_cap);
    assert_eq!(stmt_bufs.RowValBuf.capacity(), encoded_cap);
}

/// 覆盖 CheckRowBuffer 的追加、行视图读取与容量保留 Reset。
#[test]
fn TestCheckRowBuffer() {
    let mut buffer = CheckRowBuffer::default();
    buffer.Reset(6);
    assert_eq!(
        (buffer.rowToCheck.len(), buffer.rowToCheck.capacity()),
        (0, 6)
    );

    let d1 = types::NewIntDatum(1);
    let d2 = types::NewIntDatum(2);
    buffer.AddColVal(d1.clone());
    buffer.AddColVal(d2.clone());
    assert_eq!(buffer.rowToCheck.len(), 2);
    assert_eq!(buffer.rowToCheck[0].GetInt64(), d1.GetInt64());
    assert_eq!(buffer.rowToCheck[1].GetInt64(), d2.GetInt64());

    let row_to_check = buffer.GetRowToCheck();
    assert_eq!(row_to_check.Len(), 2);
    assert_eq!(row_to_check.GetInt64(0), 1);
    assert_eq!(row_to_check.GetInt64(1), 2);

    buffer.Reset(2);
    assert_eq!(
        (buffer.rowToCheck.len(), buffer.rowToCheck.capacity()),
        (0, 6)
    );
}

/// 验证 getter 返回同一缓冲实例并共享 WriteStmtBufs。
#[test]
fn TestMutateBuffersGetter() {
    let (stmt_bufs, mut ctx) = newMockMutateCtx();
    let mut buffers = std::mem::replace(
        &mut ctx.buffers,
        NewMutateBuffers(variable::WriteStmtBufs::default()),
    );

    {
        let add = buffers.GetEncodeRowBufferWithCap(6);
        assert_eq!(add.row.capacity(), 6);
        assert!(Rc::ptr_eq(&stmt_bufs, &add.writeStmtBufs));
    }

    let update = buffers.GetCheckRowBufferWithCap(6);
    assert_eq!(update.rowToCheck.capacity(), 6);

    let expected_borrow = stmt_bufs.borrow();
    let expected_ptr = &*expected_borrow as *const variable::WriteStmtBufs;
    drop(expected_borrow);
    let actual = buffers.GetWriteStmtBufs();
    assert_eq!(expected_ptr, &*actual as *const variable::WriteStmtBufs);
}

/// 对齐 Go 语义：长度/容量在原地复用与重新分配两种路径下的行为。
#[test]
fn TestEnsureCapacityAndReset() {
    let empty = ensureCapacityAndReset(Vec::<i32>::new(), 0, &[]);
    assert!(empty.is_empty());
    assert_eq!(empty.capacity(), 0);

    let input = vec![1, 2, 3];
    let input_ptr = input.as_ptr();
    let slice = ensureCapacityAndReset(input, 0, &[]);
    assert_eq!((slice.len(), slice.capacity()), (0, 3));
    assert_eq!(slice.as_ptr(), input_ptr);

    let input = vec![1, 2, 3];
    let input_ptr = input.as_ptr();
    let mut slice = ensureCapacityAndReset(input, 2, &[]);
    assert_eq!((slice.len(), slice.capacity()), (2, 3));
    assert_eq!(slice.as_ptr(), input_ptr);
    slice[1] = 5;
    assert_eq!(slice, vec![1, 5]);

    let slice = ensureCapacityAndReset(vec![1, 2, 3], 4, &[]);
    assert_eq!((slice.len(), slice.capacity()), (4, 4));

    let input = vec![1, 2, 3];
    let input_ptr = input.as_ptr();
    let mut slice = ensureCapacityAndReset(input, 1, &[2]);
    assert_eq!((slice.len(), slice.capacity()), (1, 3));
    assert_eq!(slice.as_ptr(), input_ptr);
    slice[0] = 10;
    assert_eq!(slice, vec![10]);

    let slice = ensureCapacityAndReset(vec![1, 2, 3], 2, &[4]);
    assert_eq!((slice.len(), slice.capacity()), (2, 4));

    let slice = ensureCapacityAndReset(vec![1, 2, 3], 4, &[5]);
    assert_eq!((slice.len(), slice.capacity()), (4, 5));

    // Go only uses optCap when deciding whether to allocate. If the existing
    // backing array already fits size, optCap may be smaller than size.
    let input = vec![1, 2, 3];
    let input_ptr = input.as_ptr();
    let slice = ensureCapacityAndReset(input, 3, &[2]);
    assert_eq!((slice.len(), slice.capacity()), (3, 3));
    assert_eq!(slice.as_ptr(), input_ptr);
}
