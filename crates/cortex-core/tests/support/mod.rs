use cortex_core::{OpenMode, ProjectKey, Store, StoreChannel};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf};
pub struct Project(pub PathBuf);
impl Project {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cortex-core-test-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    pub fn key(&self) -> ProjectKey {
        ProjectKey::new(&self.0, StoreChannel::Dev).unwrap()
    }
    #[allow(dead_code)]
    pub fn open(&self) -> Store {
        Store::open(self.key(), OpenMode::Initialize).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[allow(dead_code)]
pub fn count(store: &Store, table: &str) -> i64 {
    store
        .read(|db| Ok(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?))
        .unwrap()
}
