//! Audit show folders: what a person might want to fix, as one list.
//!
//! ```sh
//! cuelight-check shows/beacon shows/minigolf
//! cuelight-check --kinds error,missing shows/*/
//! ```
//!
//! Every finding is printed as `folder: kind: place: what`, and the run
//! fails when anything was printed. `--kinds` keeps only some of
//! `error`, `missing`, `unused` and `unwise`, for a project that cares
//! about one and not another.

use cuelight_loader::{Driver, Manifest};
use std::path::Path;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!("usage: cuelight-check [--kinds error,missing,unused,unwise] <show folder>...");
        std::process::ExitCode::from(2)
    };
    let mut kinds: Vec<cuelight_core::FindingKind> = cuelight_core::FindingKind::ALL.to_vec();
    let mut dirs: Vec<String> = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return usage(),
            "--kinds" => {
                let Some(list) = args.next() else {
                    return usage();
                };
                let Some(chosen) = list
                    .split(',')
                    .map(|name| cuelight_core::FindingKind::parse(name.trim()))
                    .collect::<Option<Vec<_>>>()
                else {
                    eprintln!("--kinds takes some of error, missing, unused and unwise");
                    return std::process::ExitCode::from(2);
                };
                kinds = chosen;
            }
            _ if arg.starts_with('-') => return usage(),
            _ => dirs.push(arg),
        }
    }
    if dirs.is_empty() {
        return usage();
    }
    let mut found = 0;
    for dir in dirs {
        let dir = Path::new(&dir);
        let audited = Manifest::for_dir(dir)
            .map_err(|e| e.to_string())
            .and_then(|manifest| {
                let json =
                    std::fs::read_to_string(dir.join("show.json")).map_err(|e| e.to_string())?;
                let driver = manifest
                    .files
                    .iter()
                    .any(|f| f == "test-driver.json")
                    .then(|| Driver::from_file(dir.join("test-driver.json")))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                Ok(cuelight_loader::audit(
                    &json,
                    Some(&manifest.files),
                    driver.as_ref(),
                ))
            });
        let findings = match audited {
            Ok(findings) => findings,
            Err(e) => {
                eprintln!("{}: error: {e}", dir.display());
                found += 1;
                continue;
            }
        };
        for finding in findings.iter().filter(|f| kinds.contains(&f.kind)) {
            println!(
                "{}: {}: {}: {}",
                dir.display(),
                finding.kind.name(),
                finding.path,
                finding.message
            );
            found += 1;
        }
    }
    if found > 0 {
        eprintln!("{found} finding(s)");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
