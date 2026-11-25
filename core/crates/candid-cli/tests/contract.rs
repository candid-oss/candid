//! `candid tools` as a person or agent sees it on the command line: one line per tool, name and first sentence.
//! Snapshot in `core/fixtures/contract/candid-tools.txt`; regenerate with `UPDATE_SNAPSHOTS=1` and review the diff.
#[path = "../../candid-service/tests/support/snapshot.rs"]
mod snapshot;

#[test]
fn candid_tools_lists_the_public_tools() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_candid")).arg("tools").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let names: Vec<&str> = text.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(names, candid_store::TOOL_NAMES);
    let mut snaps = snapshot::Snapshots::default();
    snaps.text("contract/candid-tools.txt", &text);
    snaps.finish();
}
