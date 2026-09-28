// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Force flush — mirrors `br/pkg/task/operator/force_flush.go`.
//! 对匹配地址模式的 TiKV 并发下发 `FlushNow`，用于日志备份强制刷盘；
//! TiFlash 一律跳过。多 store 并行，任一失败记录首错但仍等待其余线程结束，
//! 最后关闭 StoreManager/PD，与 Go 行为对齐。
//! 依赖 prepare_snap 的 dialPD/createStoreManager，避免重复实现 PD 拨号。

use std::sync::Arc;
use std::thread;

use crate::config::ForceFlushConfig;
use crate::prepare_snap::{createStoreManager, dialPD};
use crate::stubs::{Error, IsTiFlash, PDClient, Result, metapb};

/// 从 PD 拉取全部 store，过滤掉 TiFlash，只保留可刷盘的 TiKV。
/// IsTiFlash 判断与 Go 工具函数相同，防止对列存节点发 FlushNow。
pub fn getAllTiKVs(p: &dyn PDClient) -> Result<Vec<metapb::Store>> {
    let stores = p.GetAllStores()?;
    Ok(stores.into_iter().filter(|s| !IsTiFlash(s)).collect())
}

/// 入口：拨号 PD → 建 StoreManager → 按 `StoresPattern` 筛选并并发 FlushNow。
/// 无匹配 store 时不报错（空 handles），与 Go 静默成功一致。
pub fn RunForceFlush(cfg: &ForceFlushConfig) -> Result<()> {
    // 先拿 PD，再拿 StoreManager；关闭顺序相反。
    let pdMgr = dialPD(&cfg.Config)?;
    let stores = match createStoreManager(pdMgr.GetPDClient(), &cfg.Config) {
        Ok(stores) => stores,
        Err(err) => {
            pdMgr.Close();
            return Err(err);
        }
    };

    let tikvs = match getAllTiKVs(pdMgr.GetPDClient().as_ref()) {
        Ok(tikvs) => tikvs,
        Err(err) => {
            stores.Close();
            pdMgr.Close();
            return Err(err);
        }
    };
    eprintln!(
        "About to start force flushing. stores-pattern={}",
        cfg.StoresPattern
    );

    let mut handles = Vec::new();
    // 克隆正则供各线程使用；Regex 本身不可跨线程共享可变引用。
    let pattern = cfg.StoresPattern.clone();
    for s in tikvs {
        // 模式不匹配或残留 TiFlash 标记则跳过，避免误刷。
        if !pattern.is_match(&s.Address) || IsTiFlash(&s) {
            eprintln!(
                "Skipping TiFlash or not matched TiKV. store={} addr={} tiflash={}",
                s.Id,
                s.Address,
                IsTiFlash(&s)
            );
            continue;
        }
        eprintln!(
            "Starting force flush TiKV. store={} addr={}",
            s.Id, s.Address
        );
        let stores = stores.clone();
        let store_id = s.Id;
        // 每 store 一线程：Go 用 errgroup，这里用 join 收集首错。
        // FlushNow 可能返回多任务结果，任一 Success=false 即失败。
        handles.push(thread::spawn(move || -> Result<()> {
            let resp = stores.FlushNow(store_id)?;
            for res in resp {
                if !res.Success {
                    return Err(Error::Errorf(format!(
                        "failed to flush task {} at store {}: {}",
                        res.TaskName, store_id, res.ErrorMessage
                    )));
                }
                eprintln!(
                    "Force flushed task of TiKV store. store={} task={}",
                    store_id, res.TaskName
                );
            }
            Ok(())
        }));
    }

    // 先等全部 worker，再关闭资源，避免半关闭状态下 RPC 仍在途。
    // panic 路径也记入 first_err，防止静默吞掉 worker 崩溃。
    let mut first_err = None;
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(_) => {
                if first_err.is_none() {
                    first_err = Some(Error::new("force flush worker panicked"));
                }
            }
        }
    }

    stores.Close();
    pdMgr.Close();
    // 与 Go 相同：返回第一个非空错误；全部成功则 Ok。
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
