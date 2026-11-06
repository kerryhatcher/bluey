//! The `~/.bluey` folder: the settings file and a small SQLite database.
//!
//! The folder is made the first time any command runs, along with two files in it:
//!
//! - `config.yaml`, the settings. It starts with comments only. Nothing in it is needed yet,
//!   because every setting has a default.
//! - `bluey.db`, an empty SQLite database, for anything the program needs to remember later.
//!
//! Files that already exist are never touched. Only the missing ones are created.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const DIR_NAME: &str = ".bluey";
const CONFIG_NAME: &str = "config.yaml";
const DB_NAME: &str = "bluey.db";

/// What `config.yaml` starts as. It is valid YAML, so it parses as an empty settings file.
const CONFIG_TEMPLATE: &str = "\
# bluey settings. Every setting has a default, so this file can stay empty.
# An unknown setting is an error, so a typo shows up instead of being ignored.
{}
";

/// The settings in `config.yaml`. There are none yet, so any key in the file is rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {}

/// `~/.bluey`, from `$HOME`.
pub fn home_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).context("can't find ~/.bluey: HOME is not set")?;
    Ok(PathBuf::from(home).join(DIR_NAME))
}

/// Makes `~/.bluey` and its files if they are missing, and reads the settings.
pub fn ensure() -> Result<Config> {
    ensure_in(&home_dir()?)
}

/// Makes `dir` and the files in it if they are missing, and reads the settings. Anything already
/// there is left as it is.
pub fn ensure_in(dir: &Path) -> Result<Config> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let config = dir.join(CONFIG_NAME);
    if !config.exists() {
        fs::write(&config, CONFIG_TEMPLATE).with_context(|| format!("creating {}", config.display()))?;
        eprintln!("created {}", config.display());
    }

    let db = dir.join(DB_NAME);
    if !db.exists() {
        // Opening a database that doesn't exist creates an empty one.
        rusqlite::Connection::open(&db).with_context(|| format!("creating {}", db.display()))?;
        eprintln!("created {}", db.display());
    }

    // A file with a mistake in it is an error, not a silent default.
    let text = fs::read_to_string(&config).with_context(|| format!("reading {}", config.display()))?;
    serde_yaml_ng::from_str(&text).with_context(|| format!("reading {}", config.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bluey-home-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn creates_the_folder_and_both_files() {
        let dir = scratch("create");
        ensure_in(&dir).unwrap();
        assert!(dir.join(CONFIG_NAME).is_file());
        assert!(dir.join(DB_NAME).is_file());
        // The database is empty: no tables yet.
        let conn = rusqlite::Connection::open(dir.join(DB_NAME)).unwrap();
        let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
        assert_eq!(tables, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaves_existing_files_alone() {
        let dir = scratch("keep");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_NAME), "# mine\n{}\n").unwrap();
        ensure_in(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join(CONFIG_NAME)).unwrap(), "# mine\n{}\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unknown_settings() {
        let dir = scratch("unknown");
        ensure_in(&dir).unwrap();
        fs::write(dir.join(CONFIG_NAME), "colour: blue\n").unwrap();
        assert!(ensure_in(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
