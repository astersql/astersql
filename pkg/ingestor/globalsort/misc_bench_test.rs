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

// 杂项基准测试辅助：外部存储跳过门闩，以及与 Go 基准相同的键生成/数据源契约。

use crate::Storage;
use std::sync::mpsc::{Receiver, sync_channel};

/// 打开外部测试存储；当前移植未接线 URI，恒为 `None`（等同 Go 默认跳过）。
///
/// Returns `Some` storage handle when an external testing storage URI is
/// configured, mirroring Go's `-testing-storage-uri` flag. No such
/// configuration mechanism is wired up in this port, so this always returns
/// `None`, which is exactly the default (flag unset) behavior upstream.
pub fn open_testing_storage() -> Option<Box<dyn Storage>> {
    None
}

trait KvSource {
    fn next(&mut self) -> Option<(Vec<u8>, Vec<u8>)>;
    fn output_size(&self) -> usize;
}

fn increment_suffix_len(count: usize) -> usize {
    if count <= 1 {
        0
    } else {
        ((usize::BITS - (count - 1).leading_zeros()) as usize).div_ceil(8)
    }
}

fn generate_ascending_key(
    count: usize,
    key_size: usize,
    key_common_prefix: &[u8],
) -> Receiver<Vec<u8>> {
    let prefix_len = key_common_prefix.len();
    let suffix_len = increment_suffix_len(count);
    assert!(
        key_size >= prefix_len + suffix_len,
        "key size {key_size} is too small, keyCommonPrefixSize: {prefix_len}, incSuffixLen: {suffix_len}"
    );

    let (sender, receiver) = sync_channel(100);
    let mut current = vec![0; key_size];
    current[..prefix_len].copy_from_slice(key_common_prefix);
    std::thread::spawn(move || {
        for _ in 0..count {
            for byte in current[prefix_len..prefix_len + suffix_len]
                .iter_mut()
                .rev()
            {
                *byte = byte.wrapping_add(1);
                if *byte != 0 {
                    break;
                }
            }
            if sender.send(current.clone()).is_err() {
                break;
            }
        }
    });
    receiver
}

struct AscendingKeySource {
    value_size: usize,
    keys: Vec<Vec<u8>>,
    keys_idx: usize,
    total_size: usize,
}

impl AscendingKeySource {
    fn new(count: usize, key_size: usize, value_size: usize, prefix: &[u8]) -> Self {
        let keys: Vec<_> = generate_ascending_key(count, key_size, prefix)
            .into_iter()
            .collect();
        Self {
            value_size,
            total_size: keys.iter().map(|key| key.len() + value_size).sum(),
            keys,
            keys_idx: 0,
        }
    }
}

impl KvSource for AscendingKeySource {
    fn next(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        let key = self.keys.get(self.keys_idx)?.clone();
        self.keys_idx += 1;
        Some((key, vec![0; self.value_size]))
    }

    fn output_size(&self) -> usize {
        self.total_size
    }
}

struct AscendingKeyAsyncSource {
    value_size: usize,
    keys: Receiver<Vec<u8>>,
    total_size: usize,
}

impl AscendingKeyAsyncSource {
    fn new(count: usize, key_size: usize, value_size: usize, prefix: &[u8]) -> Self {
        Self {
            value_size,
            keys: generate_ascending_key(count, key_size, prefix),
            total_size: 0,
        }
    }
}

impl KvSource for AscendingKeyAsyncSource {
    fn next(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        let key = self.keys.recv().ok()?;
        self.total_size += key.len() + self.value_size;
        Some((key, vec![0; self.value_size]))
    }

    fn output_size(&self) -> usize {
        self.total_size
    }
}

fn generate_random_key(
    count: usize,
    key_size: usize,
    key_common_prefix: &[u8],
    seed: u64,
) -> Receiver<Vec<u8>> {
    let prefix_len = key_common_prefix.len();
    let suffix_len = increment_suffix_len(count);
    assert!(
        key_size >= prefix_len + suffix_len,
        "key size {key_size} is too small, keyCommonPrefixSize: {prefix_len}, incSuffixLen: {suffix_len}"
    );
    let random_len = key_size - prefix_len - suffix_len;
    let (sender, receiver) = sync_channel(100);
    let mut current = vec![0; key_size];
    current[..prefix_len].copy_from_slice(key_common_prefix);
    std::thread::spawn(move || {
        // The benchmark only requires a deterministic pseudo-random prefix. The
        // monotonically increasing suffix, as in Go, guarantees unique keys.
        let mut state = seed;
        for _ in 0..count {
            for byte in &mut current[prefix_len..prefix_len + random_len] {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            for byte in current[prefix_len + random_len..].iter_mut().rev() {
                *byte = byte.wrapping_add(1);
                if *byte != 0 {
                    break;
                }
            }
            if sender.send(current.clone()).is_err() {
                break;
            }
        }
    });
    receiver
}

struct RandomKeySource {
    value_size: usize,
    keys: Vec<Vec<u8>>,
    keys_idx: usize,
    total_size: usize,
}

impl RandomKeySource {
    fn new(count: usize, key_size: usize, value_size: usize, prefix: &[u8], seed: u64) -> Self {
        let keys: Vec<_> = generate_random_key(count, key_size, prefix, seed)
            .into_iter()
            .collect();
        Self {
            value_size,
            total_size: keys.iter().map(|key| key.len() + value_size).sum(),
            keys,
            keys_idx: 0,
        }
    }
}

impl KvSource for RandomKeySource {
    fn next(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        let key = self.keys.get(self.keys_idx)?.clone();
        self.keys_idx += 1;
        Some((key, vec![0; self.value_size]))
    }

    fn output_size(&self) -> usize {
        self.total_size
    }
}

#[test]
fn ascending_sources_match_go_contract() {
    let mut source = AscendingKeySource::new(257, 8, 3, b"pre");
    assert_eq!(257 * 11, source.output_size());
    let mut previous = None;
    for _ in 0..257 {
        let (key, value) = source.next().expect("source should contain requested key");
        assert_eq!(8, key.len());
        assert!(key.starts_with(b"pre"));
        assert_eq!(vec![0; 3], value);
        if let Some(previous) = previous {
            assert!(previous < key);
        }
        previous = Some(key);
    }
    assert!(source.next().is_none());

    let mut asynchronous = AscendingKeyAsyncSource::new(2, 4, 5, b"p");
    assert_eq!(0, asynchronous.output_size());
    assert!(asynchronous.next().is_some());
    assert_eq!(9, asynchronous.output_size());
    assert!(asynchronous.next().is_some());
    assert!(asynchronous.next().is_none());
    assert_eq!(18, asynchronous.output_size());
}

#[test]
fn random_source_preserves_prefix_and_unique_suffix() {
    let mut source = RandomKeySource::new(300, 9, 7, b"px", 42);
    assert_eq!(300 * 16, source.output_size());
    let mut keys = std::collections::HashSet::new();
    while let Some((key, value)) = source.next() {
        assert!(key.starts_with(b"px"));
        assert_eq!(vec![0; 7], value);
        assert!(keys.insert(key));
    }
    assert_eq!(300, keys.len());
}

#[test]
#[should_panic(expected = "key size 2 is too small")]
fn ascending_generator_rejects_key_that_cannot_hold_prefix_and_suffix() {
    let _ = generate_ascending_key(257, 2, b"p");
}

#[test]
#[should_panic(expected = "key size 2 is too small")]
fn random_generator_rejects_key_that_cannot_hold_prefix_and_suffix() {
    let _ = generate_random_key(257, 2, b"p", 1);
}
