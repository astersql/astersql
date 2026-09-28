// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 导出辅助工具，对应 Go `export/util.go`。
// 包含集群一致性校验、字符串映射、事务隔离级别判定，以及 Go infiniteChan 的 Rust 移植。
// EtcdClient 在此为可注入错误的离线 stubs 实现。

// TiDB 在 etcd 中注册 server info 的前缀，用于收集 DDL owner ID。
pub const tidbServerInformationPath: &str = "/tidb/server/info";
// etcd 拨号超时，与 Go defaultEtcdDialTimeOut 对齐。
pub const defaultEtcdDialTimeOut: std::time::Duration = std::time::Duration::from_secs(3);
const etcdAutoSyncInterval: std::time::Duration = std::time::Duration::from_secs(30);
const etcdGetTimeout: std::time::Duration = std::time::Duration::from_secs(10);

// 从 etcd 前缀扫描结果中提取 TiDB DDL ID 列表（取 key 最后一段）。
pub fn getPdDDLIDs(cli: &EtcdClient) -> Result<Vec<String>> {
    let kvs = cli.GetPrefixWithTimeout(tidbServerInformationPath, etcdGetTimeout)?;
    let mut ids = Vec::with_capacity(kvs.len());
    for (key, _) in kvs {
        let items: Vec<&str> = key.split('/').collect();
        ids.push(items[items.len() - 1].to_string());
    }
    Ok(ids)
}

// 比较 TiDB 实例上报的 DDL ID 与 PD/etcd 侧记录是否一致，用于 snapshot 一致性前置检查。
pub fn checkSameCluster(tctx: &tcontext::Context, db: &DB, pd_addrs: &[String]) -> Result<bool> {
    let cli = EtcdClient::New(EtcdClientConfig {
        endpoints: pd_addrs.to_vec(),
        dial_timeout: defaultEtcdDialTimeOut,
        auto_sync_interval: etcdAutoSyncInterval,
    })?;
    let mut tidb_ids = GetTiDBDDLIDs(tctx, db)?;
    let mut pd_ids = getPdDDLIDs(&cli)?;
    // 排序后比较集合相等，与 Go slices.Sort + Equal 一致。
    tidb_ids.sort();
    pd_ids.sort();
    Ok(tidb_ids == pd_ids)
}

// 按索引把两个等长切片配对成 map，Go string2Map 的直译。
pub fn string2Map(a: &[String], b: &[String]) -> HashMap<String, String> {
    // 假定 a、b 等长，与 Go string2Map 相同前置条件。
    let mut a2b = HashMap::with_capacity(a.len());
    for (i, s) in a.iter().enumerate() {
        a2b.insert(s.clone(), b[i].clone());
    }
    a2b
}

// 判定导出会话是否需要 REPEATABLE READ：TiDB + snapshot 是唯一不需要的组合。
pub fn needRepeatableRead(server_type: ServerType, consistency: &str) -> bool {
    consistency != ConsistencyTypeSnapshot || server_type != ServerType::ServerTypeTiDB
}

/// Go `infiniteChan` — 后台线程 + 内部队列实现“无限缓冲” channel。
/// 生产者不会因消费者慢而阻塞；关闭时 drain 队列后退出。
pub fn infiniteChan<T: Send + 'static>() -> (Sender<T>, Receiver<T>) {
    let (in_tx, in_rx) = mpsc::channel::<Option<T>>();
    let (out_tx, out_rx) = mpsc::channel::<T>();
    // 转发 goroutine：优先向 out 发送，同时尽量从 in 收新元素入队。
    thread::spawn(move || {
        let mut q: Vec<T> = Vec::new();
        loop {
            if !q.is_empty() {
                // try send front while also accepting
                match in_rx.try_recv() {
                    Ok(Some(e)) => q.push(e),
                    Ok(None) => {
                        // None 表示上游关闭，排空队列后结束。
                        for e in q.drain(..) {
                            let _ = out_tx.send(e);
                        }
                        return;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        let e = q.remove(0);
                        if out_tx.send(e).is_err() {
                            return;
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        for e in q.drain(..) {
                            let _ = out_tx.send(e);
                        }
                        return;
                    }
                }
            } else {
                match in_rx.recv() {
                    Ok(Some(e)) => q.push(e),
                    Ok(None) | Err(_) => return,
                }
            }
        }
    });
    // 用户侧 Sender：把 T 包装成 Option 送入内部 in 通道，Drop 时发送 None 触发关闭。
    let (user_tx, user_rx) = mpsc::channel::<T>();
    thread::spawn(move || {
        for v in user_rx {
            if in_tx.send(Some(v)).is_err() {
                break;
            }
        }
        let _ = in_tx.send(None);
    });
    (user_tx, out_rx)
}
