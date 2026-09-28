// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// encrypt 迁移回归测试：AES 模式向量、PKCS7、CTR 层与 SQL 加解密。
//
// 用 Go 侧固定密文十六进制与明文往返，校验 ECB/CBC/OFB/CTR/CFB、
// 密钥派生（DeriveKeyMySQL）、分块 CTR Writer/Reader 偏移，以及 SQLDecode/SQLEncode
//（MySQL 风格旧版加密，常用于备份/导入等场景）对 UTF-8 明文的往返。

use super::aes::*;
use super::aes_layer::*;
use super::crypt::*;
use std::sync::{Arc, Mutex};

/// 将字节切片格式化为大写十六进制，便于与 Go 测试向量比对。
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// 校验各 AES 工作模式密文与 Go 向量一致，并可解密还原。
#[test]
fn migration_aes_modes_match_go_vectors() {
    let key = b"1234567890123456";
    let iv = b"1234567890123456";

    // ECB：无 IV，块模式加密。
    let ecb = AESEncryptWithECB(b"pingcap", key).unwrap();
    assert_eq!(hex(&ecb), "697BFE9B3F8C2F289DD82C88C7BC95C4");
    assert_eq!(AESDecryptWithECB(&ecb, key).unwrap(), b"pingcap");

    // CBC：链式块加密，密文依赖 IV。
    let cbc = AESEncryptWithCBC(b"pingcap", key, iv).unwrap();
    assert_eq!(hex(&cbc), "2ECA0077C5EA5768A0485AA522774792");
    assert_eq!(AESDecryptWithCBC(&cbc, key, iv).unwrap(), b"pingcap");

    // OFB/CTR/CFB：流式模式对短明文产出相同长度密文向量。
    for (encrypt, decrypt) in [
        (
            AESEncryptWithOFB as fn(&[u8], &[u8], &[u8]) -> _,
            AESDecryptWithOFB as fn(&[u8], &[u8], &[u8]) -> _,
        ),
        (AESEncryptWithCTR, AESDecryptWithCTR),
        (AESEncryptWithCFB, AESDecryptWithCFB),
    ] {
        let ciphertext = encrypt(b"pingcap", key, iv).unwrap();
        assert_eq!(hex(&ciphertext), "0515A36BBF3DE0");
        assert_eq!(decrypt(&ciphertext, key, iv).unwrap(), b"pingcap");
    }
}

/// 校验 PKCS7 填充/去填充、非法密钥/密文错误，以及 MySQL 风格密钥派生。
#[test]
fn migration_padding_key_derivation_and_errors_match_go() {
    assert_eq!(PKCS7Pad(b"1234567890123456", 16).unwrap().len(), 32);
    assert_eq!(PKCS7Unpad(b"hello\x03\x03\x03", 8).unwrap(), b"hello");
    assert_eq!(
        PKCS7Unpad(b"hello\x02\x03\x03", 8).unwrap_err().to_string(),
        "Invalid padding"
    );
    assert!(AESEncryptWithECB(b"pingcap", b"invalid-key-size!").is_err());
    assert!(AESDecryptWithECB(b"short", b"1234567890123456").is_err());
    assert_eq!(
        hex(&DeriveKeyMySQL(b"MySecretVeryLooooongPassword", 16)),
        "22163D0233131607210A001D4C6F6F6F"
    );
}

/// 内存中的可写可读字节缓冲，同时实现 WriteCloser 与 ReaderAt，供 CTR 层测试。
#[derive(Clone, Default)]
struct SharedBytes(Arc<Mutex<Vec<u8>>>);

impl WriteCloser for SharedBytes {
    fn write(&mut self, p: &[u8]) -> (usize, Option<String>) {
        self.0.lock().unwrap().extend_from_slice(p);
        (p.len(), None)
    }
    fn close(&mut self) -> Option<String> {
        None
    }
}

