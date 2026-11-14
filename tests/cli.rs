use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn yast() -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("yast"));
    command.args(["--threads", "2"]);
    command
}

fn put(root: &Path, relative: &str, content: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn report(command: &mut Command, code: i32) -> Value {
    let output = command
        .arg("--format")
        .arg("json")
        .assert()
        .code(code)
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

#[test]
fn compare_is_path_independent_and_reports_duplicates() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "a", "common");
    put(source.path(), "nested/b", "common");
    put(source.path(), "c", "missing");
    put(target.path(), "renamed", "common");
    put(target.path(), "extra", "extra");
    let result = report(
        yast().arg("compare").arg(source.path()).arg(target.path()),
        1,
    );
    assert_eq!(result["comparison"]["missing"][0]["paths"][0], "c");
    assert_eq!(result["comparison"]["extra"][0]["paths"][0], "extra");
    assert_eq!(result["comparison"]["common"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["comparison"]["source_duplicates"][0]["paths"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn equal_trees_and_empty_trees_succeed() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    yast()
        .arg("compare")
        .arg(source.path())
        .arg(target.path())
        .assert()
        .success();
    put(source.path(), "a", "same");
    put(target.path(), "b", "same");
    yast()
        .arg("compare")
        .arg(source.path())
        .arg(target.path())
        .assert()
        .success();
}

#[test]
fn dupes_and_filters() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a", "abc");
    put(dir.path(), ".hidden", "abc");
    put(dir.path(), "ignored.tmp", "abc");
    put(dir.path(), ".folder/file", "abc");
    let result = report(yast().arg("dupes").arg(dir.path()), 1);
    assert_eq!(
        result["comparison"]["source_duplicates"][0]["paths"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    yast()
        .arg("dupes")
        .arg(dir.path())
        .args(["--skip-hidden", "--exclude", "*.tmp"])
        .assert()
        .success();
    let result = report(
        yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--min-size", "1KiB"]),
        0,
    );
    assert_eq!(result["comparison"]["source_files"], 0);
}

#[test]
fn dupes_report_default_output_is_unchanged() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a", "same");
    put(dir.path(), "b", "same");
    for format in ["text", "json", "csv"] {
        let default = yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--format", format])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let explicit = yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--action", "report", "--format", format])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(default, explicit);
        if format == "json" {
            let value: Value = serde_json::from_slice(&default).unwrap();
            assert!(value.get("dedupe").is_none());
        }
    }
}

#[test]
fn dupes_actions_are_dry_runs_by_default_in_all_report_formats() {
    let dir = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let target = parent.path().join("new");
    put(dir.path(), "a", "same");
    put(dir.path(), "b", "same");
    for action in ["delete", "move"] {
        for format in ["text", "json", "csv"] {
            let mut command = yast();
            command
                .arg("dupes")
                .arg(dir.path())
                .args(["--action", action, "--format", format]);
            if action == "move" {
                command.arg("--to").arg(&target);
            }
            let output = command.assert().success().get_output().stdout.clone();
            match format {
                "json" => {
                    let value: Value = serde_json::from_slice(&output).unwrap();
                    assert_eq!(value["dedupe"]["dry_run"], true);
                    assert_eq!(value["dedupe"]["done"], 0);
                    assert_eq!(value["dedupe"]["failed"], 0);
                    assert_eq!(value["dedupe"]["bytes_reclaimed"], 0);
                    let selected = &value["dedupe"]["actions"][0];
                    assert_eq!(selected["keeper"], "a");
                    assert_eq!(selected["path"], "b");
                    assert_eq!(selected["size"], 4);
                    assert_eq!(selected["status"], "planned");
                    assert_eq!(
                        selected["action"],
                        if action == "delete" { "trash" } else { "move" }
                    );
                    assert!(selected["error"].is_null());
                    assert_eq!(selected["destination"].is_null(), action == "delete");
                }
                "csv" => {
                    let mut reader = csv::Reader::from_reader(&output[..]);
                    assert_eq!(reader.headers().unwrap().get(8), Some("keeper"));
                    assert_eq!(reader.headers().unwrap().get(9), Some("action"));
                    let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
                    let selected = rows
                        .iter()
                        .find(|row| row.get(0) == Some("action"))
                        .unwrap();
                    assert_eq!(selected.get(4), Some("b"));
                    assert_eq!(selected.get(6), Some("planned"));
                    assert_eq!(selected.get(8), Some("a"));
                    assert_eq!(
                        selected.get(9),
                        Some(if action == "delete" { "trash" } else { "move" })
                    );
                    assert_eq!(rows.last().unwrap().get(0), Some("summary"));
                }
                _ => {
                    let text = String::from_utf8(output).unwrap();
                    assert!(text.contains("DRY RUN"));
                    assert!(text.contains("Actions:"));
                    assert!(text.contains("keeper: a"));
                    assert!(text.contains("bytes reclaimed: 0"));
                }
            }
            assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
            assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"same");
            assert!(!target.exists());
        }
    }
}

