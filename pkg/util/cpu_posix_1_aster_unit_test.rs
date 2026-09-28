// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `util` 包 POSIX CPU 百分比及相关杂项的迁移期聚合单测。
//
// 覆盖：`cpu_percentage_from_samples` 公式、真实 `GetCPUPercentage`、以及 errors/etcd/gogc/
// id_generator/misc/prefix_helper/printer/rlimit 等同包工具的 Go 语义对照。

use super::cpu_posix::{GetCPUPercentage, cpu_percentage_from_samples};
use super::{errors, etcd, gogc, id_generator, misc, prefix_helper, printer, rlimit_other};
use anyhow::anyhow;
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// Δcpu=200、Δwall=1000 → 20%。
#[test]
fn cpu_percentage_uses_process_time_delta_over_wall_time_delta() {
    let percentage = cpu_percentage_from_samples(1_000, 50, 2_000, 250);
    assert!((percentage - 20.0).abs() < f64::EPSILON);
}

/// 墙钟间隔为 0 时与 Go 浮点除法一致，结果为无穷大。
#[test]
fn cpu_percentage_preserves_go_floating_point_zero_interval_behavior() {
    let percentage = cpu_percentage_from_samples(1_000, 50, 1_000, 250);
    assert!(percentage.is_infinite());
}

/// 连续两次真实采样应得到有限且非负的百分比。
#[test]
fn cpu_percentage_reads_real_process_usage() {
    let _ = GetCPUPercentage();
    thread::sleep(Duration::from_millis(2));
    let percentage = GetCPUPercentage();
    assert!(percentage.is_finite());
    assert!(percentage >= 0.0);
}

/// 错误链最内层错误（对应 Go `OriginError`）。
#[derive(Debug, thiserror::Error)]
#[error("root")]
struct RootError;

/// 包装 `RootError` 的外层错误。
#[derive(Debug, thiserror::Error)]
#[error("outer")]
struct OuterError {
    #[source]
    source: RootError,
}

