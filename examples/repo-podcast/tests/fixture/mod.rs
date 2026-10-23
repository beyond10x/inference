//! A test directory under the target's temporary directory, removed when its guard drops, so a
//! fixture is gone after a failing assertion as well as after a passing run.

use std::ops::Deref;
use std::path::{Path, PathBuf};

/// The directory `CARGO_TARGET_TMPDIR/<name>`, emptied when made and removed on drop.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the test directory is made");
        Self(dir)
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
