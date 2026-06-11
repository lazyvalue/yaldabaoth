//! Canonical on-disk locations for yalda's durable state.
//!
//! Everything yalda persists — the per-session WALs, the persisted session
//! list, workspace/preferences, the stable client id, and logs — lives under a
//! single home: `~/.yalda`. This is durable, user-owned storage, deliberately
//! NOT the OS cache dir: `~/Library/Caches` on macOS (and `~/.cache` on Linux)
//! is *purgeable* — the OS can evict it under disk pressure and cleaner tools
//! wipe it, which would silently drop agent-session history (ADR-0018).
//!
//! Config stays at `~/.config/yalda` (XDG, `config.rs`); runtime sockets stay
//! in `/tmp` (`session_proto::socket_path`). A one-time
//! [`migrate_legacy_cache_dir`] relocates state written by older builds under
//! `<cache_dir>/yalda` into `~/.yalda`, so upgrading loses nothing.

use std::path::PathBuf;

/// The single durable home for yalda state: `~/.yalda`. `None` only if the
/// user's home directory can't be resolved (then callers fall back to their
/// existing `Option`-`None` behavior — same as a first run).
pub fn yalda_home() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".yalda"))
}

/// Legacy location used by builds before the `~/.yalda` move:
/// `<cache_dir>/yalda` (e.g. `~/Library/Caches/yalda`).
fn legacy_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("yalda"))
}

/// One-time migration: move every entry from the legacy `<cache_dir>/yalda`
/// into `~/.yalda`, skipping any name that already exists in the new home (so
/// a re-run, or a partially-migrated state, never clobbers newer data).
///
/// Best-effort and idempotent: a `rename` failure (e.g. EXDEV across mounts) is
/// logged and leaves that entry in the legacy dir rather than losing it — the
/// app then simply starts that file fresh, exactly as on a first run. Run once
/// at process startup BEFORE any state path is read. Returns the number of
/// entries moved.
pub fn migrate_legacy_cache_dir() -> usize {
    let (Some(new_home), Some(old)) = (yalda_home(), legacy_cache_dir()) else {
        return 0;
    };
    if old == new_home {
        return 0;
    }
    let moved = migrate_dir(&old, &new_home);
    if moved > 0 {
        eprintln!(
            "[yalda] migrated {moved} state entr{} from {} to {}",
            if moved == 1 { "y" } else { "ies" },
            old.display(),
            new_home.display()
        );
    }
    moved
}

/// Move every entry from `old` into `new_home`, skipping names that already
/// exist in `new_home` (never clobber). A failed `rename` (e.g. EXDEV) is
/// logged and the entry left in place — no data loss, just an un-migrated file
/// that starts fresh. Drops `old` if it ends up empty. The hermetic core of
/// [`migrate_legacy_cache_dir`]; takes explicit dirs so it's unit-testable
/// without writing under the real `~`.
fn migrate_dir(old: &std::path::Path, new_home: &std::path::Path) -> usize {
    if !old.is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(old) else {
        return 0;
    };
    if std::fs::create_dir_all(new_home).is_err() {
        return 0;
    }
    let mut moved = 0;
    for entry in entries.flatten() {
        let from = entry.path();
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = new_home.join(name);
        if to.exists() {
            continue; // new home already owns this name — never clobber
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => moved += 1,
            Err(e) => eprintln!(
                "[yalda] could not migrate {} -> {}: {e} (left in place)",
                from.display(),
                to.display()
            ),
        }
    }
    let _ = std::fs::remove_dir(old); // best-effort; non-empty → left
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yalda_home_is_dot_yalda_under_home() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(yalda_home(), Some(home.join(".yalda")));
        }
    }

    /// Migration moves a legacy entry into a fresh `~/.yalda` but never
    /// clobbers a name the new home already has. Driven with explicit dirs so
    /// it doesn't touch the real `~`.
    #[test]
    fn migrate_moves_without_clobbering() {
        // Hand-rolled migration over explicit dirs (mirrors the public fn but
        // parameterized, so the test stays hermetic — no real HOME writes).
        let base =
            std::env::temp_dir().join(format!("yalda_paths_{}_{}", std::process::id(), line!()));
        let old = base.join("Caches").join("yalda");
        let new = base.join(".yalda");
        std::fs::create_dir_all(old.join("wal")).unwrap();
        std::fs::write(old.join("wal").join("s1.log"), b"hist").unwrap();
        std::fs::write(old.join("preferences.json"), b"{}").unwrap();
        // New home already owns preferences.json with NEWER content — must win.
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("preferences.json"), b"NEW").unwrap();

        let moved = migrate_dir(&old, &new);

        // wal/ moved wholesale (new didn't have it); preferences.json NOT
        // clobbered (new kept its own).
        assert!(new.join("wal").join("s1.log").exists(), "wal migrated");
        assert_eq!(
            std::fs::read_to_string(new.join("preferences.json")).unwrap(),
            "NEW",
            "existing new-home file is never overwritten"
        );
        assert_eq!(moved, 1, "only the non-conflicting entry counts as moved");
        std::fs::remove_dir_all(&base).ok();
    }
}