/// `OriginError` 应穿透到最深层 source。
#[test]
fn origin_error_follows_the_complete_source_chain() {
    assert!(errors::OriginError(None).is_none());
    let outer = OuterError { source: RootError };
    let origin = errors::OriginError(Some(&outer as &(dyn StdError + 'static))).unwrap();
    assert_eq!(origin.to_string(), "root");
}

/// 可注入失败次数的 etcd SessionFactory 桩。
#[derive(Default)]
struct RetrySessionFactory {
    attempts: usize,
}

impl etcd::SessionFactory for RetrySessionFactory {
    type Session = usize;

    fn new_session(
        &mut self,
        _ctx: &etcd::CancellationContext,
        _ttl: i32,
    ) -> Result<Self::Session, anyhow::Error> {
        self.attempts += 1;
        if self.attempts == 1 {
            Err(anyhow!("temporary failure"))
        } else {
            Ok(self.attempts)
        }
    }
}

/// etcd 会话创建会重试；已取消的 context 不应发起尝试；租约 ID 十六进制格式化。
#[test]
fn etcd_session_retries_checks_context_and_formats_lease_id() {
    let context = etcd::CancellationContext::new();
    let mut factory = RetrySessionFactory::default();
    let session = etcd::NewSession(&context, "owner", &mut factory, 3, 60).unwrap();
    assert_eq!(session, Some(2));
    assert_eq!(factory.attempts, 2);
    assert_eq!(etcd::FormatLeaseID(0x2a), "000000000000002a");

    let cancelled = etcd::CancellationContext::new();
    cancelled.cancel();
    let mut factory = RetrySessionFactory::default();
    assert!(etcd::NewSession(&cancelled, "owner", &mut factory, 3, 60).is_err());
    assert_eq!(factory.attempts, 0);
}

/// GOGC 读写与 ID 生成器递增语义对齐 Go。
#[test]
fn gogc_and_id_generator_preserve_go_state_transitions() {
    let old = gogc::GetGOGC();
    assert_eq!(gogc::SetGOGC(250), old);
    assert_eq!(gogc::GetGOGC(), 250);
    assert_eq!(gogc::SetGOGC(0), 250);
    assert_eq!(gogc::GetGOGC(), 100);
    let _ = gogc::SetGOGC(old);

    let mut generator = id_generator::IDGenerator::default();
    assert_eq!(generator.GetNextID(), 0);
    assert_eq!(generator.GetNextID(), 1);
    generator.nextID = isize::MAX;
    assert_eq!(generator.GetNextID(), isize::MAX);
    assert_eq!(generator.GetNextID(), isize::MIN);
}

/// 重试、panic 恢复、X509 名称与 SAN 解析分支对齐 Go。
#[test]
fn retry_recovery_x509_and_san_match_go_behavior() {
    let mut attempts = 0;
    misc::RunWithRetry(3, 0, || {
        attempts += 1;
        if attempts < 2 {
            (true, Some(anyhow!("retry")))
        } else {
            (true, None)
        }
    })
    .unwrap();
    assert_eq!(attempts, 2);

    let recovered = Mutex::new(None::<String>);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    misc::WithRecovery(
        || panic!("test"),
        Some(|payload: Option<&(dyn std::any::Any + Send)>| {
            let value = payload.unwrap().downcast_ref::<&str>().unwrap();
            *recovered.lock().unwrap() = Some((*value).to_owned());
        }),
    );
    std::panic::set_hook(previous_hook);
    assert_eq!(recovered.into_inner().unwrap().as_deref(), Some("test"));

    let name = misc::PkixName {
        Names: vec![
            misc::MockPkixAttribute(misc::Country, "SE"),
            misc::MockPkixAttribute(misc::CommonName, "client"),
            misc::MockPkixAttribute(misc::Email, "client@example.com"),
        ],
    };
    assert_eq!(
        misc::X509NameOnline(name),
        "/C=SE/CN=client/emailAddress=client@example.com"
    );
    let sans = misc::ParseAndCheckSAN("dns: example.com, IP: 127.0.0.1").unwrap();
    assert_eq!(sans["DNS"], ["example.com"]);
    assert_eq!(sans["IP"], ["127.0.0.1"]);
    assert!(misc::ParseAndCheckSAN("EMAIL: x@example.com").is_err());
    misc::CheckSupportX509NameOneline("/C=SE/CN=client").unwrap();
    assert!(misc::CheckSupportX509NameOneline("/C=SE=bad").is_err());
}

/// 列元数据桩，用于 `ColumnsToProto` 分支覆盖。
#[derive(Clone)]
struct MockColumn {
    id: i64,
    primary: bool,
    array: bool,
    generated: bool,
}

impl misc::ColumnMetadata for MockColumn {
    fn id(&self) -> i64 {
        self.id
    }
    fn collation_id(&self) -> i32 {
        45
    }
    fn column_len(&self) -> i32 {
        10
    }
    fn decimal(&self) -> i32 {
        2
    }
    fn flags(&self) -> i32 {
        4
    }
    fn elements(&self) -> Vec<String> {
        vec!["a".to_owned()]
    }
    fn field_type(&self) -> i32 {
        3
    }
    fn array_element_type(&self) -> i32 {
        15
    }
    fn is_array(&self) -> bool {
        self.array
    }
    fn is_virtual_generated(&self) -> bool {
        self.generated
    }
    fn is_primary_key(&self) -> bool {
        self.primary
    }
}

/// 列转 proto、URL 拼接、插入类型 flags 与证书加载分支。
#[test]
fn column_proto_url_flags_and_certificates_preserve_go_branches() {
    let columns = [
        MockColumn {
            id: 1,
            primary: true,
            array: false,
            generated: true,
        },
        MockColumn {
            id: -1,
            primary: false,
            array: true,
            generated: false,
        },
    ];
    let protos = misc::ColumnsToProto(&columns, true, true, true);
    assert!(protos[0].PkHandle);
    assert_ne!(protos[0].Flag & (1 << 23), 0);
    assert!(protos[1].PkHandle);
    assert_eq!(protos[1].Tp, 15);
    assert_eq!(protos[1].Collation, 63);

    assert_eq!(
        misc::ComposeURL("server.example.com", ""),
        "http://server.example.com"
    );
    assert_eq!(
        misc::ComposeURL("https://server.example.com", "/api"),
        "https://server.example.com/api"
    );

    let flags = misc::GetTypeFlagsForInsert(
        task_types_group::Flags::default(),
        task_mysql::r#const::SQLMode::default(),
        false,
    );
    assert!(flags.TruncateAsWarning());
    assert!(flags.IgnoreZeroInDate());
    assert!(!flags.AllowNegativeToUnsigned());

    let directory = tempfile::tempdir().unwrap();
    let cert = directory.path().join("cert.pem");
    let key = directory.path().join("key.pem");
    misc::CreateCertificates(
        &cert,
        &key,
        1024,
        misc::PublicKeyAlgorithm::Rsa,
        misc::SignatureAlgorithm::Unspecified,
    )
    .unwrap();
    let (config, auto_reload) = misc::LoadTLSCertificates(
        "",
        key.to_str().unwrap(),
        cert.to_str().unwrap(),
        false,
        1024,
    )
    .unwrap();
    assert!(config.is_some());
    assert!(!auto_reload);
    let (disabled, _) = misc::LoadTLSCertificates("", "", "", false, 1024).unwrap();
    assert!(disabled.is_none());
}

/// 内存 KV 迭代器，供 prefix_helper 扫描/删除测试。
struct MemoryIterator {
    entries: Vec<(prefix_helper::Key, Vec<u8>)>,
    index: usize,
    closed: bool,
}

impl prefix_helper::KvIterator for MemoryIterator {
    fn valid(&self) -> bool {
        self.index < self.entries.len()
    }
    fn key(&self) -> &prefix_helper::Key {
        &self.entries[self.index].0
    }
    fn value(&self) -> &[u8] {
        &self.entries[self.index].1
    }
    fn next(&mut self) -> Result<(), anyhow::Error> {
        self.index += 1;
        Ok(())
    }
    fn close(&mut self) {
        self.closed = true;
    }
}

/// 基于 `BTreeMap` 的可检索/可删除 KV 存储桩。
#[derive(Default)]
struct MemoryStore(BTreeMap<Vec<u8>, Vec<u8>>);

impl prefix_helper::Retriever for MemoryStore {
    fn iter(
        &self,
        start: &prefix_helper::Key,
        end: &prefix_helper::Key,
    ) -> Result<Box<dyn prefix_helper::KvIterator>, anyhow::Error> {
        let entries = self
            .0
            .range(start.0.clone()..end.0.clone())
            .map(|(key, value)| (prefix_helper::Key(key.clone()), value.clone()))
            .collect();
        Ok(Box::new(MemoryIterator {
            entries,
            index: 0,
            closed: false,
        }))
    }
}

impl prefix_helper::RetrieverMutator for MemoryStore {
    fn delete(&mut self, key: prefix_helper::Key) -> Result<(), anyhow::Error> {
        self.0.remove(&key.0);
        Ok(())
    }
}

/// 前缀扫描早停、按前缀删除与行键过滤器语义。
#[test]
fn prefix_helpers_scan_stop_delete_and_compare_like_go() {
    let mut store = MemoryStore::default();
    store.0.insert(b"key-1".to_vec(), b"one".to_vec());
    store.0.insert(b"key-2".to_vec(), b"two".to_vec());
    store.0.insert(b"other".to_vec(), b"three".to_vec());
    let prefix = prefix_helper::Key::from(&b"key-"[..]);
    let mut visited = Vec::new();
    prefix_helper::ScanMetaWithPrefix(&store, prefix.clone(), |key, _| {
        visited.push(key.0.clone());
        false
    })
    .unwrap();
    assert_eq!(visited, [b"key-1".to_vec()]);
    prefix_helper::DelKeyWithPrefix(&mut store, prefix.clone()).unwrap();
    assert_eq!(
        store.0.keys().cloned().collect::<Vec<_>>(),
        [b"other".to_vec()]
    );
    let filter = prefix_helper::RowKeyPrefixFilter(prefix);
    assert!(!filter(&prefix_helper::Key::from(&b"key-value"[..])));
    assert!(filter(&prefix_helper::Key::from(&b"other"[..])));
}

/// 构建信息打印与 rlimit 生成可正常调用。
#[test]
fn printer_and_rlimit_are_operational() {
    printer::SetBuildInfo("v1", "2026-07-14", "abc", "main");
    let info = printer::GetRawInfo("TiDB");
    assert!(info.contains("App Name: TiDB"));
    assert!(info.contains("Release Version: v1"));
    assert!(info.contains("Rust Version:"));
    assert!(rlimit_other::GenRLimit("test") > 0);
}
