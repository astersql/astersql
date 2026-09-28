// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Vitess 分片键哈希：null-key DES（64 位密钥/块）将 u64 映射为哈希。
//
// 用于确定 shard key range（分片键区间）。对应 Go `pkg/util/vitess` 的
// `HashUint64`：大端写入块、全零密钥加密后再按大端读出。

use std::{convert::Infallible, sync::LazyLock};

use des::{
    Des,
    cipher::{Block, BlockEncrypt, KeyInit},
};

// nullKeyBlock 对应 Go 包级 cipher.Block。固定长度的全零密钥不可能初始化失败。
/// 全零 DES 密钥对应的全局 cipher.Block（惰性初始化一次）。
static NULL_KEY_BLOCK: LazyLock<Des> =
    LazyLock::new(|| Des::new_from_slice(&[0_u8; 8]).expect("an eight-byte DES key must be valid"));

// HashUint64 implements vitess' method of calculating a hash used for determining a shard key range.
// Uses a DES encryption with 64 bit key, 64 bit block, null-key
/// 按 Vitess 规则对分片键做 null-key DES 哈希，返回大端解释的 u64。
pub fn HashUint64(shardKey: u64) -> Result<u64, Infallible> {
    // 大端序列化到 8 字节块，加密后再按大端还原为 u64。
    let mut hashed = Block::<Des>::default();
    hashed.copy_from_slice(&shardKey.to_be_bytes());
    NULL_KEY_BLOCK.encrypt_block(&mut hashed);

    Ok(u64::from_be_bytes(hashed.into()))
}
