//! Data-layer lab. Everything runs on clones; the live library is only ever read, by `pack`.
//!
//! ```text
//! candid-lab pack <library-root> <name>            # consistent snapshot of a library into $CANDID_LAB/packs/<name>
//! candid-lab run  <pack> <experiment.json> <run>   # clone, configure, run the real worker, dump → $CANDID_LAB/runs/<run>
//! candid-lab diff <runA> <runB>                    # what changed between two runs
//! candid-lab dump <root> <from> <until>            # dump any root (e.g. a run's root after poking it)
//! ```
//! `$CANDID_LAB` defaults to `~/CandidLab`. Model replies are cached under `$CANDID_LAB/cache` by request body.

mod diff;
mod dump;
mod proxy;
mod run;

use std::path::{Path, PathBuf};

fn resolve(dir: &str, name: &str) -> PathBuf {
    let p = Path::new(name);
    if p.is_absolute() || p.exists() { p.into() } else { run::lab_dir().join(dir).join(name) }
}

fn main() {
    // The worker's model calls go to the lab proxy on this machine; a system HTTP proxy must not capture them
    // (`lab.localhost` is not in a typical proxy exception list). The lab proxy itself forwards directly.
    // SAFETY: set before any thread starts.
    unsafe { std::env::set_var("CANDID_PROXY", "direct") };
    let args: Vec<String> = std::env::args().collect();
    let a = |i: usize| args.get(i).map(String::as_str).unwrap_or_else(|| { eprintln!("missing argument {i}; see the header of candid-lab/src/main.rs"); std::process::exit(2) });
    match a(1) {
        "pack" => {
            let src = Path::new(a(2));
            let dst = run::lab_dir().join("packs").join(a(3));
            assert!(!dst.exists(), "{} exists", dst.display());
            std::fs::create_dir_all(&dst).unwrap();
            let ok = std::process::Command::new("sqlite3")
                .arg(format!("file:{}?mode=ro", src.join("timeline.sqlite").display()))
                .arg(format!(".backup '{}'", dst.join("timeline.sqlite").display()))
                .status().unwrap().success();
            assert!(ok, "snapshot failed");
            for entry in ["store.json", "blobs", "media"] {
                if src.join(entry).exists() {
                    assert!(std::process::Command::new("cp").args(["-cR"]).arg(src.join(entry)).arg(&dst).status().unwrap().success());
                }
            }
            for d in ["staging", "logs", "exports"] {
                std::fs::create_dir_all(dst.join(d)).unwrap();
            }
            eprintln!("packed {}", dst.display());
        }
        "run" => run::run(&resolve("packs", a(2)), Path::new(a(3)), &resolve("runs", a(4))),
        "diff" => {
            let load = |n: &str| serde_json::from_str(&std::fs::read_to_string(resolve("runs", n).join("dump.json")).unwrap()).unwrap();
            print!("{}", diff::diff(&load(a(2)), &load(a(3))));
        }
        "dump" => {
            let store = candid_store::TimelineStore::open(a(2), "candid/lab", false, candid_store::Clock::default()).unwrap();
            let d = dump::dump(&store, run::parse_time(a(3)), run::parse_time(a(4)));
            println!("{}", serde_json::to_string_pretty(&d).unwrap());
        }
        _ => eprintln!("usage: candid-lab pack|run|diff|dump …"),
    }
}
