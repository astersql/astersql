// Copyright 2026 AsterSQL.

use crate::storage_windows::{GetStorageSizeWith, SameDisk};
use std::io;

#[test]
fn get_storage_size_passes_the_original_path_to_win32() {
    let path = r"C:\a path\that need not exist";
    let size = GetStorageSizeWith(path, |actual, available, capacity| {
        assert_eq!(actual, path);
        *available = 25;
        *capacity = 100;
        Ok(())
    })
    .expect("successful GetDiskFreeSpaceExW call");

    assert_eq!(size.Capacity, 100);
    assert_eq!(size.Available, 25);
}

#[test]
fn get_storage_size_annotates_the_win32_error_with_the_path() {
    let path = r"Z:\missing";
    let error = GetStorageSizeWith(path, |_, _, _| Err(io::Error::from_raw_os_error(3)))
        .expect_err("failed GetDiskFreeSpaceExW call");

    assert!(
        error
            .Message
            .contains("cannot get disk capacity at Z:\\missing")
    );
    assert!(error.Message.contains("os error 3"));
}

#[test]
fn same_disk_keeps_the_go_windows_placeholder_contract() {
    assert!(!SameDisk(r"C:\one", r"C:\two").expect("placeholder is infallible"));
    assert!(!SameDisk(r"C:\one", r"D:\two").expect("placeholder is infallible"));
}
