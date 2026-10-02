// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Vector implementations for TiDB encryption and compression builtins.
//
// SQL NULL is represented independently from the byte payload so binary data is never
// interpreted as UTF-8.  Functions process rows in source order and preserve the Go
// implementation's distinction between statement errors, row-local crypto failures, and
// warnings.
//
// 加密与压缩内置函数的向量化实现。
// SQL NULL 与字节载荷分离表示，避免把二进制当 UTF-8。
// 按行处理并区分语句错误、行级加密失败与警告（与 Go 一致）。

use aes::{Aes128, Aes192, Aes256};
use cipher::block_padding::Pkcs7;
use cipher::generic_array::GenericArray;
use cipher::{
    AsyncStreamCipher, BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit,
    KeyIvInit, StreamCipher,
};
use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use md5::Md5;
use parser_auth::parser::auth::tidb_sm3::Sm3Hash;
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use std::io::Write;
use thiserror::Error;
use unicode_general_category::{GeneralCategory, get_general_category};

/// 字节列：每行 Option<Vec<u8>>，None 表示 SQL NULL。
pub type ByteColumn = Vec<Option<Vec<u8>>>;
/// 整数列。
pub type IntColumn = Vec<Option<i64>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 向量化路径上的警告种类。
pub enum Warning {
    IvIgnored,
    PasswordDeprecated,
    ZlibData,
    ZlibBuffer,
}

#[derive(Default, Debug)]
/// 求值上下文：累积警告。
pub struct EvalContext {
    warnings: Vec<Warning>,
}

impl EvalContext {
    /// 当前已累积的警告。
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// 清空警告列表。
    pub fn clear_warnings(&mut self) {
        self.warnings.clear();
    }

    /// 追加一条警告。
    fn warn(&mut self, warning: Warning) {
        self.warnings.push(warning);
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
/// 语句级向量化错误（列长不匹配、模式不支持、IV 缺失/过短等）。
pub enum EvalError {
    #[error("column length mismatch: expected {expected}, got {actual}")]
    ColumnLengthMismatch { expected: usize, actual: usize },
    #[error("unsupported block encryption mode - {0}")]
    UnsupportedBlockMode(String),
    #[error("AES initialization vector is required for {0}")]
    MissingIv(&'static str),
    #[error(
        "the initialization vector supplied to {function} is too short; it must be at least 16 bytes long"
    )]
    ShortIv { function: &'static str },
    #[error("data out of range for random_bytes: length must be between 1 and 1024")]
    RandomBytesLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// AES 块/流模式。
enum BlockMode {
    Ecb,
    Cbc,
    Ofb,
    Cfb,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 解析后的 AES 模式（密钥字节数 + 工作模式）。
pub struct AesMode {
    key_size: usize,
    block_mode: BlockMode,
}

impl AesMode {
    /// 解析 `aes-128-ecb` 形式的模式字符串。
    pub fn parse(value: &str) -> Result<Self, EvalError> {
        let mut parts = value.split('-');
        let (Some("aes"), Some(bits), Some(mode), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(EvalError::UnsupportedBlockMode(value.to_owned()));
        };
        let key_size = match bits {
            "128" => 16,
            "192" => 24,
            "256" => 32,
            _ => return Err(EvalError::UnsupportedBlockMode(value.to_owned())),
        };
        let block_mode = match mode {
            "ecb" => BlockMode::Ecb,
            "cbc" => BlockMode::Cbc,
            "ofb" => BlockMode::Ofb,
            "cfb" => BlockMode::Cfb,
            _ => return Err(EvalError::UnsupportedBlockMode(value.to_owned())),
        };
        Ok(Self {
            key_size,
            block_mode,
        })
    }

    /// 密钥字节长度。
    pub fn key_size(self) -> usize {
        self.key_size
    }

    /// 工作模式短名（ecb/cbc/ofb/cfb）。
    pub fn name(self) -> &'static str {
        match self.block_mode {
            BlockMode::Ecb => "ecb",
            BlockMode::Cbc => "cbc",
            BlockMode::Ofb => "ofb",
            BlockMode::Cfb => "cfb",
        }
    }
}

/// 校验两列行数一致。
fn check_len(expected: usize, actual: usize) -> Result<(), EvalError> {
    if expected == actual {
        Ok(())
    } else {
        Err(EvalError::ColumnLengthMismatch { expected, actual })
    }
}

/// MySQL 风格密钥派生：循环异或折叠到固定长度。
fn derive_key_mysql(key: &[u8], key_size: usize) -> Vec<u8> {
    let mut derived = vec![0_u8; key_size];
    for (index, byte) in key.iter().enumerate() {
        derived[index % key_size] ^= byte;
    }
    derived
}

#[derive(Debug)]
/// 行级加密失败标记（转换为该行 SQL NULL，非语句错误）。
struct CryptoError;

/// PKCS#7 填充到 16 字节边界。
fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let pad_len = 16 - data.len() % 16;
    let mut output = data.to_vec();
    output.resize(output.len() + pad_len, pad_len as u8);
    output
}

/// PKCS#7 去填充；非法填充视为加密失败。
fn pkcs7_unpad(data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(CryptoError);
    }
    let pad_len = data[data.len() - 1] as usize;
    if pad_len == 0
        || pad_len > 16
        || data[data.len() - pad_len..]
            .iter()
            .any(|value| *value as usize != pad_len)
    {
        return Err(CryptoError);
    }
    Ok(data[..data.len() - pad_len].to_vec())
}

/// ECB 模式逐块加/解密（输入须已对齐）。
fn ecb_crypt(data: &[u8], key: &[u8], encrypt: bool) -> Result<Vec<u8>, CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError);
    }
    let mut output = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {{
            let cipher = <$cipher>::new_from_slice(key).map_err(|_| CryptoError)?;
            for chunk in output.chunks_exact_mut(16) {
                let block = GenericArray::from_mut_slice(chunk);
                if encrypt {
                    cipher.encrypt_block(block);
                } else {
                    cipher.decrypt_block(block);
                }
            }
        }};
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => return Err(CryptoError),
    }
    Ok(output)
}

