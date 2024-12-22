use std::process::Command;

fn main() {
    if std::env::var("PIMSYNC_VERSION").is_err() {
        let version = "$Format:%(describe)$"; // Replaced by git-archive.
        let version = if version.starts_with('$') {
            match Command::new("git").args(["describe", "--tags"]).output() {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                Ok(o) => panic!("git-describe exited non-zero: {}", o.status),
                Err(err) => panic!("failed to execute git-describe: {err}"),
            }
        } else {
            String::from(version)
        };

        let version = version.strip_prefix('v').unwrap_or(&version);
        println!("cargo:rustc-env=PIMSYNC_VERSION={version}");
    }
}