#[test]
fn dupes_permanent_delete_keeps_first_path_and_unrelated_content() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a.txt", "same");
    put(dir.path(), "nested/z.txt", "same");
    put(dir.path(), "z.txt", "same");
    put(dir.path(), "unique", "different");
    put(dir.path(), "empty-a", "");
    put(dir.path(), "empty-b", "");
    let result = report(
        yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--action", "delete", "--permanent"]),
        0,
    );
    assert_eq!(result["dedupe"]["actions"][0]["action"], "delete");
    assert!(dir.path().join("z.txt").exists());
    let result = report(
        yast().arg("dupes").arg(dir.path()).args([
            "--action",
            "delete",
            "--permanent",
            "--execute",
        ]),
        0,
    );
    assert_eq!(result["dedupe"]["done"], 3);
    assert_eq!(result["dedupe"]["failed"], 0);
    assert_eq!(result["dedupe"]["bytes_reclaimed"], 8);
    assert!(result["dedupe"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["status"] == "done"));
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"same");
    assert_eq!(fs::read(dir.path().join("unique")).unwrap(), b"different");
    assert!(dir.path().join("empty-a").exists());
    assert!(!dir.path().join("empty-b").exists());
    assert!(!dir.path().join("nested/z.txt").exists());
    assert!(!dir.path().join("z.txt").exists());
    yast().arg("dupes").arg(dir.path()).assert().success();
}