/// ECB 加密（含填充）。
fn encrypt_ecb(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    ecb_crypt(&pkcs7_pad(data), key, true)
}

/// ECB 解密（含去填充）。
fn decrypt_ecb(data: &[u8], key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    pkcs7_unpad(&ecb_crypt(data, key, false)?)
}

/// CBC 加密。
fn encrypt_cbc(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError> {
    macro_rules! encrypt {
        ($cipher:ty) => {
            cbc::Encryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| CryptoError)?
                .encrypt_padded_vec_mut::<Pkcs7>(data)
        };
    }
    Ok(match key.len() {
        16 => encrypt!(Aes128),
        24 => encrypt!(Aes192),
        32 => encrypt!(Aes256),
        _ => return Err(CryptoError),
    })
}

/// CBC 解密。
fn decrypt_cbc(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError);
    }
    macro_rules! decrypt {
        ($cipher:ty) => {
            cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| CryptoError)?
                .decrypt_padded_vec_mut::<Pkcs7>(data)
                .map_err(|_| CryptoError)?
        };
    }
    Ok(match key.len() {
        16 => decrypt!(Aes128),
        24 => decrypt!(Aes192),
        32 => decrypt!(Aes256),
        _ => return Err(CryptoError),
    })
}

/// OFB 流密码（加解密同一运算）。
fn crypt_ofb(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut output = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            ofb::Ofb::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| CryptoError)?
                .apply_keystream(&mut output)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => return Err(CryptoError),
    }
    Ok(output)
}

/// CFB 加密。
fn encrypt_cfb(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut output = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            cfb_mode::Encryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| CryptoError)?
                .encrypt(&mut output)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => return Err(CryptoError),
    }
    Ok(output)
}

/// CFB 解密。
fn decrypt_cfb(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut output = data.to_vec();
    macro_rules! apply {
        ($cipher:ty) => {
            cfb_mode::Decryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| CryptoError)?
                .decrypt(&mut output)
        };
    }
    match key.len() {
        16 => apply!(Aes128),
        24 => apply!(Aes192),
        32 => apply!(Aes256),
        _ => return Err(CryptoError),
    }
    Ok(output)
}

/// 取第 row 行 IV：NULL 行返回 None；过短返回语句错误；否则截取 16 字节。
fn checked_iv<'a>(
    ivs: Option<&'a ByteColumn>,
    row: usize,
    function: &'static str,
) -> Result<Option<&'a [u8]>, EvalError> {
    let ivs = ivs.ok_or(EvalError::MissingIv(function))?;
    match ivs[row].as_deref() {
        None => Ok(None),
        Some(iv) if iv.len() < 16 => Err(EvalError::ShortIv { function }),
        Some(iv) => Ok(Some(&iv[..16])),
    }
}

