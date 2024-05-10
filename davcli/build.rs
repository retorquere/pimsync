// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::process::Command;

fn main() {
    // TODO: try reading from a file instead of env var
    if std::env::var("DAVCLI_VERSION").is_err() {
        let version = Command::new("git")
            .args(["describe", "--tags"])
            .output()
            .map(|o| {
                if o.status.success() {
                    String::from_utf8_lossy(&o.stdout).trim().to_owned()
                } else {
                    String::from("unversioned") // git exited non-zero
                }
            })
            .unwrap_or(String::from("unknown")); // failed to run git

        println!("cargo:rustc-env=DAVCLI_VERSION={version}");
    }
}
