//! Read-only Hermes snapshots. The database can rewrite/delete messages: replace the
//! owned JSONL atomically so the existing inode-aware index discards withdrawn replies.
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::Once,
    time::Duration,
};

pub(super) fn start(home: &Path) {
    static START: Once = Once::new();
    let source = super::home_dir(home, "HERMES_HOME", ".hermes").join("state.db");
    let out = super::mirror::dir(home, mooring_core::model::Agent::Hermes);
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("mirror-hermes".into())
            .spawn(move || {
                let mut previous = None;
                let mut error = String::new();
                loop {
                    let stamp = revision(&source);
                    if stamp.is_some() && (previous != stamp || !error.is_empty()) {
                        match snapshot(&source, &out) {
                            Ok(_) => {
                                previous = stamp;
                                error.clear();
                            }
                            Err(e) => {
                                let next = e.to_string();
                                if next != error {
                                    eprintln!("mooring: Hermes 读不了：{next}");
                                }
                                error = next;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                }
            });
    });
}

fn revision(path: &Path) -> Option<Vec<(u64, std::time::SystemTime)>> {
    let mut out = Vec::new();
    for file in [
        path.to_owned(),
        PathBuf::from(format!("{}-wal", path.display())),
    ] {
        match fs::metadata(&file) {
            Ok(m) => out.push((m.len(), m.modified().ok()?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound && file != path => (),
            Err(_) => return None,
        }
    }
    Some(out)
}

pub(super) fn snapshot(path: &Path, out: &Path) -> io::Result<Vec<PathBuf>> {
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    db.busy_timeout(Duration::from_millis(50))
        .map_err(io::Error::other)?;
    let tx = db.transaction().map_err(io::Error::other)?;
    // Only local interactive sources and explicitly identified children. Telegram, cron
    // and API-server traffic must not become phantom desktop terminals.
    // Hermes frontends add optional cwd/visibility columns to the same core schema.
    // Missing metadata supplies no fact: do not invent a cwd or a visibility flag.
    let has = |table: &str, column: &str| -> io::Result<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
            [table, column],
            |r| r.get(0),
        )
        .map_err(io::Error::other)
    };
    let cwd = if has("sessions", "cwd")? {
        "COALESCE(cwd,'')"
    } else {
        "''"
    };
    let visible = if has("messages", "active")? {
        if has("messages", "compacted")? {
            "(COALESCE(active,1) != 0 OR COALESCE(compacted,0) != 0)"
        } else {
            "COALESCE(active,1) != 0"
        }
    } else {
        "1"
    };
    let mut sessions = tx.prepare(&format!("SELECT id, {cwd}, source, parent_session_id, title, started_at FROM sessions WHERE source IN ('cli','tui','subagent') AND MAX(COALESCE(ended_at,started_at),COALESCE((SELECT MAX(timestamp) FROM messages WHERE session_id=sessions.id),0)) > ?1 ORDER BY id")).map_err(io::Error::other)?;
    let since = mooring_core::content::now().saturating_sub(30 * 86400);
    let rows = sessions.query_map([since as i64], |r| Ok(json!({"type":"session", "session_id":r.get::<_,String>(0)?, "cwd":r.get::<_,String>(1)?, "source":r.get::<_,String>(2)?, "parent_session_id":r.get::<_,Option<String>>(3)?, "title":r.get::<_,Option<String>>(4)?, "timestamp":r.get::<_,f64>(5)?.max(0.) as u64}))).map_err(io::Error::other)?;
    let mut messages = tx.prepare(&format!("SELECT id,role,content,tool_calls,tool_call_id,finish_reason,timestamp FROM messages WHERE session_id=?1 AND {visible} ORDER BY timestamp,id")).map_err(io::Error::other)?;
    fs::create_dir_all(out)?;
    let mut keep = HashSet::new();
    let mut changed = Vec::new();
    for row in rows {
        let header = row.map_err(io::Error::other)?;
        let id = header["session_id"].as_str().unwrap();
        // Encode bytes instead of replacing punctuation: distinct source IDs never collide.
        let file = out.join(format!(
            "{}.jsonl",
            id.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        keep.insert(file.clone());
        let mut bytes = serde_json::to_vec(&header)?;
        bytes.push(b'\n');
        let records = messages.query_map([id], |r| {
            let tools: Option<String> = r.get(3)?;
            let tools = tools.map(|t|serde_json::from_str::<Value>(&t)).transpose().map_err(|e|rusqlite::Error::FromSqlConversionFailure(3,rusqlite::types::Type::Text,Box::new(e)))?;
            Ok(json!({"id":r.get::<_,i64>(0)?.to_string(), "role":r.get::<_,String>(1)?, "content":r.get::<_,Option<String>>(2)?, "tool_calls":tools, "tool_call_id":r.get::<_,Option<String>>(4)?, "finish_reason":r.get::<_,Option<String>>(5)?, "timestamp":r.get::<_,f64>(6)?}))
        }).map_err(io::Error::other)?;
        for record in records {
            let mut record = record.map_err(io::Error::other)?;
            // Parser's common clock accepts integer seconds.
            record["timestamp"] =
                (record["timestamp"].as_f64().unwrap_or(0.).max(0.) as u64).into();
            bytes.extend(serde_json::to_vec(&record)?);
            bytes.push(b'\n');
        }
        // Latest explicit title wins after the first prompt's automatic title.
        bytes.extend(serde_json::to_vec(&header)?);
        bytes.push(b'\n');
        if fs::read(&file).ok().as_deref() != Some(&bytes) {
            let temp = file.with_extension("tmp");
            fs::write(&temp, &bytes)?;
            fs::rename(&temp, &file)?;
            changed.push(file);
        }
    }
    // Source deletions/expiry remove only files in our own mirror, never Hermes' store.
    for entry in fs::read_dir(out)? {
        let file = entry?.path();
        if file.extension().is_some_and(|e| e == "jsonl") && !keep.contains(&file) {
            fs::remove_file(file)?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
#[path = "hermes_tests.rs"]
mod tests;