/// 向量化 AES 加密；任一行明文/密钥为 NULL 则该行结果为 NULL。
pub fn aes_encrypt_vec(
    ctx: &mut EvalContext,
    input: &ByteColumn,
    keys: &ByteColumn,
    ivs: Option<&ByteColumn>,
    mode: AesMode,
) -> Result<ByteColumn, EvalError> {
    check_len(input.len(), keys.len())?;
    if mode.block_mode != BlockMode::Ecb {
        let ivs = ivs.ok_or(EvalError::MissingIv("aes_encrypt"))?;
        check_len(input.len(), ivs.len())?;
    }

    let mut result = Vec::with_capacity(input.len());
    for row in 0..input.len() {
        // 明文或密钥为 NULL：本行输出 NULL，不中断整批。
        let (Some(data), Some(key)) = (input[row].as_deref(), keys[row].as_deref()) else {
            result.push(None);
            continue;
        };
        let key = derive_key_mysql(key, mode.key_size);
        let encrypted = match mode.block_mode {
            BlockMode::Ecb => {
                if ivs.is_some() {
                    ctx.warn(Warning::IvIgnored);
                }
                encrypt_ecb(data, &key)
            }
            BlockMode::Cbc => match checked_iv(ivs, row, "aes_encrypt")? {
                Some(iv) => encrypt_cbc(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
            BlockMode::Ofb => match checked_iv(ivs, row, "aes_encrypt")? {
                Some(iv) => crypt_ofb(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
            BlockMode::Cfb => match checked_iv(ivs, row, "aes_encrypt")? {
                Some(iv) => encrypt_cfb(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
        };
        result.push(encrypted.ok());
    }
    Ok(result)
}

/// 向量化 AES 解密。
pub fn aes_decrypt_vec(
    ctx: &mut EvalContext,
    input: &ByteColumn,
    keys: &ByteColumn,
    ivs: Option<&ByteColumn>,
    mode: AesMode,
) -> Result<ByteColumn, EvalError> {
    check_len(input.len(), keys.len())?;
    if mode.block_mode != BlockMode::Ecb {
        let ivs = ivs.ok_or(EvalError::MissingIv("aes_decrypt"))?;
        check_len(input.len(), ivs.len())?;
    }

    let mut result = Vec::with_capacity(input.len());
    for row in 0..input.len() {
        let (Some(data), Some(key)) = (input[row].as_deref(), keys[row].as_deref()) else {
            result.push(None);
            continue;
        };
        let key = derive_key_mysql(key, mode.key_size);
        let decrypted = match mode.block_mode {
            BlockMode::Ecb => {
                if ivs.is_some() {
                    ctx.warn(Warning::IvIgnored);
                }
                decrypt_ecb(data, &key)
            }
            BlockMode::Cbc => match checked_iv(ivs, row, "aes_decrypt")? {
                Some(iv) => decrypt_cbc(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
            BlockMode::Ofb => match checked_iv(ivs, row, "aes_decrypt")? {
                Some(iv) => crypt_ofb(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
            BlockMode::Cfb => match checked_iv(ivs, row, "aes_decrypt")? {
                Some(iv) => decrypt_cfb(data, &key, iv),
                None => {
                    result.push(None);
                    continue;
                }
            },
        };
        result.push(decrypted.ok());
    }
    Ok(result)
}

#[derive(Clone, Copy, Default)]
/// MySQL ENCODE/DECODE 使用的伪随机状态。
struct SqlRand {
    seed1: u32,
    seed2: u32,
    max_value: u32,
    max_value_dbl: f64,
}

impl SqlRand {
    /// 用口令初始化种子（跳过空格与制表符）。
    fn initialize(&mut self, password: &[u8]) {
        let (mut nr, mut add, mut nr2) = (1_345_345_333_u32, 7_u32, 0x1234_5671_u32);
        for byte in password
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, b' ' | b'\t'))
        {
            let value = u32::from(byte);
            nr ^= (nr & 63)
                .wrapping_add(add)
                .wrapping_mul(value)
                .wrapping_add(nr << 8);
            nr2 = nr2.wrapping_add((nr2 << 8) ^ nr);
            add = add.wrapping_add(value);
        }
        self.max_value = 0x3fff_ffff;
        self.max_value_dbl = f64::from(self.max_value);
        self.seed1 = (nr & 0x7fff_ffff) % self.max_value;
        self.seed2 = (nr2 & 0x7fff_ffff) % self.max_value;
    }

    /// 产生下一个 [0,1) 伪随机数。
    fn next(&mut self) -> f64 {
        self.seed1 = self.seed1.wrapping_mul(3).wrapping_add(self.seed2) % self.max_value;
        self.seed2 = self.seed1.wrapping_add(self.seed2).wrapping_add(33) % self.max_value;
        f64::from(self.seed1) / self.max_value_dbl
    }
}

/// 基于口令的置换表加解密（历史 MySQL SQL crypt）。
struct SqlCrypt {
    random: SqlRand,
    decode: [u8; 256],
    encode: [u8; 256],
    shift: u32,
}

impl SqlCrypt {
    /// 由口令构建编解码置换表。
    fn new(password: &[u8]) -> Self {
        let mut value = Self {
            random: SqlRand::default(),
            decode: [0; 256],
            encode: [0; 256],
            shift: 0,
        };
        value.random.initialize(password);
        for (index, slot) in value.decode.iter_mut().enumerate() {
            *slot = index as u8;
        }
        for index in 0..256 {
            let swap_index = (value.random.next() * 255.0) as usize;
            value.decode.swap(swap_index, index);
        }
        for index in 0..256 {
            value.encode[value.decode[index] as usize] = index as u8;
        }
        value
    }

    /// 原地编码。
    fn encode(&mut self, data: &mut [u8]) {
        for byte in data {
            self.shift ^= (self.random.next() * 255.0) as u32;
            let original = *byte;
            *byte = self.encode[original as usize] ^ self.shift as u8;
            self.shift ^= u32::from(original);
        }
    }

    /// 原地解码。
    fn decode(&mut self, data: &mut [u8]) {
        for byte in data {
            self.shift ^= (self.random.next() * 255.0) as u32;
            *byte = self.decode[(*byte ^ self.shift as u8) as usize];
            self.shift ^= u32::from(*byte);
        }
    }
}

/// 按行对字节列做 ENCODE 或 DECODE。
fn crypt_columns(
    input: &ByteColumn,
    passwords: &ByteColumn,
    encode: bool,
) -> Result<ByteColumn, EvalError> {
    check_len(input.len(), passwords.len())?;
    Ok(input
        .iter()
        .zip(passwords)
        .map(|(data, password)| match (data, password) {
            (Some(data), Some(password)) => {
                let mut output = data.clone();
                let mut crypt = SqlCrypt::new(password);
                if encode {
                    crypt.encode(&mut output);
                } else {
                    crypt.decode(&mut output);
                }
                Some(output)
            }
            _ => None,
        })
        .collect())
}

/// 向量化 SQL ENCODE。
pub fn sql_encode_vec(input: &ByteColumn, passwords: &ByteColumn) -> Result<ByteColumn, EvalError> {
    crypt_columns(input, passwords, true)
}

/// 向量化 SQL DECODE。
pub fn sql_decode_vec(input: &ByteColumn, passwords: &ByteColumn) -> Result<ByteColumn, EvalError> {
    crypt_columns(input, passwords, false)
}

/// 向量化 RANDOM_BYTES；任一非法长度导致整句错误。
pub fn random_bytes_vec(lengths: &IntColumn) -> Result<ByteColumn, EvalError> {
    let mut rng = rand::thread_rng();
    let mut result = Vec::with_capacity(lengths.len());
    for length in lengths {
        match length {
            None => result.push(None),
            Some(length @ 1..=1024) => {
                let mut bytes = vec![0_u8; *length as usize];
                rng.fill_bytes(&mut bytes);
                result.push(Some(bytes));
            }
            Some(_) => return Err(EvalError::RandomBytesLength),
        }
    }
    Ok(result)
}

/// 通用摘要列计算，结果为十六进制字节。
fn digest_vec<D: Digest + Default>(input: &ByteColumn) -> ByteColumn {
    input
        .iter()
        .map(|value| {
            value
                .as_deref()
                .map(|value| hex::encode(D::digest(value)).into_bytes())
        })
        .collect()
}

/// 向量化 MD5。
pub fn md5_vec(input: &ByteColumn) -> ByteColumn {
    digest_vec::<Md5>(input)
}

/// 向量化 SHA1。
pub fn sha1_vec(input: &ByteColumn) -> ByteColumn {
    digest_vec::<Sha1>(input)
}

/// 向量化 SM3。
pub fn sm3_vec(input: &ByteColumn) -> ByteColumn {
    input
        .iter()
        .map(|value| {
            value
                .as_deref()
                .map(|value| hex::encode(Sm3Hash(value)).into_bytes())
        })
        .collect()
}

/// 向量化 SHA2；非法长度行输出 NULL。
pub fn sha2_vec(input: &ByteColumn, hash_lengths: &IntColumn) -> Result<ByteColumn, EvalError> {
    check_len(input.len(), hash_lengths.len())?;
    Ok(input
        .iter()
        .zip(hash_lengths)
        .map(
            |(value, hash_length)| match (value.as_deref(), hash_length) {
                (Some(value), Some(0 | 256)) => {
                    Some(hex::encode(Sha256::digest(value)).into_bytes())
                }
                (Some(value), Some(224)) => Some(hex::encode(Sha224::digest(value)).into_bytes()),
                (Some(value), Some(384)) => Some(hex::encode(Sha384::digest(value)).into_bytes()),
                (Some(value), Some(512)) => Some(hex::encode(Sha512::digest(value)).into_bytes()),
                _ => None,
            },
        )
        .collect())
}

/// 向量化 PASSWORD()；非空口令产生弃用警告。
pub fn password_vec(ctx: &mut EvalContext, input: &ByteColumn) -> ByteColumn {
    input
        .iter()
        .map(|value| match value.as_deref() {
            None | Some([]) => Some(Vec::new()),
            Some(value) => {
                ctx.warn(Warning::PasswordDeprecated);
                let first = Sha1::digest(value);
                let second = Sha1::digest(first);
                Some(format!("*{}", hex::encode_upper(second)).into_bytes())
            }
        })
        .collect()
}

/// zlib 压缩并改写结尾以匹配 Go COMPRESS 字节布局。
fn deflate(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::default());
    writer.write_all(data)?;
    writer.flush()?;

    // Go zlib 以 Flush 产生的空 stored 块标为 final 收尾；flate2 默认再追加固定块，
    // 字节会与 TiDB COMPRESS 不一致，故手动改写。
    // Go's compress/zlib closes a stream by marking the empty stored block emitted by Flush as
    // final. flate2/miniz normally appends a second empty fixed block on finish, which is valid
    // zlib but differs byte-for-byte from TiDB's COMPRESS result. Marking the pending stored block
    // final and writing the Adler-32 trailer preserves the Go wire representation.
    let mut output = writer.get_ref().clone();
    if output.len() < 5 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "zlib writer did not emit a flush block",
        ));
    }
    let flush_header = output.len() - 5;
    output[flush_header] |= 0x04;
    output.extend_from_slice(&adler2::adler32_slice(data).to_be_bytes());
    Ok(output)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InflateError {
    Data,
    Buffer,
}

/// 在声明长度的硬上限内解压，避免畸形长度头导致无界输出分配。
fn inflate(data: &[u8], declared: usize) -> Result<Vec<u8>, InflateError> {
    let mut decompressor = Decompress::new(true);
    let mut output = Vec::with_capacity(declared.min(8 * 1024));
    let mut input_offset = 0;
    loop {
        let input_before = decompressor.total_in();
        let output_before = decompressor.total_out();
        let mut buffer = [0_u8; 8 * 1024];
        let status = decompressor
            .decompress(&data[input_offset..], &mut buffer, FlushDecompress::Finish)
            .map_err(|_| InflateError::Data)?;
        let produced = (decompressor.total_out() - output_before) as usize;
        if produced > declared.saturating_sub(output.len()) {
            return Err(InflateError::Buffer);
        }
        output.extend_from_slice(&buffer[..produced]);
        input_offset = decompressor.total_in() as usize;

        if status == Status::StreamEnd {
            return Ok(output);
        }
        if decompressor.total_in() == input_before && decompressor.total_out() == output_before {
            return Err(InflateError::Data);
        }
    }
}

/// 向量化 COMPRESS。
pub fn compress_vec(input: &ByteColumn) -> ByteColumn {
    input
        .iter()
        .map(|value| match value.as_deref() {
            None => None,
            Some([]) => Some(Vec::new()),
            Some(value) => {
                let compressed = deflate(value).ok()?;
                let mut result = Vec::with_capacity(4 + compressed.len() + 1);
                result.extend_from_slice(&(value.len() as u32).to_le_bytes());
                result.extend_from_slice(&compressed);
                if result.last() == Some(&b' ') {
                    result.push(b'.');
                }
                Some(result)
            }
        })
        .collect()
}

/// 向量化 UNCOMPRESS；损坏行置 NULL 并告警。
pub fn uncompress_vec(ctx: &mut EvalContext, input: &ByteColumn) -> ByteColumn {
    input
        .iter()
        .map(|value| match value.as_deref() {
            None => None,
            Some([]) => Some(Vec::new()),
            Some(value) if value.len() <= 4 => {
                ctx.warn(Warning::ZlibData);
                None
            }
            Some(value) => {
                let declared = u32::from_le_bytes(value[..4].try_into().unwrap()) as usize;
                match inflate(&value[4..], declared) {
                    Err(InflateError::Data) => {
                        ctx.warn(Warning::ZlibData);
                        None
                    }
                    Err(InflateError::Buffer) => {
                        ctx.warn(Warning::ZlibBuffer);
                        None
                    }
                    Ok(output) => Some(output),
                }
            }
        })
        .collect()
}

/// 向量化 UNCOMPRESSED_LENGTH。
pub fn uncompressed_length_vec(ctx: &mut EvalContext, input: &ByteColumn) -> IntColumn {
    input
        .iter()
        .map(|value| match value.as_deref() {
            None => None,
            Some([]) => Some(0),
            Some(value) if value.len() <= 4 => {
                ctx.warn(Warning::ZlibData);
                Some(0)
            }
            Some(value) => Some(i64::from(u32::from_le_bytes(
                value[..4].try_into().unwrap(),
            ))),
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 向量化密码强度策略。
pub struct PasswordPolicy {
    pub username: Option<String>,
    pub auth_username: Option<String>,
    pub check_username: bool,
    pub min_length: usize,
    pub mixed_case_count: usize,
    pub number_count: usize,
    pub special_char_count: usize,
    pub dictionary: Vec<String>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            username: None,
            auth_username: None,
            check_username: true,
            min_length: 8,
            mixed_case_count: 1,
            number_count: 1,
            special_char_count: 1,
            dictionary: Vec::new(),
        }
    }
}

/// 单行密码强度评分（0/25/50/75/100）。
fn password_score(value: &[u8], policy: &PasswordPolicy) -> i64 {
    if policy.check_username {
        for username in [policy.auth_username.as_deref(), policy.username.as_deref()]
            .into_iter()
            .flatten()
            .filter(|value| !value.is_empty())
        {
            let username = username.as_bytes();
            let reversed: Vec<u8> = username.iter().rev().copied().collect();
            if value
                .windows(username.len())
                .any(|window| window == username)
                || value
                    .windows(reversed.len())
                    .any(|window| window == reversed.as_slice())
            {
                return 0;
            }
        }
    }

    let Ok(password) = std::str::from_utf8(value) else {
        return 0;
    };
    let character_count = password.chars().count();
    if character_count < policy.min_length {
        return 25;
    }

    let mut lower_count = 0;
    let mut upper_count = 0;
    let mut number_count = 0;
    let mut special_count = 0;
    for character in password.chars() {
        match get_general_category(character) {
            GeneralCategory::LowercaseLetter => lower_count += 1,
            GeneralCategory::UppercaseLetter => upper_count += 1,
            GeneralCategory::DecimalNumber => number_count += 1,
            _ => special_count += 1,
        }
    }
    if lower_count < policy.mixed_case_count
        || upper_count < policy.mixed_case_count
        || number_count < policy.number_count
        || special_count < policy.special_char_count
    {
        return 50;
    }

    let lower = password.to_lowercase();
    if policy
        .dictionary
        .iter()
        .filter(|word| (4..=100).contains(&word.len()))
        .any(|word| lower.contains(&word.to_lowercase()))
    {
        return 75;
    }
    100
}

/// 向量化 VALIDATE_PASSWORD_STRENGTH；未启用以 0 填充非 NULL 行。
pub fn validate_password_strength_vec(
    input: &ByteColumn,
    enabled: bool,
    policy: &PasswordPolicy,
) -> IntColumn {
    input
        .iter()
        .map(|value| {
            value.as_deref().map(|value| {
                if enabled {
                    password_score(value, policy)
                } else {
                    0
                }
            })
        })
        .collect()
}
