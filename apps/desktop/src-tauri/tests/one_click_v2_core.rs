#[path = "../src/folder_access.rs"]
mod folder_access;
#[path = "../src/one_click_v2.rs"]
mod one_click_v2;

use one_click_v2::{apply_plan, build_plan, undo};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn fixture() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("zemo-one-click-v2-contract-{id}"));
    fs::create_dir_all(&root).unwrap();
    let names = [
        "facture.pdf",
        "photo.jpg",
        "video.mp4",
        "archive.zip",
        "notes.txt",
        "client-projet.docx",
        "capture.png",
        "film.mov",
        "data.csv",
        "mystere.xyz",
    ];
    for i in 0..150_u32 {
        fs::write(
            root.join(format!("{i:03}-{}", names[i as usize % names.len()])),
            b"zemo",
        )
        .unwrap();
    }
    root
}

fn loose_files(root: &PathBuf) -> usize {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .count()
}

#[test]
fn messy_desktop_becomes_clean_and_undo_is_exact() {
    let root = fixture();
    assert_eq!(loose_files(&root), 150);

    let started = std::time::Instant::now();
    let plan = build_plan(std::slice::from_ref(&root));
    assert!(started.elapsed().as_secs_f32() < 10.0);
    assert_eq!(plan.files_seen, 150);
    assert_eq!(plan.proposed_moves, 150);

    let applied = apply_plan(&plan);
    assert!(applied.errors.is_empty(), "apply errors: {:?}", applied.errors);
    assert_eq!(applied.applied.len(), 150);
    assert_eq!(loose_files(&root), 0, "desktop must be visibly clean");

    let undone = undo(&applied.applied);
    assert!(undone.errors.is_empty(), "undo errors: {:?}", undone.errors);
    assert_eq!(loose_files(&root), 150, "undo must restore every loose file");

    let _ = fs::remove_dir_all(root);
}