#[test]
fn dupes_move_keep_preserves_paths_content_and_mtime() {
    let dir = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let target = parent.path().join("new");
    put(dir.path(), "a.txt", "same");
    put(dir.path(), "nested/z.txt", "same");
    let modified = fs::metadata(dir.path().join("nested/z.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let result = report(
        yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--action", "move", "--to"])
            .arg(&target)
            .arg("--execute"),
        0,
    );
    assert_eq!(result["dedupe"]["done"], 1);
    assert_eq!(result["dedupe"]["bytes_reclaimed"], 0);
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("nested/z.txt").exists());
    assert_eq!(fs::read(target.join("nested/z.txt")).unwrap(), b"same");
    assert_eq!(
        fs::metadata(target.join("nested/z.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn dupes_move_flatten_disambiguates_existing_and_planned_collisions() {
    for existing in [false, true] {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        put(dir.path(), "0.txt", "same");
        put(dir.path(), "a/b.txt", "same");
        put(dir.path(), "a_b.txt", "same");
        if existing {
            put(target.path(), "a_b.txt", "existing");
        }
        let result = report(
            yast()
                .arg("dupes")
                .arg(dir.path())
                .args(["--action", "move", "--to"])
                .arg(target.path())
                .args(["--layout", "flatten", "--max-name-len", "24", "--execute"]),
            0,
        );
        assert_eq!(result["dedupe"]["done"], 2);
        let actions = result["dedupe"]["actions"].as_array().unwrap();
        assert_ne!(actions[0]["destination"], actions[1]["destination"]);
        for action in actions {
            let destination = Path::new(action["destination"].as_str().unwrap());
            assert_eq!(
                destination.parent(),
                Some(target.path().canonicalize().unwrap().as_path())
            );
            let name = destination.file_name().unwrap().to_string_lossy();
            assert!(name.chars().count() <= 24 && name.ends_with(".txt"));
            assert_eq!(fs::read(destination).unwrap(), b"same");
        }
        if existing {
            assert_eq!(
                fs::read(target.path().join("a_b.txt")).unwrap(),
                b"existing"
            );
        }
        assert!(dir.path().join("0.txt").exists());
        assert!(!dir.path().join("a/b.txt").exists());
        assert!(!dir.path().join("a_b.txt").exists());
    }
}

#[test]
fn dupes_move_reports_conflicts_without_overwriting_and_continues_other_actions() {
    let dir = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(dir.path(), "a", "same");
    put(dir.path(), "b", "same");
    put(dir.path(), "c", "same");
    put(target.path(), "b", "existing");
    let result = report(
        yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--action", "move", "--to"])
            .arg(target.path())
            .arg("--execute"),
        2,
    );
    assert_eq!(result["dedupe"]["failed"], 1);
    assert_eq!(result["dedupe"]["done"], 1);
    assert_eq!(result["dedupe"]["actions"][0]["status"], "failed");
    assert!(result["dedupe"]["actions"][0]["error"]
        .as_str()
        .unwrap()
        .contains("not overwritten"));
    assert_eq!(fs::read(target.path().join("b")).unwrap(), b"existing");
    assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"same");
    assert_eq!(fs::read(target.path().join("c")).unwrap(), b"same");
    assert!(dir.path().join("a").exists());
}

#[test]
fn dupes_rejects_overlapping_destinations_and_invalid_options() {
    let parent = TempDir::new().unwrap();
    let dir = parent.path().join("source");
    fs::create_dir(&dir).unwrap();
    for target in [
        dir.join("nested"),
        dir.clone(),
        parent.path().to_path_buf(),
        dir.join("missing/../nested"),
    ] {
        yast()
            .arg("dupes")
            .arg(&dir)
            .args(["--action", "move", "--to"])
            .arg(&target)
            .assert()
            .code(2);
    }
    for args in [
        vec!["--to", "target"],
        vec!["--action", "delete", "--to", "target"],
        vec!["--action", "move"],
        vec!["--permanent"],
        vec!["--action", "move", "--to", "target", "--permanent"],
        vec!["--execute"],
        vec!["--action", "delete", "--layout", "flatten"],
        vec!["--action", "move", "--to", "target", "--max-name-len", "15"],
    ] {
        yast().arg("dupes").arg(&dir).args(args).assert().code(2);
    }
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn size_filter_and_partial_hash_never_hide_equal_size_differences() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let mut bytes = vec![b'a'; 128 * 1024];
    put(source.path(), "large", &bytes);
    bytes[100_000] = b'b';
    put(target.path(), "large", &bytes);
    put(source.path(), "unique", "uniquely sized");
    let result = report(
        yast().arg("compare").arg(source.path()).arg(target.path()),
        1,
    );
    assert_eq!(result["comparison"]["missing"].as_array().unwrap().len(), 2);
    assert_eq!(result["comparison"]["hashed_files"], 2);
    assert_eq!(result["comparison"]["partial_hashed_files"], 2);
    let all = report(
        yast()
            .arg("compare")
            .arg(source.path())
            .arg(target.path())
            .arg("--hash-all"),
        1,
    );
    assert_eq!(all["comparison"]["hashed_files"], 3);
}

#[test]
fn sync_is_dry_by_default_and_copies_one_alphabetical_representative() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "a.txt", "duplicate");
    put(source.path(), "z.txt", "duplicate");
    put(source.path(), "nested/unique.txt", "different");
    let result = report(yast().arg("sync").arg(source.path()).arg(target.path()), 0);
    assert_eq!(result["sync"]["dry_run"], true);
    assert_eq!(result["sync"]["planned"], 2);
    assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
    let result = report(
        yast()
            .arg("sync")
            .arg(source.path())
            .arg(target.path())
            .args(["--execute", "--verify"]),
        0,
    );
    assert_eq!(result["sync"]["copied"], 2);
    assert!(target.path().join("a.txt").is_file());
    assert!(!target.path().join("z.txt").exists());
    assert_eq!(
        fs::read(target.path().join("nested/unique.txt")).unwrap(),
        b"different"
    );
    let left = fs::metadata(source.path().join("a.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let right = fs::metadata(target.path().join("a.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(left, right);
    let repeat = report(
        yast()
            .arg("sync")
            .arg(source.path())
            .arg(target.path())
            .arg("--execute"),
        0,
    );
    assert_eq!(repeat["sync"]["copied"], 0);
    assert_eq!(repeat["sync"]["skipped"], 3);
}

#[test]
fn flatten_handles_existing_and_planned_collisions() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "a/b.txt", "first");
    put(source.path(), "a_b.txt", "second");
    put(target.path(), "flat/a_b.txt", "never overwrite");
    let result = report(
        yast()
            .arg("sync")
            .arg(source.path())
            .arg(target.path())
            .args([
                "--layout",
                "flatten",
                "--dest-subdir",
                "flat",
                "--max-name-len",
                "24",
                "--execute",
                "--verify",
            ]),
        0,
    );
    assert_eq!(result["sync"]["copied"], 2);
    let names: Vec<_> = fs::read_dir(target.path().join("flat"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names
        .iter()
        .all(|name| name.chars().count() <= 24 && name.ends_with(".txt")));
    assert_eq!(
        fs::read(target.path().join("flat/a_b.txt")).unwrap(),
        b"never overwrite"
    );
}

#[test]
fn keep_conflicts_are_errors_and_extras_are_never_deleted() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "a", "new");
    put(target.path(), "a", "old");
    put(target.path(), "extra", "extra");
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(target.path())
        .arg("--execute")
        .assert()
        .code(2);
    assert_eq!(fs::read(target.path().join("a")).unwrap(), b"old");
    assert!(target.path().join("extra").is_file());
}

