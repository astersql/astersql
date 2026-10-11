// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// bindinfo 绑定操作（binding operator）测试模块。
//
// “绑定”（SQL Binding / SQL Plan Binding）是数据库中把某条 SQL 与一份指定的
// 执行计划提示（hint，如 use_index）关联起来的机制：优化器在生成执行计划时，
// 若查询的归一化形态（参数被替换为 ? 的模板）与已有绑定匹配，就按绑定给出的
// 提示选择计划，从而在不改写业务 SQL 的前提下稳定或修正执行计划。
//
// 可执行 Rust 单元测试覆盖缓存选择、持久化操作、状态切换、GC 与锁语义。
/// 验证 `pickCachedBinding` 在合并缓存绑定时的两条规则：
///
/// 1. 同一 SQL 摘要（SQLDigest）存在多个版本时，优先选择
///    `UpdateTime` 更新的绑定（新版本覆盖旧版本）；
/// 2. 若最新版本的状态为“已删除”（deleted，即墓碑标记，用于在多节点间
///    传播删除事件而非立即物理删除），则该绑定视为不存在，返回 `None`。
#[test]
fn canonical_cached_binding_prefers_newer_and_honors_delete_tombstone() {
    use std::sync::Arc;

    // 构造两个同摘要、不同更新时间的启用状态绑定：old(10) 与 newer(20)。
    let old = Arc::new(crate::Binding {
        Status: crate::StatusEnabled.to_owned(),
        UpdateTime: crate::BindingTime(10),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    let newer = Arc::new(crate::Binding {
        Status: crate::StatusEnabled.to_owned(),
        UpdateTime: crate::BindingTime(20),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    // 合并后应选中更新时间更大的 newer（UpdateTime=20）。
    assert_eq!(
        crate::pickCachedBinding(Some(old), [Arc::clone(&newer)])
            .unwrap()
            .UpdateTime,
        crate::BindingTime(20)
    );
    // 最新版本被标记为删除（UpdateTime=30 的墓碑）时，整个绑定应被判定为不存在。
    let deleted = Arc::new(crate::Binding {
        Status: crate::StatusDeleted.to_owned(),
        UpdateTime: crate::BindingTime(30),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    assert!(crate::pickCachedBinding(Some(newer), [deleted]).is_none());
}

mod operator_parity {
    use std::sync::{Arc, Mutex};

    use crate::{
        Binding, BindingCache, BindingCacheUpdater, BindingOperator, BindingRow, BindingSqlContext,
        BindingStore, BindingTime, BindingValidator, Result, SqlValue, StatusDisabled,
        StatusEnabled, TableName, bindingOperator,
    };

    #[derive(Default)]
    struct StoreState {
        replaced: Vec<Arc<Binding>>,
        deleted: Vec<(Vec<String>, BindingTime)>,
        statuses: Vec<(String, String, BindingTime)>,
        gc_cutoffs: Vec<BindingTime>,
    }

    #[derive(Default)]
    struct RecordingStore(Mutex<StoreState>);

    impl BindingStore for RecordingStore {
        fn read_bindings_since(&self, _since: BindingTime) -> Result<Vec<Arc<Binding>>> {
            Ok(Vec::new())
        }

        fn replace_bindings(&self, bindings: &[Arc<Binding>]) -> Result<()> {
            self.0.lock().unwrap().replaced = bindings.to_vec();
            Ok(())
        }

        fn mark_deleted(&self, sql_digests: &[String], at: BindingTime) -> Result<u64> {
            self.0
                .lock()
                .unwrap()
                .deleted
                .push((sql_digests.to_vec(), at));
            Ok(sql_digests.len() as u64)
        }

        fn set_status(&self, sql_digest: &str, status: &str, at: BindingTime) -> Result<bool> {
            self.0
                .lock()
                .unwrap()
                .statuses
                .push((sql_digest.to_owned(), status.to_owned(), at));
            Ok(false)
        }

        fn gc_deleted_before(&self, cutoff: BindingTime) -> Result<u64> {
            self.0.lock().unwrap().gc_cutoffs.push(cutoff);
            Ok(0)
        }

        fn save_usage(
            &self,
            _sql_digest: &str,
            _plan_digest: &str,
            _used_at: BindingTime,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CacheState {
        reloads: usize,
        direct_sets: usize,
        direct_removes: usize,
    }

    #[derive(Default)]
    struct RecordingCache(Mutex<CacheState>);

    impl BindingCache for RecordingCache {
        fn MatchingBinding(
            &self,
            _current_db: &str,
            _no_db_digest: &str,
            _table_names: &[TableName],
        ) -> (Option<Arc<Binding>>, bool) {
            (None, false)
        }

        fn GetBinding(&self, _sql_digest: &str) -> Option<Arc<Binding>> {
            None
        }

        fn GetAllBindings(&self) -> Vec<Arc<Binding>> {
            Vec::new()
        }

        fn SetBinding(&self, _sql_digest: String, _binding: Arc<Binding>) -> Result<()> {
            self.0.lock().unwrap().direct_sets += 1;
            Ok(())
        }

        fn RemoveBinding(&self, _sql_digest: &str) {
            self.0.lock().unwrap().direct_removes += 1;
        }

        fn SetMemCapacity(&self, _capacity: i64) {}
        fn GetMemUsage(&self) -> i64 {
            0
        }
        fn GetMemCapacity(&self) -> i64 {
            0
        }
        fn Size(&self) -> usize {
            0
        }
        fn Close(&self) {}
    }

    impl BindingCacheUpdater for RecordingCache {
        fn LoadFromStorageToCache(&self, _full_load: bool, _from_remote: bool) -> Result<()> {
            self.0.lock().unwrap().reloads += 1;
            Ok(())
        }

        fn UpdateBindingUsageInfoToStorage(&self) -> Result<()> {
            Ok(())
        }
        fn LastUpdateTime(&self) -> BindingTime {
            BindingTime::default()
        }
    }

    struct Validator;

    impl BindingValidator for Validator {
        fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct SqlContext(Mutex<Vec<String>>);

    impl BindingValidator for SqlContext {
        fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
            Ok(())
        }
    }

    impl BindingSqlContext for SqlContext {
        fn execute(&self, sql: &str, _args: &[SqlValue]) -> Result<Vec<BindingRow>> {
            self.0.lock().unwrap().push(sql.to_owned());
            Ok(Vec::new())
        }

        fn plan_digest(&self, _schema: &str, _binding_sql: &str) -> Result<String> {
            Ok(String::new())
        }
    }

    fn operator() -> (bindingOperator, Arc<RecordingStore>, Arc<RecordingCache>) {
        let store = Arc::new(RecordingStore::default());
        let cache = Arc::new(RecordingCache::default());
        (
            bindingOperator {
                store: store.clone(),
                cache: cache.clone(),
                lease: std::time::Duration::from_secs(3),
            },
            store,
            cache,
        )
    }

    #[test]
    fn create_matches_go_metadata_and_reloads_cache() {
        let (operator, store, cache) = operator();
        operator
            .CreateBinding(
                &Validator,
                vec![Binding {
                    BindSQL: "select * from t".to_owned(),
                    Db: "TeSt".to_owned(),
                    CreateTime: BindingTime(1),
                    ..Binding::default()
                }],
            )
            .unwrap();

        let state = store.0.lock().unwrap();
        assert_eq!(state.replaced.len(), 1);
        assert!(state.replaced[0].CreateTime > BindingTime(1));
        assert_eq!(state.replaced[0].Db, "test");
        drop(state);
        let cache = cache.0.lock().unwrap();
        assert_eq!(cache.reloads, 1);
        assert_eq!(cache.direct_sets, 0);
    }

    #[test]
    fn empty_drop_is_rejected_like_go() {
        let (operator, _, _) = operator();
        assert_eq!(
            operator.DropBinding(&[]).unwrap_err().to_string(),
            "sql digest is empty"
        );
    }

    #[test]
    fn drop_and_unchanged_status_reload_instead_of_mutating_cache_directly() {
        let (operator, _, cache) = operator();
        assert_eq!(operator.DropBinding(&["digest".to_owned()]).unwrap(), 1);
        assert!(!operator.SetBindingStatus(StatusDisabled, "digest").unwrap());
        let cache = cache.0.lock().unwrap();
        assert_eq!(cache.reloads, 2);
        assert_eq!(cache.direct_removes, 0);
    }

    #[test]
    fn unsupported_status_matches_no_rows_like_go() {
        let (operator, store, cache) = operator();
        for invalid in ["", "deleted", "using", "arbitrary"] {
            assert!(!operator.SetBindingStatus(invalid, "digest").unwrap());
        }
        assert!(!operator.SetBindingStatus(StatusEnabled, "digest").unwrap());
        assert_eq!(store.0.lock().unwrap().statuses.len(), 1);
        assert_eq!(cache.0.lock().unwrap().reloads, 5);
    }

    #[test]
    fn gc_uses_ten_leases_and_saturates_large_durations() {
        let (mut operator, store, _) = operator();
        operator.lease = std::time::Duration::MAX;
        operator.GCBinding().unwrap();
        assert!(store.0.lock().unwrap().gc_cutoffs[0] < BindingTime::now());
    }

    #[test]
    fn lock_executes_the_go_lock_statement_without_arguments() {
        let context = SqlContext::default();
        crate::lockBindInfoTable(&context).unwrap();
        assert_eq!(
            context.0.lock().unwrap().as_slice(),
            [crate::LockBindInfoSQL]
        );
    }
}
