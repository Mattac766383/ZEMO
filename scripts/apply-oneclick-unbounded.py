from pathlib import Path

scanner = Path("crates/application/src/scanner.rs")
s = scanner.read_text()

old_caps = '''/// One-click intentionally organizes only loose files at the top level of the
/// standard personal folders. Existing user folder trees are left alone.
const CONSUMER_TOP_LEVEL_MAX_ENTRIES: usize = 5_000;
/// A normal personal folder should never monopolize the app indefinitely. The
/// bound is checked between native metadata inspections; it never weakens the
/// Apply-time identity/source-drift checks.
const CONSUMER_FOLDER_TIME_BUDGET: Duration = Duration::from_secs(3);'''
new_caps = '''/// One-click intentionally organizes only loose files at the top level of the
/// standard personal folders. Existing user folder trees are left alone.
/// There is no arbitrary file-count or wall-clock cutoff. The scan runs until
/// the folder is exhausted or the user explicitly cancels it. Progress events
/// are throttled only to keep the UI responsive on large folders.
const CONSUMER_PROGRESS_EMIT_EVERY: u64 = 128;'''
assert old_caps in s, "consumer cap marker missing"
s = s.replace(old_caps, new_caps, 1)
s = s.replace(
    "    time::{Duration, Instant, SystemTime, UNIX_EPOCH},",
    "    time::{SystemTime, UNIX_EPOCH},",
    1,
)

old_discovery = '''        let started = Instant::now();
        let (paths, mut truncated) = top_level_regular_paths(
            &root.absolute_path_native,
            CONSUMER_TOP_LEVEL_MAX_ENTRIES,
            is_cancelled,
        )?;'''
new_discovery = '''        let paths = top_level_regular_paths(&root.absolute_path_native, is_cancelled)?;
        let truncated = false;'''
assert old_discovery in s, "bounded discovery marker missing"
s = s.replace(old_discovery, new_discovery, 1)

old_budget = '''            if started.elapsed() >= CONSUMER_FOLDER_TIME_BUDGET {
                truncated = true;
                issues.push(ScanIssueInput {
                    relative_path: ".".to_owned(),
                    code: "time_budget_exceeded".to_owned(),
                    message: "consumer metadata scan reached its per-folder time budget".to_owned(),
                    is_directory: true,
                    is_error: false,
                    skipped: true,
                });
                progress.skipped_items = progress.skipped_items.saturating_add(1);
                break;
            }

'''
assert old_budget in s, "time budget branch missing"
s = s.replace(old_budget, "", 1)

old_progress = '''            on_progress(progress);
        }

        progress.phase = if cancelled {'''
new_progress = '''            if progress.files_discovered % CONSUMER_PROGRESS_EMIT_EVERY == 0 {
                on_progress(progress);
            }
        }
        on_progress(progress);

        progress.phase = if cancelled {'''
assert old_progress in s, "per-file progress marker missing"
s = s.replace(old_progress, new_progress, 1)

old_paths = '''fn top_level_regular_paths(
    root: &Path,
    max_entries: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<PathBuf>, bool), ApplicationError> {
    let mut paths = Vec::new();
    let mut truncated = false;
    let entries = fs::read_dir(root).map_err(ApplicationError::Io)?;
    for entry in entries {
        if is_cancelled() {
            break;
        }
        if paths.len() >= max_entries {
            truncated = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if file_type.is_file() && !file_type.is_symlink() {
            paths.push(PathBuf::from(name));
        }
    }
    Ok((paths, truncated))
}'''
new_paths = '''fn top_level_regular_paths(
    root: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<PathBuf>, ApplicationError> {
    let mut paths = Vec::new();
    let entries = fs::read_dir(root).map_err(ApplicationError::Io)?;
    for entry in entries {
        if is_cancelled() {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if file_type.is_file() && !file_type.is_symlink() {
            paths.push(PathBuf::from(name));
        }
    }
    Ok(paths)
}'''
assert old_paths in s, "top-level discovery function marker missing"
s = s.replace(old_paths, new_paths, 1)
scanner.write_text(s)

app = Path("apps/desktop/src/App.tsx")
a = app.read_text()
old_apply = '''        const completed = await startExecution(approved.session.id);
        executionIds.push(completed.session.id);
        filesMoved += completed.session.summary?.applied ?? current.summary.proposedMoves;'''
new_apply = '''        const completed = await startExecution(approved.session.id);
        const applied = completed.session.summary?.applied ?? 0;
        if (current.summary.proposedMoves > 0 && applied === 0) {
          throw new Error(
            `Apply returned zero physical moves for proposal ${current.id}; ZEMO will not report success.`,
          );
        }
        executionIds.push(completed.session.id);
        filesMoved += applied;'''
assert old_apply in a, "Apply accounting marker missing"
app.write_text(a.replace(old_apply, new_apply, 1))

test = Path("crates/application/tests/one_click_organize.rs")
t = test.read_text()
if "fn consumer_scan_has_no_arbitrary_file_count_cap()" not in t:
    t += r'''

#[test]
fn consumer_scan_has_no_arbitrary_file_count_cap() {
    let sandbox = MutationSandbox::new();
    const FILE_COUNT: usize = 5_257;
    for index in 0..FILE_COUNT {
        sandbox.write(&format!("loose-{index:05}.txt"), b"x");
    }
    let database = Arc::new(
        Database::open_in_memory(&DatabaseKey::from_bytes([31; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}")),
    );
    let platform = native_platform();
    let scanner = ScannerApplicationService::new(database, platform);
    let workspace = scanner
        .create_workspace("Unbounded one-click corpus")
        .unwrap_or_else(|error| panic!("workspace should be created: {error}"));
    scanner
        .register_root(workspace.id, sandbox.path())
        .unwrap_or_else(|error| panic!("root should register: {error}"));
    let scan = scanner
        .scan_workspace_consumer(workspace.id, &|| false, &mut |_| {})
        .unwrap_or_else(|error| panic!("unbounded metadata scan should succeed: {error}"));
    assert_eq!(scan.indexed_count as usize, FILE_COUNT);
    assert!(!scan.truncated, "one-click must not silently truncate a large folder");
}
'''
test.write_text(t)