#[test]
fn new_target_is_created_only_on_execute_and_subdir_cannot_escape() {
    let source = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let target = parent.path().join("new");
    put(source.path(), "a", "new");
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(&target)
        .assert()
        .success();
    assert!(!target.exists());
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(&target)
        .args(["--dest-subdir", "../escape", "--execute"])
        .assert()
        .code(2);
    assert!(!target.exists());
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(&target)
        .arg("--execute")
        .assert()
        .success();
    assert!(target.join("a").exists());
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(source.path().join("nested"))
        .arg("--execute")
        .assert()
        .code(2);
}

#[test]
fn csv_and_output_file_are_valid_and_quiet_still_reports() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "comma,name.txt", "missing");
    let output = yast()
        .arg("compare")
        .arg(source.path())
        .arg(target.path())
        .args(["--format", "csv", "--quiet"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(output.stderr.is_empty());
    let mut reader = csv::Reader::from_reader(&output.stdout[..]);
    assert_eq!(reader.headers().unwrap().get(0), Some("category"));
    let rows = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows[0].get(4), Some("comma,name.txt"));
    let path = target.path().join("report.json");
    yast()
        .arg("dupes")
        .arg(source.path())
        .args(["--format", "json", "--output"])
        .arg(&path)
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["command"], "dupes");
    yast()
        .arg("dupes")
        .arg(source.path())
        .arg("--output")
        .arg(&path)
        .assert()
        .code(2);
}

