//! Informational CLI responses are handled before parsing or filesystem access.

use infraware::{EXIT_USAGE, run};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static CWD_LOCK: Mutex<()> = Mutex::new(());

const HELP: &str = "Usage: infraware <COMMAND>\n\nCommands:\n  ingest <TASK_TEXT> <RULE_TEXT>\n  query <TASK_TEXT>\n  list [LIMIT] [OFFSET]\n  get <ID> [--entity rules|tasks]\n  patch <ID> --text <TEXT> [--entity rules|tasks]\n  remove <ID>\n  status\n  revise <ID> --entity rules|tasks\n";

struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
    path: PathBuf,
}

impl TempCwd {
    fn new() -> Self {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original working directory");
        let path = std::env::temp_dir().join(format!(
            "infraware-informational-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("create temporary working directory");
        std::env::set_current_dir(&path).expect("enter temporary working directory");
        Self {
            _lock,
            original,
            path,
        }
    }

    fn assert_empty(&self) {
        assert!(!self.path.join(".infraware").exists());
        assert!(
            fs::read_dir(&self.path)
                .expect("read temporary directory")
                .next()
                .is_none()
        );
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_cli(args: &[&str]) -> (u8, Vec<u8>, Vec<u8>) {
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(args, &mut stdout, &mut stderr);
    (status, stdout, stderr)
}

#[test]
fn bare_invocation_prints_help_without_store_access() {
    let cwd = TempCwd::new();
    let (status, stdout, stderr) = run_cli(&[]);
    assert_eq!(status, 0);
    assert_eq!(stdout, HELP.as_bytes());
    assert!(stderr.is_empty());
    cwd.assert_empty();
}

#[test]
fn help_prints_the_same_bytes_without_store_access() {
    let cwd = TempCwd::new();
    let (status, stdout, stderr) = run_cli(&["--help"]);
    assert_eq!(status, 0);
    assert_eq!(stdout, HELP.as_bytes());
    assert!(stderr.is_empty());
    cwd.assert_empty();
}

#[test]
fn version_uses_the_compile_time_package_version_without_store_access() {
    let cwd = TempCwd::new();
    let (status, stdout, stderr) = run_cli(&["--version"]);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        format!("infraware {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(stderr.is_empty());
    cwd.assert_empty();
}

#[test]
fn unsupported_informational_forms_remain_usage_errors() {
    let cwd = TempCwd::new();
    for args in [
        &["-h"][..],
        &["-V"][..],
        &["help"][..],
        &["query", "--help"][..],
        &["--help", "extra"][..],
        &["--version", "extra"][..],
    ] {
        let (status, stdout, stderr) = run_cli(args);
        assert_eq!(status, EXIT_USAGE, "args {args:?}");
        assert!(stdout.is_empty(), "args {args:?}");
        assert_eq!(
            stderr.iter().filter(|byte| **byte == b'\n').count(),
            1,
            "args {args:?}"
        );
    }
    cwd.assert_empty();
}
