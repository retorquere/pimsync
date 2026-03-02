// Copyright 2026 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: ISC

//! Integration test: sync a vdir to a CalDAV server (xandikos) over a Unix socket.

use std::fmt::Write as _;
use std::fs;
use std::process::Command;
use std::time::Duration;

use rand::Rng;
use tempfile::TempDir;

fn random_string(len: usize) -> String {
    rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn minimal_icalendar(summary: &str) -> String {
    let uid = random_string(12);
    let mut entry = String::new();
    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//nl.whynothugo.pimsync//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n").unwrap();
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    write!(entry, "SUMMARY:{summary}\r\n").unwrap();
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");
    entry
}

/// Wait until the Unix socket exists, or panic after a timeout.
fn wait_for_socket(path: &std::path::Path, timeout: Duration) {
    let start = std::time::Instant::now();
    while !path.exists() {
        if start.elapsed() > timeout {
            panic!(
                "xandikos did not create socket at {} within {:?}",
                path.display(),
                timeout
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sync_vdir_to_xandikos_via_unix_socket() {
    let tmpdir = TempDir::new().unwrap();
    let base = tmpdir.path();

    let xandikos_data = base.join("xandikos_data");
    let vdir_root = base.join("vdir");
    let collection_dir = vdir_root.join("my-calendar");
    let status_dir = base.join("status");
    let socket_path = base.join("xandikos.sock");
    let config_path = base.join("pimsync.conf");

    fs::create_dir_all(&xandikos_data).unwrap();
    fs::create_dir_all(&collection_dir).unwrap();
    fs::create_dir_all(&status_dir).unwrap();

    let summary = format!("Test Event {}", random_string(8));
    let ical = minimal_icalendar(&summary);
    let event_path = collection_dir.join("event.ics");
    fs::write(&event_path, &ical).unwrap();

    let mut xandikos = Command::new("xandikos")
        .arg("-d")
        .arg(&xandikos_data)
        .arg("-l")
        .arg(&socket_path)
        .arg("--defaults")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start xandikos");

    wait_for_socket(&socket_path, Duration::from_secs(2));

    let config = format!(
        r#"status_path "{status}"

storage local {{
    type vdir/icalendar
    path "{vdir}"
    fileext ics
}}

storage remote {{
    type caldav
    url "http://localhost/user/calendars/"
    socket "{socket}"
}}

pair calendars {{
    storage_a local
    storage_b remote
    collections from a
    on_empty sync
    on_delete sync
}}
"#,
        status = status_dir.display(),
        vdir = vdir_root.display(),
        socket = socket_path.display(),
    );
    fs::write(&config_path, &config).unwrap();

    // Magic variable set by Cargo.
    let pimsync = env!("CARGO_BIN_EXE_pimsync");

    let output = Command::new(pimsync)
        .arg("-c")
        .arg(&config_path)
        .arg("-v")
        .arg("debug")
        .arg("sync")
        .output()
        .expect("failed to run pimsync");

    xandikos.kill().unwrap();
    let _ = xandikos.wait();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "pimsync sync failed (exit {:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code()
        );
    }

    let found = find_ics_with_summary(&xandikos_data, &summary);
    assert!(
        found,
        "expected to find an .ics file in xandikos data with SUMMARY:{summary}"
    );
}

/// Recursively search for any .ics file containing the given SUMMARY line.
fn find_ics_with_summary(dir: &std::path::Path, summary: &str) -> bool {
    let needle = format!("SUMMARY:{summary}");
    walk_dir_for_ics(dir, &needle)
}

fn walk_dir_for_ics(dir: &std::path::Path, needle: &str) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if walk_dir_for_ics(&path, needle) {
                return true;
            }
        } else if path.extension().is_some_and(|ext| ext == "ics")
            && let Ok(contents) = fs::read_to_string(&path)
            && contents.contains(needle)
        {
            return true;
        }
    }
    false
}
