// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Runtime implementation of the scalar behavior in `builtin_encryption.go`.
// The expression framework supplies NULL propagation and argument coercion;
// this module owns the deterministic builtin behavior and observable warnings.
//
// 加密与压缩类标量内置函数的运行时实现（对应 `builtin_encryption.go`）。
// 覆盖 AES 加解密、摘要（MD5/SHA/SM3）、PASSWORD、ENCODE/DECODE、
// COMPRESS/UNCOMPRESS、RANDOM_BYTES，以及密码强度校验。
// 表达式框架负责 NULL 传播与参数强制转换；本模块负责确定性计算与可观察警告。

use std::io::{Read, Write};

use encrypt::{aes as tidb_aes, crypt as sql_crypt};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use md5::Md5;
use parser_auth::parser::auth::{mysql_native_password::EncodePassword, tidb_sm3::Sm3Hash};
use rand::{RngCore, rngs::OsRng};
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use thiserror::Error;
use unicode_general_category::{GeneralCategory, get_general_category};

/// AES 块大小（字节）；IV 至少需要这么长。
pub const AES_BLOCK_SIZE: usize = 16;
/// SHA2 长度参数：0 表示默认 SHA-256。
pub const SHA0: i64 = 0;
/// SHA-224 位长度。
pub const SHA224: i64 = 224;
/// SHA-256 位长度。
pub const SHA256: i64 = 256;
/// SHA-384 位长度。
pub const SHA384: i64 = 384;
/// SHA-512 位长度。
pub const SHA512: i64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 加密/压缩相关的 SQL 警告（IV 被忽略、zlib 数据错误、PASSWORD 弃用等）。
pub enum EncryptionWarning {
    IvIgnored,
    ZlibData,
    ZlibBuffer,
    PasswordDeprecated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 带可选警告列表的求值结果；`value` 为 None 表示 SQL NULL。
pub struct Eval<T> {
    pub value: Option<T>,
    pub warnings: Vec<EncryptionWarning>,
}

impl<T> Eval<T> {
    /// 构造成功结果（无警告）。
    fn some(value: T) -> Self {
        Self {
            value: Some(value),
            warnings: Vec::new(),
        }
    }

