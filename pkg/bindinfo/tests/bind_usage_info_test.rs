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

// binding（SQL 绑定）使用信息的集成测试。
//
// “binding” 是数据库为某类 SQL 固定执行计划（execution plan，优化器为语句选择
// 的具体执行方式）的机制；本文件验证 binding 被命中后，其使用时间
// （last_used_date / LastUsedAt）能被正确记录并批量写回存储层。
//
// 这些 digest 覆盖 delete、join delete、update 以及跨库 fuzzy binding 场景。
const CHECKLIST: [&str; 4] = [
    "5ce1df6eadf8b24222668b1bd2e995b72d4c88e6fe9340d8b13e834703e28c32",
    "5d3975ef2160c1e0517353798dac90a9914095d82c025e7cd97bd55aeb804798",
    "9d3995845aef70ba086d347f38a4e14c9705e966f7c5793b9fa92194bca2bbef",
    "aa3c510b94b9d680f729252ca88415794c8a4f52172c5f9e06c27bee57d08329",
];

// test_bind_usage_info 对应 Go 的 TestBindUsageInfo。
// 它验证 binding 被命中后 last_used_date 会按批量写回 storage，并且不会重复写回。
#[test]
fn test_bind_usage_info() {
    use std::sync::Arc;

    let store = UsageStore::default();
    let bindings = CHECKLIST
        .iter()
        .enumerate()
        .map(|(index, digest)| {
            let binding = Arc::new(astersql_bindinfo::Binding {
                SQLDigest: (*digest).to_owned(),
                PlanDigest: format!("plan-{index}"),
                ..Default::default()
            });
            binding.UpdateLastSavedAt(Some(astersql_bindinfo::BindingTime(0)));
            binding.UpdateLastUsedAt();
            binding
        })
        .collect::<Vec<_>>();

    astersql_bindinfo::updateBindingUsageInfoToStorage(&store, &bindings).unwrap();
    let saved = store.saved.lock().unwrap();
    assert_eq!(saved.len(), CHECKLIST.len());
    assert_eq!(
        saved
            .iter()
            .map(|(digest, _, _)| digest.as_str())
            .collect::<Vec<_>>(),
        CHECKLIST.to_vec()
    );
    drop(saved);
    for binding in &bindings {
        assert!(
            binding.UsageInfo.last_saved_at().unwrap() >= binding.UsageInfo.last_used_at().unwrap()
        );
    }

    // 对应 Go 中“没有新命中时不重复落盘”的循环边界。
    astersql_bindinfo::updateBindingUsageInfoToStorage(&store, &bindings).unwrap();
    assert_eq!(store.saved.lock().unwrap().len(), CHECKLIST.len());
}

/// 测试用的内存版 binding 存储。
///
/// 只关心 `save_usage` 的调用记录：把每次写回的
/// (SQL 摘要, 计划摘要, 使用时间) 三元组收集到 `saved` 中，
/// 其余接口均返回空实现，便于单独验证使用信息写回逻辑。
#[derive(Default)]
struct UsageStore {
    /// 记录 `save_usage` 收到的全部写回请求；用 Mutex 保护以满足
    /// trait 对并发访问的要求。
    saved: std::sync::Mutex<Vec<(String, String, astersql_bindinfo::BindingTime)>>,
}

/// 在指定调用处失败，用于复现 Go 单批事务写回的错误路径。
struct FailingUsageStore {
    calls: std::sync::Mutex<usize>,
    fail_at: usize,
}