#[test]
fn all_hash_algorithms_and_invalid_inputs() {
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    put(source.path(), "a", "");
    put(target.path(), "b", "");
    for algorithm in ["blake3", "xxh3", "crc32", "md5", "sha1", "sha256"] {
        yast()
            .arg("compare")
            .arg(source.path())
            .arg(target.path())
            .args(["--hash", algorithm])
            .assert()
            .success();
    }
    yast()
        .arg("dupes")
        .arg(source.path())
        .args(["--threads", "0"])
        .assert()
        .code(2);
    yast()
        .arg("dupes")
        .arg(source.path())
        .args(["--exclude", "["])
        .assert()
        .code(2);
    yast()
        .arg("dupes")
        .arg(source.path())
        .args(["--min-size", "18446744073709551615GiB"])
        .assert()
        .code(2);
    yast()
        .arg("dupes")
        .arg(source.path().join("missing"))
        .assert()
        .code(2);
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_json_filenames_are_rejected_before_duplicate_actions() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for action in ["delete", "move"] {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        put(dir.path(), "a", "same");
        let duplicate = dir.path().join(OsString::from_vec(b"z-\xff".to_vec()));
        fs::write(&duplicate, b"same").unwrap();
        let output = dir.path().join("report.json");
        let mut command = yast();
        command
            .arg("dupes")
            .arg(dir.path())
            .args([
                "--action",
                action,
                "--execute",
                "--format",
                "json",
                "--output",
            ])
            .arg(&output);
        if action == "delete" {
            command.arg("--permanent");
        } else {
            command.arg("--to").arg(target.path());
        }
        let result = command.assert().code(2).get_output().clone();
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("validating duplicate report paths")
        );
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
        assert_eq!(fs::read(&duplicate).unwrap(), b"same");
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_json_move_destination_is_rejected_before_duplicate_actions() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let dir = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    put(dir.path(), "a", "same");
    put(dir.path(), "b", "same");
    let target = parent
        .path()
        .join(OsString::from_vec(b"destination-\xff".to_vec()));
    let result = yast()
        .arg("dupes")
        .arg(dir.path())
        .args(["--action", "move", "--execute", "--format", "json", "--to"])
        .arg(&target)
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(!result.stderr.is_empty());
    assert!(result.stdout.is_empty());
    assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
    assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"same");
    assert!(!target.exists());
}

#[cfg(windows)]
#[test]
fn dupes_move_rejects_a_drive_root_containing_source() {
    use std::path::{Component, PathBuf, Prefix};
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a", "same");
    put(dir.path(), "b", "same");
    let drive = match dir.path().components().next().unwrap() {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => panic!("expected a drive-based temporary directory"),
        },
        _ => panic!("expected a drive prefix"),
    };
    let root = PathBuf::from(format!("{}:\\", char::from(drive)));
    for command in ["dupes", "sync"] {
        let mut invocation = yast();
        invocation.arg(command).arg(dir.path());
        if command == "dupes" {
            invocation.args(["--action", "move", "--to"]);
        }
        let result = invocation
            .arg(&root)
            .arg("--execute")
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert!(String::from_utf8_lossy(&result.stderr).contains("non-overlapping"));
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
        assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"same");
    }
}

#[cfg(unix)]
#[test]
fn dupes_move_resolves_symlinks_before_parent_components() {
    use std::os::unix::fs::symlink;
    let parent = TempDir::new().unwrap();
    let dir = parent.path().join("source");
    put(&dir, "a", "same");
    put(&dir, "b", "same");
    fs::create_dir(dir.join("nested")).unwrap();
    let link = parent.path().join("link");
    symlink(dir.join("nested"), &link).unwrap();
    let destination = link.join("..").join("dupes");
    yast()
        .arg("dupes")
        .arg(&dir)
        .args(["--action", "move", "--to"])
        .arg(&destination)
        .arg("--execute")
        .assert()
        .code(2);
    assert_eq!(fs::read(dir.join("a")).unwrap(), b"same");
    assert_eq!(fs::read(dir.join("b")).unwrap(), b"same");
    assert!(!dir.join("dupes").exists());
    assert!(!parent.path().join("dupes").exists());

    let outside = TempDir::new().unwrap();
    fs::create_dir(outside.path().join("nested")).unwrap();
    let outside_link = parent.path().join("outside-link");
    symlink(outside.path().join("nested"), &outside_link).unwrap();
    yast()
        .arg("dupes")
        .arg(&dir)
        .args(["--action", "move", "--to"])
        .arg(outside_link.join("..").join("dupes"))
        .arg("--execute")
        .assert()
        .success();
    assert_eq!(fs::read(outside.path().join("dupes/b")).unwrap(), b"same");
    assert!(!parent.path().join("dupes").exists());
}

