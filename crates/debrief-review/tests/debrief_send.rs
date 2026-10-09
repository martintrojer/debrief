//! Runs scripts/test-debrief-send.sh, which drives scripts/debrief-send against a fake `mu`.

use std::path::Path;
use std::process::Command;

#[test]
fn debrief_send_script() {
    if Command::new("jq").arg("--version").output().is_err() {
        eprintln!("skipping: jq not found");
        return;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test-debrief-send.sh");
    let out = Command::new("bash").arg(&script).output().unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
