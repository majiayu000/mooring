//! Where a launch starts when the folder it names is gone here: another machine's window
//! asks for a folder only this host can look at (docs/decisions/2026-11-14-one-window-many-hosts.md).
use fibo_core::model::Workspace;
use std::path::Path;

/// `cwd` when it is a folder here, else the space's own folder, else home; an empty or
/// relative one is left for the launch to turn down.
pub(super) fn start_dir(w: &Workspace, space: &str, cwd: String) -> String {
    start_dir_in(w, space, cwd, fibo_platform::dirs::home_str())
}

fn start_dir_in(w: &Workspace, space: &str, cwd: String, home: Option<String>) -> String {
    let dir = |d: &str| Path::new(d).is_absolute() && Path::new(d).is_dir();
    if cwd.is_empty() || !Path::new(&cwd).is_absolute() || dir(&cwd) {
        return cwd;
    }
    let own = (w.spaces.iter().find(|s| s.id == space)).map(|s| s.cwd.clone());
    own.filter(|d| dir(d))
        .or_else(|| home.filter(|d| dir(d)))
        .unwrap_or(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fibo_core::model::Space;

    #[test]
    fn a_gone_folder_starts_in_its_space_then_home() {
        let tmp = std::env::temp_dir();
        let here = tmp.to_string_lossy().into_owned();
        let gone = tmp
            .join("fibo-start-dir-gone")
            .to_string_lossy()
            .into_owned();
        let mut w = Workspace::default();
        let home = Some(here.clone());
        assert_eq!(
            start_dir_in(&w, "s", here.clone(), None),
            here,
            "a folder here stays"
        );
        assert_eq!(
            start_dir_in(&w, "s", gone.clone(), home.clone()),
            here,
            "else home"
        );
        assert_eq!(
            start_dir_in(&w, "s", gone.clone(), None),
            gone,
            "nothing better: as asked"
        );
        w.spaces.push(Space {
            id: "s".into(),
            name: "s".into(),
            cwd: "/".into(),
        });
        assert_eq!(
            start_dir_in(&w, "s", gone, home),
            "/",
            "its space's folder first"
        );
        assert_eq!(
            start_dir_in(&w, "s", "rel".into(), None),
            "rel",
            "turned down later"
        );
    }
}
