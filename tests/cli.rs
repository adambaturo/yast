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
