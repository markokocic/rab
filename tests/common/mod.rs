/// Create a temporary directory with a unique name.
pub fn tmp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Write a JSON string to a file, creating parent directories as needed.
pub fn write_file(path: &std::path::Path, json: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, json).unwrap();
}
