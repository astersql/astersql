// Copyright 2026 AsterSQL.

use super::inspection_profile::{
    MetricQueryRow, NewProfileBuilder, ProfileDataSource, ProfileResult, metricValue,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

struct EmptyDataSource;

impl ProfileDataSource for EmptyDataSource {
    fn execute_metric_sql(&self, _sql: &str) -> ProfileResult<Vec<MetricQueryRow>> {
        Ok(Vec::new())
    }

    fn metric_comment(&self, _metric_table: &str) -> ProfileResult<Option<String>> {
        Ok(None)
    }

    fn format_metric_time(&self, time: SystemTime) -> ProfileResult<String> {
        Ok(time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string())
    }
}

#[test]
fn metric_comment_formats_zero_duration_like_go() {
    let value = metricValue {
        count: 1,
        ..metricValue::default()
    };

    let comment = value.getComment();
    assert!(comment.contains("total_time: 0s\n"), "{comment}");
    assert!(comment.contains("avg_time: 0s\n"), "{comment}");
}

#[test]
fn metric_comment_preserves_go_millisecond_precision() {
    let value = metricValue {
        sum: 0.001_234_567,
        count: 1,
        ..metricValue::default()
    };

    let comment = value.getComment();
    assert!(comment.contains("total_time: 1.234567ms\n"), "{comment}");
}

#[test]
fn profile_header_preserves_negative_duration_like_go() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(2);
    let end = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let mut builder = NewProfileBuilder(Arc::new(EmptyDataSource), start, end, "sum").unwrap();

    builder.Collect().unwrap();

    assert!(
        builder.buffer.contains("Duration: -1s\\l"),
        "{}",
        builder.buffer
    );
}
