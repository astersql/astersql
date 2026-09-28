// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/filter_test.go`.
//!
//! 验证 PiTRIdTracker 空状态、AddDB/TrackTableId 与 Contains* 组合语义。
//! TrackTableId 隐式登记 db，使 ContainsDB(3) 在未显式 AddDB 时也为真。

use crate::filter::NewPiTRIdTracker;

#[test]
fn test_pitr_table_tracker() {
    // test new tracker
    // 新建跟踪器各集合应为空。
    {
        let tracker = NewPiTRIdTracker();
        assert!(tracker.DBIds.is_empty());
        assert!(tracker.TableIdToDBIds.is_empty());
    }

    // test update and contains table
    // 交叉验证：同表不同库、同库不同表均需精确匹配。
    {
        let mut tracker = NewPiTRIdTracker();
        tracker.AddDB(1);
        tracker.TrackTableId(1, 100);
        tracker.AddDB(2);
        assert!(tracker.ContainsDB(1));
        assert!(tracker.ContainsDB(2));
        assert!(tracker.ContainsDBAndTableId(1, 100));
        assert!(!tracker.ContainsDBAndTableId(1, 101));
        // 库 2 未登记表 100，即使表 id 已在别的库出现也不应命中。
        assert!(!tracker.ContainsDBAndTableId(2, 100));

        tracker.TrackTableId(1, 101);
        tracker.TrackTableId(2, 200);
        assert!(tracker.ContainsDBAndTableId(1, 100));
        assert!(tracker.ContainsDBAndTableId(1, 101));
        assert!(tracker.ContainsDBAndTableId(2, 200));

        // TrackTableId(3, 300) 同时插入 DBIds{3}。
        tracker.TrackTableId(3, 300);
        assert!(tracker.ContainsDB(3));
        assert!(tracker.ContainsDBAndTableId(3, 300));
    }
}