impl astersql_bindinfo::BindingStore for FailingUsageStore {
    fn read_bindings_since(
        &self,
        _since: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<Vec<std::sync::Arc<astersql_bindinfo::Binding>>> {
        Ok(Vec::new())
    }

    fn replace_bindings(
        &self,
        _bindings: &[std::sync::Arc<astersql_bindinfo::Binding>],
    ) -> astersql_bindinfo::Result<()> {
        Ok(())
    }

    fn mark_deleted(
        &self,
        _sql_digests: &[String],
        _at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<u64> {
        Ok(0)
    }

    fn set_status(
        &self,
        _sql_digest: &str,
        _status: &str,
        _at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<bool> {
        Ok(false)
    }

    fn gc_deleted_before(
        &self,
        _cutoff: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<u64> {
        Ok(0)
    }

    fn save_usage(
        &self,
        _sql_digest: &str,
        _plan_digest: &str,
        _used_at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<()> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == self.fail_at {
            Err(astersql_bindinfo::BindError(
                "injected save failure".to_owned(),
            ))
        } else {
            Ok(())
        }
    }
}

// BindingStore 是 bindinfo crate 定义的 binding 持久化抽象；
// 这里除 save_usage 外全部给出无副作用的桩实现。
impl astersql_bindinfo::BindingStore for UsageStore {
    fn read_bindings_since(
        &self,
        _since: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<Vec<std::sync::Arc<astersql_bindinfo::Binding>>> {
        Ok(Vec::new())
    }

    fn replace_bindings(
        &self,
        _bindings: &[std::sync::Arc<astersql_bindinfo::Binding>],
    ) -> astersql_bindinfo::Result<()> {
        Ok(())
    }

    fn mark_deleted(
        &self,
        _sql_digests: &[String],
        _at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<u64> {
        Ok(0)
    }

    fn set_status(
        &self,
        _sql_digest: &str,
        _status: &str,
        _at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<bool> {
        Ok(false)
    }

    fn gc_deleted_before(
        &self,
        _cutoff: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<u64> {
        Ok(0)
    }

    // 唯一有实际行为的接口：把写回的使用信息追加到 saved 列表，供断言检查。
    fn save_usage(
        &self,
        sql_digest: &str,
        plan_digest: &str,
        used_at: astersql_bindinfo::BindingTime,
    ) -> astersql_bindinfo::Result<()> {
        self.saved
            .lock()
            .unwrap()
            .push((sql_digest.to_owned(), plan_digest.to_owned(), used_at));
        Ok(())
    }
}

/// 验证使用信息持久化只在“已保存时间戳过期”时才写回存储：
/// 当 LastSavedAt 早于 LastUsedAt 时应触发一次 save_usage，
/// 写回完成后保存时间不得早于使用时间（LastSavedAt >= LastUsedAt）。
#[test]
fn canonical_usage_persistence_updates_only_stale_saved_timestamps() {
    use std::sync::Arc;

    let store = UsageStore::default();
    // 构造一条带 SQL 摘要与计划摘要（digest：对语句/计划做哈希得到的唯一标识）的 binding。
    let binding = Arc::new(astersql_bindinfo::Binding {
        SQLDigest: "sql-digest".to_owned(),
        PlanDigest: "plan-digest".to_owned(),
        ..astersql_bindinfo::Binding::default()
    });
    // 先把“上次保存时间”设为很早的时刻，再刷新“最近使用时间”，
    // 制造 LastSavedAt < LastUsedAt 的过期状态，从而触发写回。
    binding.UpdateLastSavedAt(Some(astersql_bindinfo::BindingTime(1)));
    binding.UpdateLastUsedAt();
    astersql_bindinfo::updateBindingUsageInfoToStorage(&store, &[Arc::clone(&binding)]).unwrap();
    // 应恰好发生一次写回，且写回后的保存时间不早于使用时间。
    assert_eq!(store.saved.lock().unwrap().len(), 1);
    assert!(
        binding.UsageInfo.last_saved_at().unwrap() >= binding.UsageInfo.last_used_at().unwrap()
    );
}

/// Go `TestBindUsageInfo` 的反向断言：已经写回的最近使用时间不能重复落盘。
#[test]
fn test_bind_usage_info_skips_fresh_saved_timestamp() {
    use std::sync::Arc;

    let store = UsageStore::default();
    let binding = Arc::new(astersql_bindinfo::Binding {
        SQLDigest: "sql-digest".to_owned(),
        PlanDigest: "plan-digest".to_owned(),
        ..Default::default()
    });
    binding.UpdateLastUsedAt();
    let used_at = binding.UsageInfo.last_used_at();
    binding.UpdateLastSavedAt(used_at);

    astersql_bindinfo::updateBindingUsageInfoToStorage(&store, &[binding]).unwrap();
    assert!(store.saved.lock().unwrap().is_empty());
}

/// Go `updateBindingUsageInfoToStorageInternal` 仅在整批事务成功后更新 LastSavedAt。
#[test]
fn test_bind_usage_info_does_not_advance_any_timestamp_when_batch_fails() {
    use std::sync::Arc;

    let store = FailingUsageStore {
        calls: std::sync::Mutex::new(0),
        fail_at: 2,
    };
    let bindings = ["first", "second"].map(|digest| {
        let binding = Arc::new(astersql_bindinfo::Binding {
            SQLDigest: digest.to_owned(),
            PlanDigest: format!("plan-{digest}"),
            ..Default::default()
        });
        binding.UpdateLastSavedAt(Some(astersql_bindinfo::BindingTime(0)));
        binding.UpdateLastUsedAt();
        binding
    });

    let result = astersql_bindinfo::updateBindingUsageInfoToStorage(&store, &bindings);
    assert_eq!(result.unwrap_err().0, "injected save failure");
    assert_eq!(*store.calls.lock().unwrap(), 2);
    for binding in bindings {
        assert_eq!(
            binding.UsageInfo.last_saved_at(),
            Some(astersql_bindinfo::BindingTime(0)),
            "a failed Go transaction must not mark any binding in the batch as saved"
        );
    }
}
