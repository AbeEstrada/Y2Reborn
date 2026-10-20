//! The real panic hook ends Reborn, so it is tried in its own test binary:
//! a panic inside `isolated` is recorded and returned, and Reborn lives on.

use reborn_observability::{isolated, Observer};

#[test]
fn an_isolated_panic_is_recorded_and_returned_not_fatal() {
    let dir = std::env::temp_dir().join(format!("reborn-isolated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Observer::new(&dir).unwrap();
    log.install_panic_hook("test");
    let text = String::from("Don’t");
    let result = isolated(|| text[..4].len());
    assert!(result.unwrap_err().contains("not a char boundary"));
    assert_eq!(isolated(|| 7), Ok(7));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("panic-last.json")).unwrap()).unwrap();
    assert_eq!(record["isolated"], true);
    assert!(record["location"]
        .as_str()
        .unwrap()
        .contains("isolated_panic.rs"));
    let _ = std::fs::remove_dir_all(&dir);
}
