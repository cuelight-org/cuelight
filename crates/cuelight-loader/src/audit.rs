//! The audit: everything a person might want to fix in a show, as one
//! list of findings, over the document and, when there is one, the folder
//! beside it. Advisory throughout: nothing here stops a show loading.

use crate::manifest::{is_path, references_at, Asset};
use crate::{Driver, Step, SOUND_EXTENSIONS, VECTOR_EXTENSION, VIDEO_EXTENSIONS};
use cuelight_core::{Finding, FindingKind};

/// Image files a show folder may hold, whatever this build decodes: the
/// audit asks whether a file is there, not whether it can be read.
const IMAGE_FILES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// What an artwork name by stem may stand for: pixels or vector artwork,
/// whichever is there, since one layer kind draws both.
const ARTWORK_FILES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", VECTOR_EXTENSION];

/// Audit the show document `json`: what a strict load would refuse
/// ([`FindingKind::Error`]), fields the engine does not know ([`FindingKind::Unwise`]),
/// and everything [`cuelight_core::audit`] finds in the document. With
/// `files`, the paths of the show's folder as a [`Manifest`](crate::Manifest)
/// lists them, also the names with no file behind them
/// ([`FindingKind::Missing`]) and the files nothing names ([`FindingKind::Unused`]);
/// with `driver`, the steps of its script that fire a trigger nothing
/// listens to or set a name the show does not have ([`FindingKind::Missing`]).
///
/// An editor passes the document it is editing, unsaved, and the files
/// it knows of; a tool passes what it read off disk. A document that is
/// not a show at all is one finding. Each finding names its place: a
/// path in the document, or a file's path in the show folder.
pub fn audit(json: &str, files: Option<&[String]>, driver: Option<&Driver>) -> Vec<Finding> {
    let audited = match cuelight_core::audit_document(json) {
        Ok(audited) => audited,
        Err(e) => {
            return vec![Finding {
                path: "show.json".to_owned(),
                message: e.to_string(),
                kind: FindingKind::Error,
            }]
        }
    };
    let mut out = audited.findings;
    // The document as the audit saw it, its dropped parts blanked in
    // place, so a file finding after a dropped layer names the layer
    // the author sees.
    let show = &audited.show;
    if let Some(files) = files {
        out.extend(file_findings(show, files));
    }
    if let Some(driver) = driver {
        out.extend(driver_findings(show, driver));
    }
    out
}

/// The file in `files` a name of `kind` stands for: the name itself when
/// it is a path, else the file of that stem in the conventional folder.
fn file_of<'a>(kind: Asset, name: &str, files: &'a [String]) -> Option<&'a String> {
    if is_path(name) {
        return files.iter().find(|f| *f == name);
    }
    let (folder, extensions): (&str, &[&str]) = match kind {
        Asset::Image => ("assets", ARTWORK_FILES),
        Asset::Vector => ("assets", &[VECTOR_EXTENSION]),
        Asset::Sound => ("assets/sounds", SOUND_EXTENSIONS),
        Asset::Video => ("assets/videos", VIDEO_EXTENSIONS),
        Asset::Font => ("assets/fonts", &["fnt", "ttf", "otf"]),
    };
    files.iter().find(|file| {
        let Some((dir, filename)) = file.rsplit_once('/') else {
            return false;
        };
        let Some((stem, extension)) = filename.rsplit_once('.') else {
            return false;
        };
        dir == folder
            && stem == name
            && extensions.contains(&extension.to_ascii_lowercase().as_str())
    })
}

fn file_findings(show: &cuelight_core::Show, files: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut used: Vec<&String> = Vec::new();
    for (kind, name, at) in references_at(show) {
        match file_of(kind, &name, files) {
            Some(file) => used.push(file),
            None => {
                let what = match kind {
                    Asset::Image => "artwork",
                    Asset::Vector => "vector artwork",
                    Asset::Sound => "sound",
                    Asset::Video => "video",
                    Asset::Font => "font",
                };
                out.push(Finding {
                    path: at,
                    message: format!("names {what} {name:?}, and no file in the show is it"),
                    kind: FindingKind::Missing,
                });
            }
        }
    }
    for file in files {
        let asset = file.starts_with("assets/");
        // A bitmap font's page images sit beside it and are its to use.
        let page = file.starts_with("assets/fonts/")
            && file
                .rsplit_once('.')
                .is_some_and(|(_, e)| IMAGE_FILES.contains(&e.to_ascii_lowercase().as_str()));
        if asset && !page && !used.contains(&file) {
            out.push(Finding {
                path: file.clone(),
                message: "nothing in the show names this file".to_owned(),
                kind: FindingKind::Unused,
            });
        }
    }
    out
}

fn driver_findings(show: &cuelight_core::Show, driver: &Driver) -> Vec<Finding> {
    let triggers = show.triggers();
    let mut out = Vec::new();
    for (i, step) in driver.steps.iter().enumerate() {
        let at = format!("test-driver.json:steps[{i}]");
        match step {
            Step::Trigger { trigger } if !triggers.contains(trigger) => out.push(Finding {
                path: at,
                message: format!("fires {trigger:?}, which nothing in the show listens to"),
                kind: FindingKind::Missing,
            }),
            Step::Set { set } => {
                for name in set.keys() {
                    if !show.variables.contains_key(name) && !show.values.contains_key(name) {
                        out.push(Finding {
                            path: at.clone(),
                            message: format!(
                                "sets {name:?}, which the show declares as neither a variable \
                                 nor a value"
                            ),
                            kind: FindingKind::Missing,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}