#[test]
fn rejected_report_destinations_leave_duplicates_untouched() {
    for action in ["delete", "move"] {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        put(dir.path(), "a", "same");
        put(dir.path(), "b", "same");
        put(dir.path(), "report.json", "existing report");
        for output in [
            dir.path().join("b"),
            dir.path().join("report.json"),
            dir.path().join("missing/report.json"),
            dir.path().join("a/report.json"),
            dir.path().to_path_buf(),
        ] {
            let mut command = yast();
            command
                .arg("dupes")
                .arg(dir.path())
                .args(["--action", action, "--execute"]);
            if action == "delete" {
                command.arg("--permanent");
            } else {
                command.arg("--to").arg(target.path());
            }
            command.arg("--output").arg(output).assert().code(2);
            assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
            assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"same");
            assert_eq!(
                fs::read(dir.path().join("report.json")).unwrap(),
                b"existing report"
            );
            assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
        }
    }
}

#[test]
fn report_output_is_reserved_after_scan_and_reused_after_deletion() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a", "");
    put(dir.path(), "b", "");
    let output = dir.path().join("0-report.json");
    yast()
        .arg("dupes")
        .arg(dir.path())
        .args([
            "--action",
            "delete",
            "--permanent",
            "--execute",
            "--format",
            "json",
            "--output",
        ])
        .arg(&output)
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(value["comparison"]["source_files"], 2);
    assert_eq!(value["dedupe"]["done"], 1);
    assert!(dir.path().join("a").exists());
    assert!(!dir.path().join("b").exists());
}

#[cfg(unix)]
#[test]
fn dupes_move_rejects_symlink_parents_and_delete_rejects_symlink_members() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    put(dir.path(), "a", "same");
    put(dir.path(), "nested/b", "same");
    symlink(outside.path(), target.path().join("nested")).unwrap();
    let result = report(
        yast()
            .arg("dupes")
            .arg(dir.path())
            .args(["--action", "move", "--to"])
            .arg(target.path())
            .arg("--execute"),
        2,
    );
    assert_eq!(result["dedupe"]["failed"], 1);
    assert!(result["dedupe"]["actions"][0]["error"]
        .as_str()
        .unwrap()
        .contains("plain directory"));
    assert_eq!(fs::read(dir.path().join("nested/b")).unwrap(), b"same");
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

    symlink(dir.path().join("a"), dir.path().join("0-link")).unwrap();
    let result = report(
        yast().arg("dupes").arg(dir.path()).args([
            "--follow-symlinks",
            "--action",
            "delete",
            "--permanent",
            "--execute",
        ]),
        2,
    );
    assert_eq!(result["dedupe"]["done"], 0);
    assert!(dir
        .path()
        .join("0-link")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"same");
    assert_eq!(fs::read(dir.path().join("nested/b")).unwrap(), b"same");
}

#[cfg(unix)]
#[test]
fn symlinks_are_opt_in_and_destination_symlink_is_rejected() {
    use std::os::unix::fs::symlink;
    let source = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    put(source.path(), "real/a", "content");
    symlink(source.path().join("real/a"), source.path().join("linked")).unwrap();
    yast().arg("dupes").arg(source.path()).assert().success();
    yast()
        .arg("dupes")
        .arg(source.path())
        .arg("--follow-symlinks")
        .assert()
        .code(1);
    symlink(outside.path(), target.path().join("real")).unwrap();
    yast()
        .arg("sync")
        .arg(source.path())
        .arg(target.path())
        .arg("--execute")
        .assert()
        .code(2);
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
