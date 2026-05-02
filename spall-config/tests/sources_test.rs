use spall_config::sources::{derive_name_from_filename, expand_tilde, scan_spec_dirs};
use std::path::PathBuf;
use tempfile::TempDir;

fn temp_config_dir() -> TempDir {
    TempDir::new().expect("temp dir")
}

fn write_file(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    std::fs::write(path, content).expect("write file");
}

#[test]
fn expand_tilde_home() {
    let expanded = expand_tilde("~/test");
    assert_ne!(expanded, PathBuf::from("~/test"));
    assert!(!expanded.to_string_lossy().starts_with('~'));
}

#[test]
fn expand_tilde_no_tilde() {
    let expanded = expand_tilde("/usr/local/bin");
    assert_eq!(expanded, PathBuf::from("/usr/local/bin"));
}

#[test]
fn derive_name_from_filename_json() {
    assert_eq!(
        derive_name_from_filename(PathBuf::from("/specs/pet_store.json").as_path()),
        Some("pet-store".to_string())
    );
}

#[test]
fn derive_name_from_filename_yaml() {
    assert_eq!(
        derive_name_from_filename(PathBuf::from("/specs/my-internal-api.yaml").as_path()),
        Some("my-internal-api".to_string())
    );
}

#[test]
fn derive_name_from_filename_no_stem() {
    assert_eq!(
        derive_name_from_filename(PathBuf::from("/").as_path()),
        None
    );
}

#[test]
fn scan_spec_dirs_finds_json_yaml_yml() {
    let tmp = temp_config_dir();
    write_file(&tmp.path().join("petstore.json"), r#"{"openapi":"3.0.0"}"#);
    write_file(&tmp.path().join("users.yaml"), "openapi: '3.0.0'");
    write_file(&tmp.path().join("orders.yml"), "openapi: '3.0.0'");
    write_file(&tmp.path().join("README.md"), "readme");

    let entries = scan_spec_dirs(&[tmp.path().to_path_buf()]).expect("scan should succeed");
    let names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"petstore".to_string()));
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
}

#[test]
fn scan_spec_dirs_skips_nonexistent() {
    let entries =
        scan_spec_dirs(&[PathBuf::from("/nonexistent/path")]).expect("scan should succeed");
    assert!(entries.is_empty());
}