    /// 构造带警告的 SQL NULL。
    fn null_with(warning: EncryptionWarning) -> Self {
        Self {
            value: None,
            warnings: vec![warning],
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
/// 语句级错误（不支持的模式、IV 过短、随机字节长度越界等）。
pub enum EncryptionError {
    #[error("unsupported block encryption mode - {0}")]
    UnsupportedMode(String),
    #[error("incorrect parameter count for {0}")]
    IncorrectParameterCount(&'static str),
    #[error(
        "The initialization vector supplied to {function} is too short. Must be at least 16 bytes long"
    )]
    IvTooShort { function: &'static str },
    #[error("length is out of range in random_bytes")]
    RandomBytesLength,
    #[error("fail to generate random bytes: {0}")]
    RandomBytes(String),
    #[error("zlib failure: {0}")]
    Zlib(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// AES 工作模式：ECB/CBC/OFB/CFB。
enum AesKind {
    Ecb,
    Cbc,
    Ofb,
    Cfb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 解析后的 AES 模式：密钥长度与是否需要 IV。
struct AesMode {
    kind: AesKind,
    key_size: usize,
    iv_required: bool,
}

/// 解析 `aes-128-cbc` 等形式的模式名。
fn aes_mode(name: &str) -> Result<AesMode, EncryptionError> {
    let normalized = name.to_ascii_lowercase();
    let (key_size, kind, iv_required) = match normalized.as_str() {
        "aes-128-ecb" => (16, AesKind::Ecb, false),
        "aes-192-ecb" => (24, AesKind::Ecb, false),
        "aes-256-ecb" => (32, AesKind::Ecb, false),
        "aes-128-cbc" => (16, AesKind::Cbc, true),
        "aes-192-cbc" => (24, AesKind::Cbc, true),
        "aes-256-cbc" => (32, AesKind::Cbc, true),
        "aes-128-ofb" => (16, AesKind::Ofb, true),
        "aes-192-ofb" => (24, AesKind::Ofb, true),
        "aes-256-ofb" => (32, AesKind::Ofb, true),
        "aes-128-cfb" => (16, AesKind::Cfb, true),
        "aes-192-cfb" => (24, AesKind::Cfb, true),
        "aes-256-cfb" => (32, AesKind::Cfb, true),
        _ => return Err(EncryptionError::UnsupportedMode(name.to_owned())),
    };
    Ok(AesMode {
        kind,
        key_size,
        iv_required,
    })
}

/// 校验 IV：不需要时忽略并告警；需要时检查存在且至少 16 字节，只取前 16 字节。
fn checked_iv<'a>(
    iv: Option<&'a [u8]>,
    mode: AesMode,
    function: &'static str,
) -> Result<(Option<&'a [u8]>, Vec<EncryptionWarning>), EncryptionError> {
    if !mode.iv_required {
        return Ok((
            None,
            iv.map(|_| vec![EncryptionWarning::IvIgnored])
                .unwrap_or_default(),
        ));
    }
    let iv = iv.ok_or(EncryptionError::IncorrectParameterCount(function))?;
    if iv.len() < AES_BLOCK_SIZE {
        return Err(EncryptionError::IvTooShort { function });
    }
    Ok((Some(&iv[..AES_BLOCK_SIZE]), Vec::new()))
}

/// AES 加密；库层失败转为 SQL NULL（与 TiDB 一致）。
pub fn aes_encrypt(
    input: &[u8],
    key: &[u8],
    mode_name: &str,
    iv: Option<&[u8]>,
) -> Result<Eval<Vec<u8>>, EncryptionError> {
    let mode = aes_mode(mode_name)?;
    let (iv, warnings) = checked_iv(iv, mode, "aes_encrypt")?;
    let key = tidb_aes::DeriveKeyMySQL(key, mode.key_size);
    let result = match mode.kind {
        AesKind::Ecb => tidb_aes::AESEncryptWithECB(input, &key),
        AesKind::Cbc => tidb_aes::AESEncryptWithCBC(input, &key, iv.unwrap()),
        AesKind::Ofb => tidb_aes::AESEncryptWithOFB(input, &key, iv.unwrap()),
        AesKind::Cfb => tidb_aes::AESEncryptWithCFB(input, &key, iv.unwrap()),
    };
    // TiDB 有意将加密库失败转换为 SQL NULL。
    // TiDB deliberately converts encryption-library failures to SQL NULL.
    Ok(Eval {
        value: result.ok(),
        warnings,
    })
}

/// AES 解密；密文非法时返回 NULL。
pub fn aes_decrypt(
    input: &[u8],
    key: &[u8],
    mode_name: &str,
    iv: Option<&[u8]>,
) -> Result<Eval<Vec<u8>>, EncryptionError> {
    let mode = aes_mode(mode_name)?;
    let (iv, warnings) = checked_iv(iv, mode, "aes_decrypt")?;
    let key = tidb_aes::DeriveKeyMySQL(key, mode.key_size);
    let result = match mode.kind {
        AesKind::Ecb => tidb_aes::AESDecryptWithECB(input, &key),
        AesKind::Cbc => tidb_aes::AESDecryptWithCBC(input, &key, iv.unwrap()),
        AesKind::Ofb => tidb_aes::AESDecryptWithOFB(input, &key, iv.unwrap()),
        AesKind::Cfb => tidb_aes::AESDecryptWithCFB(input, &key, iv.unwrap()),
    };
    Ok(Eval {
        value: result.ok(),
        warnings,
    })
}

/// MySQL DECODE：历史命名反直觉——此处把明文变成乱码表示（对应 Go SQLDecode）。
pub fn sql_decode(input: &[u8], password: &[u8]) -> Vec<u8> {
    sql_crypt::SQLDecode(input, password).unwrap()
}

/// MySQL ENCODE：把 DECODE 的乱码还原为原文。
pub fn sql_encode(input: &[u8], password: &[u8]) -> Vec<u8> {
    sql_crypt::SQLEncode(input, password).unwrap()
}

/// 弃用的 PASSWORD()：双次 SHA1 并带 `*` 前缀；产生 PasswordDeprecated 警告。
pub fn mysql_password(password: &str) -> Eval<String> {
    if password.is_empty() {
        return Eval::some(String::new());
    }
    Eval {
        value: Some(EncodePassword(password)),
        warnings: vec![EncryptionWarning::PasswordDeprecated],
    }
}

/// RANDOM_BYTES：长度须在 1..=1024。
pub fn random_bytes(length: i64) -> Result<Vec<u8>, EncryptionError> {
    if !(1..=1024).contains(&length) {
        return Err(EncryptionError::RandomBytesLength);
    }
    let mut result = vec![0; length as usize];
    OsRng
        .try_fill_bytes(&mut result)
        .map_err(|error| EncryptionError::RandomBytes(error.to_string()))?;
    Ok(result)
}

/// MD5 摘要，返回小写十六进制。
pub fn md5_hash(input: &[u8]) -> String {
    hex::encode(Md5::digest(input))
}

/// SHA1 摘要。
pub fn sha1_hash(input: &[u8]) -> String {
    hex::encode(Sha1::digest(input))
}

/// SHA2 摘要；非法长度返回 NULL（None）。
pub fn sha2_hash(input: &[u8], hash_length: i64) -> Option<String> {
    match hash_length {
        SHA0 | SHA256 => Some(hex::encode(Sha256::digest(input))),
        SHA224 => Some(hex::encode(Sha224::digest(input))),
        SHA384 => Some(hex::encode(Sha384::digest(input))),
        SHA512 => Some(hex::encode(Sha512::digest(input))),
        _ => None,
    }
}

/// 国密 SM3 摘要。
pub fn sm3_hash(input: &[u8]) -> String {
    hex::encode(Sm3Hash(input))
}

/// 按比特读取 DEFLATE 流，用于把 zlib 结尾规范化为 Go 字节布局。
struct DeflateBits<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl DeflateBits<'_> {
    /// 读取 count 个比特（LSB first）。
    fn read(&mut self, count: usize) -> Result<u16, EncryptionError> {
        if self.position + count > self.bytes.len() * 8 {
            return Err(EncryptionError::Zlib("truncated DEFLATE stream".into()));
        }
        let mut value = 0_u16;
        for offset in 0..count {
            let bit = (self.bytes[self.position / 8] >> (self.position % 8)) & 1;
            value |= u16::from(bit) << offset;
            self.position += 1;
        }
        Ok(value)
    }

    /// 对齐到下一字节边界。
    fn align_byte(&mut self) {
        self.position = (self.position + 7) & !7;
    }

    /// 跳过 count 个字节。
    fn skip_bytes(&mut self, count: usize) -> Result<(), EncryptionError> {
        let bits = count
            .checked_mul(8)
            .ok_or_else(|| EncryptionError::Zlib("DEFLATE length overflow".into()))?;
        if self.position + bits > self.bytes.len() * 8 {
            return Err(EncryptionError::Zlib("truncated DEFLATE block".into()));
        }
        self.position += bits;
        Ok(())
    }
}

#[derive(Clone)]
/// DEFLATE Huffman 码表（canonical）。
struct DeflateHuffman {
    entries: Vec<(u16, u8, u16)>,
    max_length: u8,
}

impl DeflateHuffman {
    /// 由码长数组构建 Huffman 表。
    fn new(lengths: &[u8]) -> Result<Self, EncryptionError> {
        let max_length = lengths.iter().copied().max().unwrap_or(0);
        if max_length == 0 || max_length > 15 {
            return Err(EncryptionError::Zlib("invalid DEFLATE Huffman tree".into()));
        }
        let mut counts = [0_u16; 16];
        for &length in lengths {
            if length > 0 {
                counts[length as usize] += 1;
            }
        }
        let mut next_code = [0_u16; 16];
        let mut code = 0_u16;
        for bits in 1..=15 {
            code = (code + counts[bits - 1]) << 1;
            next_code[bits] = code;
        }
        let mut entries = Vec::new();
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let canonical = next_code[length as usize];
            next_code[length as usize] += 1;
            let reversed = canonical.reverse_bits() >> (16 - length);
            entries.push((reversed, length, symbol as u16));
        }
        Ok(Self {
            entries,
            max_length,
        })
    }

    /// 从比特流解码一个符号。
    fn decode(&self, bits: &mut DeflateBits<'_>) -> Result<u16, EncryptionError> {
        let mut code = 0_u16;
        for length in 1..=self.max_length {
            code |= bits.read(1)? << (length - 1);
            if let Some((_, _, symbol)) = self
                .entries
                .iter()
                .find(|(entry, entry_length, _)| *entry_length == length && *entry == code)
            {
                return Ok(*symbol);
            }
        }
        Err(EncryptionError::Zlib("invalid DEFLATE Huffman code".into()))
    }
}

/// 固定 Huffman 字面/距离树。
fn fixed_deflate_trees() -> Result<(DeflateHuffman, DeflateHuffman), EncryptionError> {
    let mut literal_lengths = vec![0_u8; 288];
    literal_lengths[..144].fill(8);
    literal_lengths[144..256].fill(9);
    literal_lengths[256..280].fill(7);
    literal_lengths[280..].fill(8);
    Ok((
        DeflateHuffman::new(&literal_lengths)?,
        DeflateHuffman::new(&[5; 32])?,
    ))
}

/// 从比特流解析动态 Huffman 树。
fn dynamic_deflate_trees(
    bits: &mut DeflateBits<'_>,
) -> Result<(DeflateHuffman, DeflateHuffman), EncryptionError> {
    let literal_count = usize::from(bits.read(5)?) + 257;
    let distance_count = usize::from(bits.read(5)?) + 1;
    let code_length_count = usize::from(bits.read(4)?) + 4;
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut code_lengths = [0_u8; 19];
    for index in 0..code_length_count {
        code_lengths[ORDER[index]] = bits.read(3)? as u8;
    }
    let code_tree = DeflateHuffman::new(&code_lengths)?;
    let total = literal_count + distance_count;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        match code_tree.decode(bits)? {
            symbol @ 0..=15 => lengths.push(symbol as u8),
            16 => {
                let previous = *lengths
                    .last()
                    .ok_or_else(|| EncryptionError::Zlib("invalid DEFLATE repeat code".into()))?;
                let repeat = usize::from(bits.read(2)?) + 3;
                lengths.extend(std::iter::repeat_n(previous, repeat));
            }
            17 => {
                let repeat = usize::from(bits.read(3)?) + 3;
                lengths.extend(std::iter::repeat_n(0, repeat));
            }
            18 => {
                let repeat = usize::from(bits.read(7)?) + 11;
                lengths.extend(std::iter::repeat_n(0, repeat));
            }
            _ => return Err(EncryptionError::Zlib("invalid DEFLATE code length".into())),
        }
        if lengths.len() > total {
            return Err(EncryptionError::Zlib(
                "DEFLATE code lengths overflow".into(),
            ));
        }
    }
    let literal_tree = DeflateHuffman::new(&lengths[..literal_count])?;
    let distance_tree = DeflateHuffman::new(&lengths[literal_count..])?;
    Ok((literal_tree, distance_tree))
}

/// 扫描压缩块直到结束符，推进比特位置。
fn scan_compressed_block(
    bits: &mut DeflateBits<'_>,
    literal_tree: &DeflateHuffman,
    distance_tree: &DeflateHuffman,
) -> Result<(), EncryptionError> {
    loop {
        let symbol = literal_tree.decode(bits)?;
        match symbol {
            0..=255 => {}
            256 => return Ok(()),
            257..=285 => {
                let length_extra = match symbol {
                    257..=264 | 285 => 0,
                    265..=268 => 1,
                    269..=272 => 2,
                    273..=276 => 3,
                    277..=280 => 4,
                    281..=284 => 5,
                    _ => unreachable!(),
                };
                bits.read(length_extra)?;
                let distance = distance_tree.decode(bits)?;
                if distance > 29 {
                    return Err(EncryptionError::Zlib("invalid DEFLATE distance".into()));
                }
                let distance_extra = if distance < 4 {
                    0
                } else {
                    usize::from(distance / 2 - 1)
                };
                bits.read(distance_extra)?;
            }
            _ => return Err(EncryptionError::Zlib("invalid DEFLATE length".into())),
        }
    }
}

/// 定位最后一个 DEFLATE 块的头尾比特位置。
fn final_deflate_block(deflate: &[u8]) -> Result<(usize, usize), EncryptionError> {
    let mut bits = DeflateBits {
        bytes: deflate,
        position: 0,
    };
    loop {
        let header_position = bits.position;
        let is_final = bits.read(1)? != 0;
        match bits.read(2)? {
            0 => {
                bits.align_byte();
                let length = usize::from(bits.read(16)?);
                let inverse = bits.read(16)?;
                if inverse != !(length as u16) {
                    return Err(EncryptionError::Zlib("invalid DEFLATE stored block".into()));
                }
                bits.skip_bytes(length)?;
            }
            1 => {
                let (literal_tree, distance_tree) = fixed_deflate_trees()?;
                scan_compressed_block(&mut bits, &literal_tree, &distance_tree)?;
            }
            2 => {
                let (literal_tree, distance_tree) = dynamic_deflate_trees(&mut bits)?;
                scan_compressed_block(&mut bits, &literal_tree, &distance_tree)?;
            }
            _ => return Err(EncryptionError::Zlib("reserved DEFLATE block type".into())),
        }
        if is_final {
            return Ok((header_position, bits.position));
        }
    }
}

/// 向字节缓冲追加单个比特。
fn append_bit(bytes: &mut Vec<u8>, position: &mut usize, bit: bool) {
    if *position / 8 == bytes.len() {
        bytes.push(0);
    }
    if bit {
        bytes[*position / 8] |= 1 << (*position % 8);
    }
    *position += 1;
}

/// 把 flate2 输出改写成与 Go compress/flate 一致的结尾（BFINAL + 空 stored 块）。
fn normalize_go_zlib_end(stream: &mut Vec<u8>) -> Result<(), EncryptionError> {
    if stream.len() < 6 {
        return Err(EncryptionError::Zlib("truncated zlib stream".into()));
    }
    let trailer_offset = stream.len() - 4;
    let trailer = stream[trailer_offset..].to_vec();
    let (final_header, end_position) = final_deflate_block(&stream[2..trailer_offset])?;
    let final_header = final_header + 16;
    let end_position = end_position + 16;

    stream[final_header / 8] &= !(1 << (final_header % 8));
    stream.truncate(end_position.div_ceil(8));
    if end_position % 8 != 0 {
        let mask = (1_u8 << (end_position % 8)) - 1;
        *stream.last_mut().unwrap() &= mask;
    }
    let mut position = end_position;
    // 追加最终空 stored 块：BFINAL=1, BTYPE=00，再字节对齐。
    // Final empty stored block: BFINAL=1, BTYPE=00, then byte alignment.
    append_bit(stream, &mut position, true);
    append_bit(stream, &mut position, false);
    append_bit(stream, &mut position, false);
    while position % 8 != 0 {
        append_bit(stream, &mut position, false);
    }
    stream.extend_from_slice(&[0, 0, 0xff, 0xff]);
    stream.extend_from_slice(&trailer);
    Ok(())
}

/// MySQL COMPRESS：4 字节小端长度头 + zlib；末尾空格时再追加 '.'。
pub fn compress(input: &[u8]) -> Result<Eval<Vec<u8>>, EncryptionError> {
    if input.is_empty() {
        return Ok(Eval::some(Vec::new()));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|error| EncryptionError::Zlib(error.to_string()))?;
    let mut compressed = encoder
        .finish()
        .map_err(|error| EncryptionError::Zlib(error.to_string()))?;
    // Go 通过清除当前块 BFINAL 并追加最终空 stored 块结束；需保持字节级一致，
    // 因为 COMPRESS() 输出可被观测且有 Go 向量测试。
    // Go's compress/flate closes by clearing the current block's BFINAL bit and
    // appending a final empty stored block. Preserve that exact byte-level result
    // because MySQL COMPRESS() output is observable and covered by Go vectors.
    normalize_go_zlib_end(&mut compressed)?;
    let append_suffix = compressed.last() == Some(&b' ');
    let mut output = Vec::with_capacity(4 + compressed.len() + usize::from(append_suffix));
    output.extend_from_slice(&(input.len() as u32).to_le_bytes());
    output.extend_from_slice(&compressed);
    if append_suffix {
        output.push(b'.');
    }
    Ok(Eval::some(output))
}

/// UNCOMPRESS：校验长度头与 zlib；损坏数据返回带 ZlibData/ZlibBuffer 警告的 NULL。
pub fn uncompress(payload: &[u8]) -> Eval<Vec<u8>> {
    if payload.is_empty() {
        return Eval::some(Vec::new());
    }
    if payload.len() <= 4 {
        return Eval::null_with(EncryptionWarning::ZlibData);
    }
    let declared = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
    let mut decoder = ZlibDecoder::new(&payload[4..]);
    let mut output = Vec::new();
    if decoder.read_to_end(&mut output).is_err() {
        return Eval::null_with(EncryptionWarning::ZlibData);
    }
    if declared < output.len() {
        return Eval::null_with(EncryptionWarning::ZlibBuffer);
    }
    Eval::some(output)
}

/// UNCOMPRESSED_LENGTH：只读长度头；过短载荷返回 0 并告警。
pub fn uncompressed_length(payload: &[u8]) -> Eval<i64> {
    if payload.is_empty() {
        return Eval::some(0);
    }
    if payload.len() <= 4 {
        return Eval {
            value: Some(0),
            warnings: vec![EncryptionWarning::ZlibData],
        };
    }
    Eval::some(u32::from_le_bytes(payload[..4].try_into().unwrap()) as i64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// VALIDATE_PASSWORD_STRENGTH 使用的策略参数。
pub struct PasswordPolicy {
    pub enabled: bool,
    pub check_username: bool,
    pub username: Option<String>,
    pub auth_username: Option<String>,
    pub minimum_length: usize,
    pub mixed_case_count: usize,
    pub number_count: usize,
    pub special_char_count: usize,
    pub dictionary: Vec<String>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            check_username: true,
            username: None,
            auth_username: None,
            minimum_length: 8,
            mixed_case_count: 1,
            number_count: 1,
            special_char_count: 1,
            dictionary: Vec::new(),
        }
    }
}

/// 密码强度评分：0/25/50/75/100；策略关闭或过短返回 0；NULL 输入返回 None。
pub fn validate_password_strength(input: Option<&str>, policy: &PasswordPolicy) -> Option<i64> {
    let password = input?;
    if password.chars().count() < 4 || !policy.enabled {
        return Some(0);
    }

    // 用户名或其反转若出现在密码中，强度直接为 0。
    if policy.check_username {
        for username in [policy.auth_username.as_deref(), policy.username.as_deref()]
            .into_iter()
            .flatten()
        {
            if username.is_empty() {
                continue;
            }
            let reversed: Vec<u8> = username.as_bytes().iter().rev().copied().collect();
            if password
                .as_bytes()
                .windows(username.len())
                .any(|part| part == username.as_bytes())
                || password
                    .as_bytes()
                    .windows(reversed.len())
                    .any(|part| part == reversed)
            {
                return Some(0);
            }
        }
    }

    if password.chars().count() < policy.minimum_length {
        return Some(25);
    }

    let (mut lower, mut upper, mut number, mut special) = (0, 0, 0, 0);
    for character in password.chars() {
        match get_general_category(character) {
            GeneralCategory::UppercaseLetter => upper += 1,
            GeneralCategory::LowercaseLetter => lower += 1,
            GeneralCategory::DecimalNumber => number += 1,
            _ => special += 1,
        }
    }
    if lower < policy.mixed_case_count
        || upper < policy.mixed_case_count
        || number < policy.number_count
        || special < policy.special_char_count
    {
        return Some(50);
    }

    let normalized = password.to_lowercase();
    if policy
        .dictionary
        .iter()
        .any(|word| (4..=100).contains(&word.len()) && normalized.contains(&word.to_lowercase()))
    {
        return Some(75);
    }
    Some(100)
}
