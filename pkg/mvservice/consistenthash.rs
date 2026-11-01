// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::{collections::BTreeSet, sync::Arc};
pub type HashFunction = Arc<dyn Fn(&[u8]) -> u32 + Send + Sync>;
/// MurmurHash3 x86_32, zero seed, identical to twmb/murmur3.Sum32.
pub fn murmur3(data: &[u8]) -> u32 {
    let mut h = 0u32;
    let mix = |mut k: u32| {
        k = k.wrapping_mul(0xcc9e2d51);
        k = k.rotate_left(15);
        k.wrapping_mul(0x1b873593)
    };
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        h ^= mix(u32::from_le_bytes(chunk.try_into().unwrap()));
        h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xe6546b64);
    }
    let tail = chunks.remainder();
    let mut k = 0u32;
    for (i, b) in tail.iter().enumerate() {
        k |= (*b as u32) << (8 * i)
    }
    if !tail.is_empty() {
        h ^= mix(k)
    }
    h ^= data.len() as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^ (h >> 16)
}
pub struct ConsistentHash {
    pub(crate) ring: Vec<(u32, String)>,
    nodes: BTreeSet<String>,
    replicas: usize,
    pub hash_func: HashFunction,
}
impl ConsistentHash {
    pub fn new(replicas: isize) -> Self {
        Self {
            ring: Vec::new(),
            nodes: BTreeSet::new(),
            replicas: replicas.clamp(1, 200) as usize,
            hash_func: Arc::new(murmur3),
        }
    }
    fn insert(&mut self, node: &str) -> bool {
        if node.is_empty() || !self.nodes.insert(node.into()) {
            return false;
        }
        for i in 0..self.replicas {
            self.ring.push((
                (self.hash_func)(format!("{node}#{i}").as_bytes()),
                node.into(),
            ))
        }
        true
    }
    pub fn rebuild<'a>(&mut self, nodes: impl IntoIterator<Item = &'a str>) {
        self.ring.clear();
        self.nodes.clear();
        for n in nodes {
            self.insert(n);
        }
        self.ring.sort();
    }
    pub fn add_node(&mut self, node: &str) -> bool {
        let added = self.insert(node);
        if added {
            self.ring.sort()
        }
        added
    }
    pub fn remove_node(&mut self, node: &str) {
        if self.nodes.remove(node) {
            self.ring.retain(|(_, n)| n != node)
        }
    }
    pub fn get_node(&self, key: &[u8]) -> &str {
        if self.ring.is_empty() {
            return "";
        }
        let hash = (self.hash_func)(key);
        let i = self.ring.partition_point(|(h, _)| *h < hash);
        &self.ring[i % self.ring.len()].1
    }
    pub fn get_nodes(&self) -> Vec<String> {
        self.nodes.iter().cloned().collect()
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn virtual_nodes(&self, node: &str) -> usize {
        if self.nodes.contains(node) {
            self.replicas
        } else {
            0
        }
    }
}
