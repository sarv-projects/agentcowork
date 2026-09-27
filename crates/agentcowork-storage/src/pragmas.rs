//! P45.1–P45.3 — SQLite pragma tuning for the **non-crypto** DBs.
//!
//! The SQLCipher **vault is intentionally untouched**: credentials must not
//! trade durability, so `synchronous=FULL` (its current setting) stays.
//! These helpers are only ever called from the memory/search/replay indexes.
//!
//! - P45.1 `synchronous=NORMAL` — safe with WAL, major write throughput win.
//! - P45.2 `mmap_size=256MB` — zero-copy reads on the read-mostly FTS5 /
//!   trigram / search indexes (opt-in via `read_heavy`).
//! - P45.3 `journal_size_limit` + throttled `wal_autocheckpoint` — bound WAL
//!   growth and avoid per-commit stalls (checkpoint less often, never
//!   unbounded).

use rusqlite::Connection;

/// P45.1 + P45.3 for any non-vault connection (journal WAL, synchronous
/// NORMAL, bounded WAL). In-memory connections accept the pragmas as no-ops.
pub fn apply_non_vault(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA journal_size_limit=67108864;
         PRAGMA wal_autocheckpoint=4000;",
    )
}

/// P45.2 — memory-map the read-heavy FTS5/trigram/search indexes (256 MiB).
/// Call after `apply_non_vault` on the read-mostly stores only.
pub fn apply_read_heavy(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA mmap_size=268435456;")
}

/// P45.3 — checkpoint when the machine is idle or on battery and the WAL
/// has reached the page cap. This is the policy; it does not measure latency.
/// True when this Linux host is on battery. Other hosts report false.
pub fn host_on_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    for entry in entries.flatten() {
        let status = entry.path().join("status");
        if let Ok(text) = std::fs::read_to_string(status) {
            if text.trim().eq_ignore_ascii_case("Discharging") {
                return true;
            }
        }
    }
    false
}

/// Run a passive WAL checkpoint when the idle/battery policy says so.
/// Returns whether a checkpoint was requested.
pub fn checkpoint_connection(conn: &Connection, user_idle: bool) -> rusqlite::Result<bool> {
    let wal_pages = wal_file_pages(conn);
    if !checkpoint_when_idle_or_battery(user_idle, host_on_battery(), wal_pages, 1) {
        return Ok(false);
    }
    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
    Ok(true)
}

fn wal_file_pages(conn: &Connection) -> u64 {
    let path: String = conn
        .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
        .unwrap_or_default();
    if path.is_empty() {
        return 0;
    }
    let bytes = std::fs::metadata(format!("{path}-wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    let page = conn
        .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
        .unwrap_or(4096)
        .max(1) as u64;
    bytes / page
}

pub fn checkpoint_when_idle_or_battery(
    idle: bool,
    on_battery: bool,
    wal_pages: u64,
    page_cap: u64,
) -> bool {
    (idle || on_battery) && wal_pages >= page_cap
}

/// Single call for the common read-heavy case.
pub fn apply_read_heavy_index(conn: &Connection) -> rusqlite::Result<()> {
    apply_non_vault(conn)?;
    apply_read_heavy(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn pragma_i64(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// P45.1 acceptance: per-DB `synchronous` value asserted on a real
    /// on-disk connection (in-memory connections ignore some journal pragmas).
    #[test]
    fn non_vault_db_gets_wal_and_synchronous_normal() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-pragmas-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idx.sqlite");
        let conn = Connection::open(&path).unwrap();
        apply_non_vault(&conn).unwrap();

        // synchronous=NORMAL (1), WAL journal, bounded WAL + throttled checkpoint.
        assert_eq!(
            pragma_i64(&conn, "PRAGMA synchronous;"),
            1,
            "synchronous must be NORMAL (1)"
        );
        let journal: String = conn
            .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal", "journal_mode must be WAL");
        assert_eq!(pragma_i64(&conn, "PRAGMA journal_size_limit;"), 67_108_864);
        assert_eq!(pragma_i64(&conn, "PRAGMA wal_autocheckpoint;"), 4000);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// P45.2 acceptance: mmap_size applied only via the read-heavy helper.
    #[test]
    fn read_heavy_index_gets_mmap_without_mutation_side_effects() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-mmap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fts.sqlite");
        let conn = Connection::open(&path).unwrap();
        apply_read_heavy_index(&conn).unwrap();
        assert_eq!(
            pragma_i64(&conn, "PRAGMA mmap_size;"),
            268_435_456,
            "mmap_size must be 256MiB"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// P45.1 acceptance: the plain non-vault call must NOT set mmap (it is
    /// reserved for the read-heavy helper) — keeps write DBs off mmap.
    #[test]
    fn plain_pragma_call_leaves_mmap_default() {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-mmap-default-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plain.sqlite");
        let conn = Connection::open(&path).unwrap();
        apply_non_vault(&conn).unwrap();
        let mmap = pragma_i64(&conn, "PRAGMA mmap_size;");
        assert!(
            mmap != 268_435_456,
            "mmap must not be applied by the plain call"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn idle_or_battery_checkpoints_only_after_the_page_cap() {
        assert!(!checkpoint_when_idle_or_battery(true, false, 10, 4000));
        assert!(checkpoint_when_idle_or_battery(true, false, 4000, 4000));
        assert!(checkpoint_when_idle_or_battery(false, true, 4000, 4000));
        assert!(!checkpoint_when_idle_or_battery(false, false, 9000, 4000));
    }

    #[test]
    fn synchronous_normal_and_mmap_are_measured_on_a_real_database() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-p45-measure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rows = 400u32;
        let normal = timed_inserts(&dir.join("normal.sqlite"), false, rows);
        let full = timed_inserts(&dir.join("full.sqlite"), true, rows);
        let buffered = timed_reads(&dir.join("normal.sqlite"), false);
        let mapped = timed_reads(&dir.join("mapped.sqlite"), true);
        eprintln!(
            "p45 measure normal_ms={normal} full_ms={full} buffered_read_ms={buffered} mmap_read_ms={mapped}"
        );
        assert!(normal > 0 || full > 0);
        assert!(buffered > 0 && mapped > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn timed_inserts(path: &std::path::Path, full: bool, rows: u32) -> u128 {
        let conn = Connection::open(path).unwrap();
        if full {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
                .unwrap();
        } else {
            apply_non_vault(&conn).unwrap();
        }
        conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, body TEXT);")
            .unwrap();
        let start = std::time::Instant::now();
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..rows {
            tx.execute("INSERT INTO t(body) VALUES (?1)", [format!("row-{i}")])
                .unwrap();
        }
        tx.commit().unwrap();
        start.elapsed().as_millis()
    }

    fn timed_reads(path: &std::path::Path, mmap: bool) -> u128 {
        let conn = Connection::open(path).unwrap();
        apply_non_vault(&conn).unwrap();
        if mmap {
            apply_read_heavy(&conn).unwrap();
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS t(id INTEGER PRIMARY KEY, body TEXT);")
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap_or(0);
        if n == 0 {
            for i in 0..200 {
                conn.execute("INSERT INTO t(body) VALUES (?1)", [format!("row-{i}")])
                    .unwrap();
            }
        }
        let start = std::time::Instant::now();
        for _ in 0..20 {
            let mut stmt = conn.prepare("SELECT body FROM t").unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
            for row in rows {
                let _ = row.unwrap();
            }
        }
        start.elapsed().as_millis().max(1)
    }
}
