//! Loading and saving the JSON files in %APPDATA%\TodoWidget.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{ItemFile, Settings};

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open() -> Self {
        // TODO_WIDGET_DATA overrides the folder, for testing without touching real data.
        let dir = match std::env::var_os("TODO_WIDGET_DATA") {
            Some(d) => PathBuf::from(d),
            None => std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("TodoWidget"),
        };
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn load_tasks(&self) -> ItemFile {
        load_json(&self.path("tasks.json"))
    }

    pub fn load_archive(&self) -> ItemFile {
        load_json(&self.path("archive.json"))
    }

    pub fn load_settings(&self) -> Settings {
        load_json(&self.path("settings.json"))
    }

    pub fn save_tasks(&self, f: &ItemFile) {
        save_json(&self.path("tasks.json"), f);
    }

    pub fn save_archive(&self, f: &ItemFile) {
        save_json(&self.path("archive.json"), f);
    }

    pub fn save_settings(&self, s: &Settings) {
        save_json(&self.path("settings.json"), s);
    }
}

/// Missing file: default. Unreadable or invalid file: moved aside, then default,
/// so a bad file is never silently overwritten by the next save.
fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(bytes) = fs::read(path) else {
        return T::default();
    };
    // Editors such as Notepad may save UTF-8 with a byte-order mark.
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match serde_json::from_slice(json) {
        Ok(v) => v,
        Err(_) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            let aside = path.with_file_name(format!("{stem}.corrupt-{stamp}.json"));
            let _ = fs::rename(path, aside);
            T::default()
        }
    }
}

/// Writes to a temp file, flushes it to disk, then renames it over the target.
fn save_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_vec_pretty(value) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    let written = fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(&json)?;
        f.sync_all()
    });
    if written.is_ok() {
        let _ = fs::rename(&tmp, path);
    } else {
        let _ = fs::remove_file(&tmp);
    }
}
