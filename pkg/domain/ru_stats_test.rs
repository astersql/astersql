// RU（Request Unit，请求单元）增量行生成与 SQL 转义的单元测试。
//
// 校验按资源组 ID 匹配计算读写 RU 增量，以及资源组名中单引号的 SQL 转义。

// Copyright 2026 AsterSQL.
#[test]
/// 增量 = 最新累计 − 上期同 ID 累计；SQL 中 `'` 转义为 `''`。
fn canonical_ru_rows_use_id_matched_deltas_and_escape_sql_names() {
    use crate::ru_stats::{DailyRuStats, GroupRuStats, RuStats, generate_rows, generate_sql};
    use std::time::{Duration, UNIX_EPOCH};
    let end = UNIX_EPOCH + Duration::from_secs(86_400);
    let stats = RuStats {
        previous: Some(DailyRuStats {
            end_time: UNIX_EPOCH,
            groups: vec![GroupRuStats {
                id: 1,
                name: "rg'o".into(),
                read_ru: 2.0,
                write_ru: 3.0,
            }],
        }),
        latest: DailyRuStats {
            end_time: end,
            groups: vec![GroupRuStats {
                id: 1,
                name: "rg'o".into(),
                read_ru: 8.0,
                write_ru: 4.0,
            }],
        },
    };
    let rows = generate_rows(&stats, Duration::from_secs(86_400));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_ru, 7.0);
    assert!(generate_sql(&rows).contains("'rg''o'"));
}

#[test]
/// Go iterates every latest snapshot entry and truncates the RU value to int64 in SQL.
fn duplicate_latest_groups_and_integer_sql_match_go() {
    use crate::ru_stats::{DailyRuStats, GroupRuStats, RuStats, generate_rows, generate_sql};
    use std::time::{Duration, UNIX_EPOCH};

    let interval = Duration::from_secs(86_400);
    let end = UNIX_EPOCH + interval;
    let stats = RuStats {
        previous: None,
        latest: DailyRuStats {
            end_time: end,
            groups: vec![
                GroupRuStats {
                    id: 1,
                    name: "duplicate".into(),
                    read_ru: 1.25,
                    write_ru: 1.5,
                },
                GroupRuStats {
                    id: 1,
                    name: "duplicate".into(),
                    read_ru: 3.25,
                    write_ru: 1.5,
                },
            ],
        },
    };

    let rows = generate_rows(&stats, interval);
    assert_eq!(rows.len(), 2);
    assert!(generate_sql(&rows).contains("'duplicate',2)"));
    assert!(generate_sql(&rows).contains("'duplicate',4)"));
}

#[test]
fn writer_fetches_persists_inserts_and_then_skips_an_inserted_interval() {
    use crate::ru_stats::{
        GroupRuStats, RU_STATS_INTERVAL, RuStats, RuStatsBackend, RuStatsRow, RuStatsWriter,
    };
    use std::cell::{Cell, RefCell};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct Backend {
        inserted: Cell<bool>,
        fetches: Cell<usize>,
        persisted: RefCell<Option<RuStats>>,
        rows: RefCell<Vec<RuStatsRow>>,
    }
    impl RuStatsBackend for Backend {
        fn is_inserted(&self, _: SystemTime, _: SystemTime) -> Result<bool, String> {
            Ok(self.inserted.get())
        }
        fn load_latest(&self) -> Result<Option<RuStats>, String> {
            Ok(self.persisted.borrow().clone())
        }
        fn fetch_groups(&self) -> Result<Vec<GroupRuStats>, String> {
            self.fetches.set(self.fetches.get() + 1);
            Ok(vec![GroupRuStats {
                id: 1,
                name: "default".into(),
                read_ru: 200.0,
                write_ru: 150.0,
            }])
        }
        fn persist_latest(&self, stats: &RuStats) -> Result<(), String> {
            self.persisted.replace(Some(stats.clone()));
            Ok(())
        }
        fn insert_rows(&self, rows: &[RuStatsRow]) -> Result<(), String> {
            self.rows.borrow_mut().extend_from_slice(rows);
            Ok(())
        }
        fn delete_before(&self, _: SystemTime, _: usize) -> Result<usize, String> {
            Ok(0)
        }
    }

    let writer = RuStatsWriter {
        interval: RU_STATS_INTERVAL,
        start_time: UNIX_EPOCH + RU_STATS_INTERVAL,
        backend: Backend::default(),
    };
    let rows = writer.write().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_ru, 350.0);
    assert_eq!(writer.backend.fetches.get(), 1);
    assert_eq!(writer.backend.rows.borrow().as_slice(), rows.as_slice());

    writer.backend.inserted.set(true);
    assert!(writer.write().unwrap().is_empty());
    assert_eq!(writer.backend.fetches.get(), 1);
}

#[test]
fn canonical_utc_interval_alignment_matches_go_table() {
    use crate::ru_stats::get_last_expected_time;
    use std::time::{Duration, UNIX_EPOCH};

    let now = UNIX_EPOCH + Duration::from_secs(10 * 3_600 + 46 * 60 + 23);
    for (minutes, expected) in [
        (5, 10 * 3_600 + 45 * 60),
        (10, 10 * 3_600 + 40 * 60),
        (30, 10 * 3_600 + 30 * 60),
        (60, 10 * 3_600),
        (180, 9 * 3_600),
        (240, 8 * 3_600),
        (720, 0),
        (1_440, 0),
    ] {
        assert_eq!(
            get_last_expected_time(now, Duration::from_secs(minutes * 60)).unwrap(),
            UNIX_EPOCH + Duration::from_secs(expected)
        );
    }
}
