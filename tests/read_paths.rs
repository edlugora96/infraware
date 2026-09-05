//! v2 list/get output and read-only behavior.
use infraware::{EXIT_FAILURE, run};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("infraware-read-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn exec(p: &Path, a: &[&str]) -> (u8, String, String) {
    let _lock = cwd_lock().lock().unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(p).unwrap();
    let mut o = Vec::new();
    let mut e = Vec::new();
    let args: Vec<String> = a.iter().map(|x| x.to_string()).collect();
    let s = run(args, &mut o, &mut e);
    std::env::set_current_dir(old).unwrap();
    (
        s,
        String::from_utf8(o).unwrap(),
        String::from_utf8(e).unwrap(),
    )
}
#[test]
fn list_and_get_missing_store_fail_closed() {
    let p = root();
    for a in [&["list"][..], &["get", "1"][..], &["status"][..]] {
        let (s, o, e) = exec(&p, a);
        assert_eq!(s, EXIT_FAILURE);
        assert!(o.is_empty());
        assert_eq!(e.lines().count(), 1);
        assert!(e.starts_with("error:"));
    }
    assert!(!p.join(".infraware").exists());
    let _ = std::fs::remove_dir_all(p);
}
#[test]
fn reads_missing_store_are_read_only() {
    let p = root();
    let (s, o, e) = exec(&p, &["list"]);
    assert_eq!(s, EXIT_FAILURE);
    assert!(o.is_empty());
    assert_eq!(e.lines().count(), 1);
    assert!(e.starts_with("error:"));
    assert!(!p.join(".infraware").exists());
    let _ = std::fs::remove_dir_all(p);
}
