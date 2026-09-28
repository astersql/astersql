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

//! Go `log_file_map_test.go` — LogFilesSkipMap vs native map parity.
//! 用原生 HashMap/HashSet 作真理源，在不同稀疏密度下对照 SkipMap 的 Insert/NeedSkip。
//! 不覆盖 Ext 变体：本测试只验证基础 SkipMap，与 Go 同名用例范围一致。

use std::collections::{HashMap, HashSet};

use crate::log_file_map::NewLogFilesSkipMap;

/// Go `TestLogFilesSkipMap`：随机抽样插入后双向校验。
/// ratio 从 0.1 倍增到接近 1.0，覆盖稀疏与近似稠密场景。
/// 正向保证已插入必跳过，反向保证未插入不得误跳过。
#[test]
fn test_log_files_skip_map() {
    let meta_num = 2;
    let group_num = 4;
    let file_num = 1000;
    let mut ratio = 0.1_f64;
    // LCG 种子与 Go 测试相同乘数，保证跨语言可复现抽样序列。
    let mut seed = 1u64;

    while ratio < 1.0 {
        let mut skipmap = NewLogFilesSkipMap();
        // nativemap 作为期望集合：仅在首次插入时同步写入 skipmap。
        let mut nativemap: HashMap<String, HashMap<i32, HashSet<i32>>> = HashMap::new();
        let mut count = 0;
        // 目标插入次数随 ratio 放大，逼近全空间以检验位图边界。
        let n = (ratio * (meta_num * group_num * file_num) as f64) as i32;
        for _ in 0..n {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let meta_key = format!("{}", (seed % meta_num as u64) as i32);
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let group_off = (seed % group_num as u64) as i32;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let file_off = (seed % file_num as u64) as i32;

            let gp = nativemap
                .entry(meta_key.clone())
                .or_default()
                .entry(group_off)
                .or_default();
            // HashSet::insert 返回 false 表示重复，跳过二次 Insert 以匹配 Go。
            if gp.insert(file_off) {
                skipmap.Insert(&meta_key, group_off, file_off);
                count += 1;
            }
        }

        // 正向：nativemap 中每条都必须 NeedSkip=true。
        let mut ncount = 0;
        for (meta_key, mp) in &nativemap {
            for (group_off, gp) in mp {
                for file_off in gp {
                    assert!(skipmap.NeedSkip(meta_key, *group_off, *file_off));
                    ncount += 1;
                }
            }
        }
        // count 与遍历计数一致，排除重复插入导致的统计漂移。
        assert_eq!(count, ncount);

        // 反向：全空间扫描，未插入的偏移不得被误判为跳过。
        for metai in 0..meta_num {
            let meta_key = format!("{metai}");
            for groupi in 0..group_num {
                for filei in 0..file_num {
                    let should_skip = nativemap
                        .get(&meta_key)
                        .and_then(|mp| mp.get(&(groupi as i32)))
                        .map(|gp| gp.contains(&(filei as i32)))
                        .unwrap_or(false);
                    if should_skip {
                        continue;
                    }
                    assert!(!skipmap.NeedSkip(&meta_key, groupi as i32, filei as i32));
                }
            }
        }
        // 倍增 ratio，进入下一密度档。
        ratio *= 2.0;
    }
}
