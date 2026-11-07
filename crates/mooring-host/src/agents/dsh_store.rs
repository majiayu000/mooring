//! Select the current generation, decode complete zstd frames, and publish an owned
//! snapshot. Partial writes leave the last good snapshot intact; the source is never repaired.
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Once,
    time::{Duration, SystemTime},
};

pub(super) fn start(home: &Path) {
    static START: Once = Once::new();
    let root = super::home_dir(home, "DSH_HOME", ".dsh").join("sessions");
    let out = super::mirror::dir(home, mooring_core::model::Agent::Dsh);
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("mirror-dsh".into())
            .spawn(move || {
                let mut store = Store::default();
                let mut error = String::new();
                loop {
                    if root.is_dir() {
                        match store.poll(&root, &out) {
                            Ok(_) => error.clear(),
                            Err(e) => {
                                let next = e.to_string();
                                if next != error {
                                    eprintln!("mooring: DSH 读不了：{next}");
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

#[derive(Default)]
pub(super) struct Store {
    seen: HashMap<PathBuf, ((u64, SystemTime), PathBuf)>,
}
fn dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    fs::read_dir(root)?
        .filter_map(|r| match r {
            Ok(e) => match e.file_type() {
                Ok(t) if t.is_dir() => Some(Ok(e.path())),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            },
            Err(e) => Some(Err(e)),
        })
        .collect()
}
fn generation(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let plain = name.strip_suffix(".zstd").unwrap_or(name);
    let n = plain.strip_prefix("session.v")?.strip_suffix(".jsonl")?;
    let version = n.parse::<u32>().ok()?;
    (version.to_string() == n).then_some(version)
}
impl Store {
    pub(super) fn poll(&mut self, root: &Path, out: &Path) -> io::Result<Vec<PathBuf>> {
        let mut changed = Vec::new();
        let mut keep = HashSet::new();
        let mut error = None;
        for project in dirs(root)? {
            for session in dirs(&project)? {
                // One partially written or unsupported session cannot starve other sessions.
                match self.session(&session, out, &mut changed) {
                    Ok(Some(path)) => {
                        keep.insert(path);
                    }
                    Ok(None) => (),
                    Err(e) => {
                        error.get_or_insert(e);
                    }
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        if out.is_dir() {
            for entry in fs::read_dir(out)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "jsonl") && !keep.contains(&path) {
                    fs::remove_file(path)?;
                }
            }
        }
        self.seen.retain(|_, (_, target)| keep.contains(target));
        Ok(changed)
    }
    fn session(
        &mut self,
        session: &Path,
        out: &Path,
        changed: &mut Vec<PathBuf>,
    ) -> io::Result<Option<PathBuf>> {
        let mut generations = Vec::new();
        for entry in fs::read_dir(session)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(version) = generation(&entry.path()) {
                    generations.push((version, entry.path()));
                }
            }
        }
        generations.sort();
        let Some((version, path)) = generations.last() else {
            return Ok(None);
        };
        let meta = fs::metadata(path)?;
        let modified = meta.modified()?;
        if SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age > Duration::from_secs(30 * 86400))
        {
            return Ok(None);
        }
        if *version != 4 {
            return Err(io::Error::other(format!(
                "不支持 DSH session v{version}：{}",
                path.display()
            )));
        }
        if generations.iter().filter(|(v, _)| v == version).count() != 1 {
            return Err(io::Error::other("DSH 同一代有多个日志，无法确定来源"));
        }
        let stamp = (meta.len(), modified);
        if let Some((old, target)) = self.seen.get(path) {
            if *old == stamp && target.is_file() {
                return Ok(Some(target.clone()));
            }
        }
        let bytes = decode(path)?;
        let (id, normalized) = normalize(&bytes)?;
        fs::create_dir_all(out)?;
        let name = id.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let target = out.join(format!("{name}.jsonl"));
        if fs::read(&target).ok().as_deref() != Some(&normalized) {
            let temp = target.with_extension("tmp");
            fs::write(&temp, &normalized)?;
            fs::rename(temp, &target)?;
            changed.push(target.clone());
        }
        self.seen.insert(path.clone(), (stamp, target.clone()));
        Ok(Some(target))
    }
}
fn decode(path: &Path) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "zstd") {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut bytes = Vec::new();
    reader.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(io::Error::other("DSH 会话解压超过 64 MiB，暂停读取"));
    }
    Ok(bytes)
}
fn normalize(bytes: &[u8]) -> io::Result<(String, Vec<u8>)> {
    if !bytes.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "DSH 日志还未写完",
        ));
    }
    let mut rows = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(serde_json::from_slice::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let head = rows
        .first()
        .ok_or_else(|| io::Error::other("DSH 缺少 header"))?;
    if head["type"] != "session" || head["version"] != 4 {
        return Err(io::Error::other("DSH header 不是 v4 session"));
    }
    let id = head["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| io::Error::other("DSH 缺少 session id"))?
        .to_owned();
    let seed = if head["isSeeded"] == true {
        rows.iter()
            .rposition(|v| v["type"] == "session/end-seed" && v["data"]["inherited"] == true)
            .ok_or_else(|| io::Error::other("DSH 继承记录尚未写完"))?
    } else {
        0
    };
    let mut out = Vec::new();
    for (i, v) in rows.iter_mut().enumerate() {
        if i > 0 && i <= seed {
            continue;
        }
        // Packed model stream chunks are not session events and carry no turn facts.
        if !v.is_object() || v["type"].as_str().is_none() {
            continue;
        }
        let ms = if i == 0 {
            v["createdAt"].as_u64()
        } else {
            v["time"].as_u64()
        };
        v["timestamp"] = (ms.unwrap_or(0) / 1000).into();
        out.extend(serde_json::to_vec(v)?);
        out.push(b'\n');
    }
    Ok((id, out))
}

#[cfg(test)]
#[path = "dsh_tests.rs"]
mod tests;