impl ReaderAt for SharedBytes {
    fn read_at(&self, p: &mut [u8], off: u64) -> (usize, Option<String>) {
        let data = self.0.lock().unwrap();
        let off = off as usize;
        if off >= data.len() {
            return (0, Some("EOF".to_owned()));
        }
        let n = p.len().min(data.len() - off);
        p[..n].copy_from_slice(&data[off..off + n]);
        (n, (n < p.len()).then(|| "EOF".to_owned()))
    }
}

/// 校验 CTR 加密层：非法块大小、写缓冲与按偏移解密读取。
#[test]
fn migration_ctr_layer_buffers_and_reads_at_go_offsets() {
    assert_eq!(
        NewCtrCipherWithBlockSize(17).unwrap_err().to_string(),
        "invalid encrypt block size"
    );
    let cipher = NewCtrCipherWithBlockSize(32).unwrap();
    let storage = SharedBytes::default();
    let mut writer = NewWriter(storage.clone(), &cipher);
    assert_eq!(writer.Write(b"0123456789"), (10, None));
    assert_eq!(writer.Buffered(), 10);
    assert_eq!(writer.GetCache(), b"0123456789");
    assert_eq!(writer.GetCacheDataOffset(), 0);
    // 再写满一块以上数据后 Close，触发加密落盘。
    assert_eq!(writer.Write(&vec![b'x'; 40]), (40, None));
    assert_eq!(writer.Close(), None);

    let reader = NewReader(storage, cipher);
    let mut out = [0_u8; 15];
    assert_eq!(reader.ReadAt(&mut out, 5), (15, None));
    assert_eq!(&out, b"56789xxxxxxxxxx");

    let mut tail = [0_u8; 10];
    assert_eq!(reader.ReadAt(&mut tail, 45), (5, Some("EOF".to_owned())));
    assert_eq!(&tail[..5], b"xxxxx");
}

/// 底层写入同时返回部分字节数和错误时，Flush 仍须累计已落盘字节。
struct PartialErrorWriter;

impl WriteCloser for PartialErrorWriter {
    fn write(&mut self, _p: &[u8]) -> (usize, Option<String>) {
        (3, Some("disk full".to_owned()))
    }

    fn close(&mut self) -> Option<String> {
        panic!("Close must not run after Flush fails")
    }
}

#[test]
fn migration_ctr_layer_preserves_partial_write_count_with_error() {
    let cipher = NewCtrCipherWithBlockSize(32).unwrap();
    let mut writer = NewWriter(PartialErrorWriter, &cipher);

    assert_eq!(writer.Write(b"abcdef"), (6, None));
    assert_eq!(writer.Flush(), Some("disk full".to_owned()));
    assert_eq!(writer.GetCacheDataOffset(), 3);
    assert_eq!(writer.Write(b"x"), (0, Some("disk full".to_owned())));
    assert_eq!(writer.Close(), Some("disk full".to_owned()));
}

/// 校验 SQLDecode/SQLEncode 与 Go 二进制向量一致，并支持 UTF-8 明文往返。
#[test]
fn migration_sql_crypt_matches_go_binary_vectors_and_round_trips_utf8() {
    let decoded = SQLDecode(b"pingcap", b"1234567890123456").unwrap();
    assert_eq!(hex(&decoded), "2C35B5A4ADF391");
    assert_eq!(
        SQLEncode(&decoded, b"1234567890123456").unwrap(),
        b"pingcap"
    );

    let input = "分布式データベース".as_bytes();
    let decoded = SQLDecode(input, "pass1234@#$%%^^&".as_bytes()).unwrap();
    assert_eq!(
        hex(&decoded),
        "80CADC8D328B3026D04FB285F36FED04BBCA0CC685BF78B1E687CE"
    );
    assert_eq!(
        SQLEncode(&decoded, "pass1234@#$%%^^&".as_bytes()).unwrap(),
        input
    );
}
