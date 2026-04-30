use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=build.rs");

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let dirty_suffix = if dirty { "-dirty" } else { "" };

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let timestamp = format_unix_utc(secs);

    let version = env!("CARGO_PKG_VERSION");
    let build_info = format!("{} ({}{} {})", version, sha, dirty_suffix, timestamp);

    println!("cargo:rustc-env=SKETCH_BUILD_INFO={}", build_info);
    println!("cargo:rustc-env=SKETCH_BUILD_SHA={}{}", sha, dirty_suffix);
    println!("cargo:rustc-env=SKETCH_BUILD_TIME={}", timestamp);

    // cargo:warning= is the only mechanism build scripts have to print
    // messages to the user; cargo always shows it.
    println!("cargo:warning=sketch build {}", build_info);
}

/// Render a UNIX timestamp as a UTC ISO-8601 date+time, e.g. "2026-04-30T14:23:09Z".
/// Self-contained so build.rs has no extra deps.
fn format_unix_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = days_to_ymd(days as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, m, s)
}

/// Convert days since 1970-01-01 to (year, month, day). Algorithm from
/// Howard Hinnant's date library.
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
