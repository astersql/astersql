// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! 按前缀分页扫描键值源；对齐 Go `prefix_scanner.go`。
//!
//! 用 `PrefixNextKey` 划定半开区间上界，再通过 `Page`/`AllPages` 在 etcd 或内存源上翻页。

use crate::stubs::Entry;

/// 计算前缀的下一个键，对齐 tikv client-go `kv.PrefixNextKey`。
/// PrefixNextKey matches tikv client-go kv.PrefixNextKey.
pub fn PrefixNextKey(key: &[u8]) -> Vec<u8> {
    let mut buf = key.to_vec();
    let mut i = buf.len();
    // 从末字节进位：非 0xff 加一后截断；全 0xff 溢出时返回空上界。
    while i > 0 {
        i -= 1;
        buf[i] = buf[i].wrapping_add(1);
        if buf[i] != 0 {
            return buf[..=i].to_vec();
        }
    }
    // TiKV client-go uses an empty end key as +infinity when every byte overflows
    // (including the empty input), rather than appending a zero byte.
    Vec::new()
}

/// 可扫描的键值源抽象（etcd / 内存假源等），返回 `(条目, 是否还有更多)`。
pub trait Source: Send + Sync {
    fn Scan(&self, from: &[u8], to: &[u8], limit: i32) -> Result<(Vec<Entry>, bool), String>;
}

/// 在 `[prefix, PrefixNextKey(prefix))` 上维护游标的分页扫描器。
pub struct PrefixScanner<'a> {
    src: &'a dyn Source,
    /// 下一页起点（含）。
    next: Vec<u8>,
    /// 前缀扫描上界（不含）。
    end: Vec<u8>,
    done: bool,
}

/// `AllPages` 的聚合错误；与 Go 的 `(已完成条目, error)` 返回契约一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanAllPagesError {
    pub entries: Vec<Entry>,
    pub source: String,
}

impl std::fmt::Display for ScanAllPagesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

impl std::error::Error for ScanAllPagesError {}

/// 构造前缀扫描器；`end` 取 `PrefixNextKey`，与 Go `scanPrefix` 一致。
pub fn scanPrefix<'a>(src: &'a dyn Source, prefix: &str) -> PrefixScanner<'a> {
    PrefixScanner {
        src,
        next: prefix.as_bytes().to_vec(),
        end: PrefixNextKey(prefix.as_bytes()),
        done: false,
    }
}

impl PrefixScanner<'_> {
    /// 拉取一页：若 `more` 则把下一游标设为「最后键 + 0」，避免重复读同一键。
    pub fn Page(&mut self, size: i32) -> Result<Vec<Entry>, String> {
        let (kvs, more) = self.src.Scan(&self.next, &self.end, size)?;
        if !more {
            self.done = true;
        } else {
            // Go 侧用 lastKey+"\x00" 推进，确保严格大于已返回键。
            let mut n = kvs[kvs.len() - 1].Key.clone();
            n.push(0);
            self.next = n;
        }
        Ok(kvs)
    }

    /// 循环 `Page` 直至 `Done`，汇总全部匹配条目。
    pub fn AllPages(&mut self, size: i32) -> Result<Vec<Entry>, ScanAllPagesError> {
        let mut kvs = Vec::with_capacity(size as usize);
        while !self.Done() {
            let new_kvs = match self.Page(size) {
                Ok(new_kvs) => new_kvs,
                Err(source) => {
                    return Err(ScanAllPagesError {
                        entries: kvs,
                        source,
                    });
                }
            };
            kvs.extend(new_kvs);
        }
        Ok(kvs)
    }

    /// 是否已扫完前缀区间。
    pub fn Done(&self) -> bool {
        self.done
    }
}
