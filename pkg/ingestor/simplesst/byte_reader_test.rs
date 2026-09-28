// Copyright 2026 AsterSQL.

// `ByteReader` 偏移、并发模式切换与 EOF 行为的单元测试。
//
// 校验顺序读、启用并发读、切回顺序模式后位置连续，以及 EOF/非法长度/已关闭错误。

/// 从偏移 3 起读，经并发模式开关后位置与错误语义保持正确。
#[test]
fn canonical_byte_reader_preserves_offsets_mode_switches_and_eof() {
    use crate::{ByteReader, Error};

    let mut reader = ByteReader::new((0_u8..16).collect(), 3, 2).unwrap();
    assert_eq!(reader.read_n_bytes(3).unwrap(), vec![3, 4, 5]);
    // 启用并发读配置并切入并发模式
    reader.enable_concurrent_read(3, 2).unwrap();
    reader.switch_concurrent_mode(true).unwrap();
    assert_eq!(reader.read_n_bytes(5).unwrap(), vec![6, 7, 8, 9, 10]);
    // 切回顺序模式后逻辑位置应仍为 11
    reader.switch_concurrent_mode(false).unwrap();
    assert_eq!(reader.position(), 11);
    assert!(matches!(reader.read_n_bytes(6), Err(error) if error.is_unexpected_eof()));
    assert!(matches!(reader.read_n_bytes(0), Err(Error::InvalidData(_))));
    reader.close().unwrap();
    assert!(matches!(reader.read_n_bytes(1), Err(Error::Closed)));
}

/// 对照 Go `TestByteReaderAuxBuf`：跨多个底层小缓冲读取时返回内容必须连续。
#[test]
fn test_byte_reader_aux_buf() {
    use crate::ByteReader;

    let mut reader = ByteReader::new(b"0123456789".to_vec(), 0, 1).unwrap();
    assert_eq!(reader.read_n_bytes(1).unwrap(), b"0");
    assert_eq!(reader.read_n_bytes(2).unwrap(), b"12");
    assert_eq!(reader.read_n_bytes(1).unwrap(), b"3");
    assert_eq!(reader.read_n_bytes(2).unwrap(), b"45");
}

/// 对照 Go `TestUnexpectedEOF`：已有部分数据但不足请求长度时必须报告截断，而非正常 EOF。
#[test]
fn test_unexpected_eof() {
    use crate::ByteReader;

    let mut reader = ByteReader::new(b"0123456789".to_vec(), 0, 3).unwrap();
    let error = reader.read_n_bytes(100).unwrap_err();
    assert!(
        error.to_string().contains("unexpected"),
        "partial read must be unexpected EOF, got {error}"
    );
}

/// 对照 Go `TestEmptyContent`：构造阶段首次预读空对象应直接返回正常 EOF。
#[test]
fn test_empty_content() {
    use crate::ByteReader;

    let error = ByteReader::new(Vec::new(), 0, 100).unwrap_err();
    assert!(error.is_eof());
    assert!(!error.to_string().contains("unexpected"));
}

/// 对照 Go `TestSwitchMode`：频繁开关并发读取不能跳过或重复字节。
#[test]
fn test_switch_mode() {
    use crate::ByteReader;

    let data = (0_u8..=200).collect::<Vec<_>>();
    let mut reader = ByteReader::new(data.clone(), 0, 7).unwrap();
    reader.enable_concurrent_read(4, 5).unwrap();
    let mut output = Vec::new();
    for (index, chunk) in data.chunks(3).enumerate() {
        reader.switch_concurrent_mode(index % 2 == 0).unwrap();
        output.extend(reader.read_n_bytes(chunk.len()).unwrap());
    }
    assert_eq!(output, data);
    assert!(reader.read_n_bytes(1).unwrap_err().is_eof());
}
