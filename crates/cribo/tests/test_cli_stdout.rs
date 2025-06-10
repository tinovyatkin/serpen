#![allow(clippy::disallowed_methods)] // insta macros use unwrap internally

use insta::{assert_snapshot, with_settings};
use std::env;
use std::process::Command;

/// Helper function to get the path to a fixture file
fn get_fixture_path(relative_path: &str) -> String {
    let cwd = env::current_dir().expect("Failed to get current directory");
    let test_fixture_path = cwd.join("tests/fixtures").join(relative_path);
    test_fixture_path.to_string_lossy().to_string()
}

/// Run cribo with given arguments and return (stdout, stderr, exit_code)
fn run_cribo(args: &[&str]) -> (String, String, i32) {
    let output = Command::new("cargo")
        .args(["run", "--bin", "cribo", "--"])
        .args(args)
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    (stdout, stderr, exit_code)
}

/// Filters for normalizing paths in snapshots
fn get_cli_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        // Normalize file paths
        (r"/Volumes/workplace/[^\s]+", "<WORKSPACE>"),
        (r"\\\\?[A-Z]:\\\\[^\\s]+", "<WORKSPACE>"),
        // Normalize cargo paths
        (r"/Users/[^/]+/.cargo/[^\s]+", "<CARGO>"),
        (
            r"\\\\?C:\\\\Users\\\\[^\\\\]+\\\\.cargo\\\\[^\\s]+",
            "<CARGO>",
        ),
        // Normalize temporary paths
        (r"/var/folders/[^/]+/[^/]+/T/[^\s]+", "<TMP>"),
        (r"/tmp/[^\s]+", "<TMP>"),
        // Normalize timestamps if any
        (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", "<TIMESTAMP>"),
        // Normalize build times
        (
            r"Finished `[^`]+` profile \[[^\]]+\] target\(s\) in [0-9.]+s",
            "Finished `<PROFILE>` profile [<FLAGS>] target(s) in <TIME>",
        ),
        // Remove variable "Blocking waiting" lines and compilation lines
        (r"(?m)^\s*Blocking waiting for file lock.*\n", ""),
        (r"(?m)^\s*Compiling cribo.*\n", ""),
    ]
}

#[test]
fn test_stdout_flag_help() {
    let (stdout, _, exit_code) = run_cribo(&["--help"]);

    // Should succeed
    assert_eq!(exit_code, 0);

    // Check help contains stdout flag
    assert!(stdout.contains("--stdout"));
    assert!(stdout.contains("Output bundled code to stdout instead of a file"));
}

#[test]
fn test_stdout_conflicts_with_output() {
    let (_, stderr, exit_code) = run_cribo(&[
        "--entry",
        "nonexistent.py",
        "--output",
        "output.py",
        "--stdout",
    ]);

    // Should fail
    assert_ne!(exit_code, 0);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_conflicts_with_output_stderr", stderr);
    });
}

#[test]
fn test_missing_output_and_stdout_flags() {
    let (_, stderr, exit_code) = run_cribo(&["--entry", "nonexistent.py"]);

    // Should fail
    assert_ne!(exit_code, 0);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("missing_output_and_stdout_stderr", stderr);
    });
}

#[test]
fn test_stdout_bundling_functionality() {
    let (stdout, stderr, exit_code) = run_cribo(&[
        "--entry",
        &get_fixture_path("simple_project/main.py"),
        "--stdout",
    ]);

    // Should succeed
    assert_eq!(exit_code, 0, "Command failed with stderr: {}", stderr);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_bundling_output", stdout);
        assert_snapshot!("stdout_bundling_stderr", stderr);
    });

    // Ensure no log messages in stdout
    assert!(!stdout.contains("INFO"));
    assert!(!stdout.contains("WARN"));
    assert!(!stdout.contains("ERROR"));
}

#[test]
fn test_stdout_with_verbose_separation() {
    let (stdout, stderr, exit_code) = run_cribo(&[
        "--entry",
        &get_fixture_path("simple_project/main.py"),
        "--stdout",
        "-v",
    ]);

    // Should succeed
    assert_eq!(exit_code, 0);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_verbose_output", stdout);
        assert_snapshot!("stdout_verbose_stderr", stderr);
    });

    // Stdout should only contain Python code
    assert!(!stdout.contains("INFO"));
    assert!(!stdout.contains("Starting Cribo"));
}

#[test]
fn test_stdout_with_requirements() {
    let (stdout, stderr, exit_code) = run_cribo(&[
        "--entry",
        &get_fixture_path("simple_project/main.py"),
        "--stdout",
        "--emit-requirements",
    ]);

    // Should succeed
    assert_eq!(exit_code, 0);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_requirements_output", stdout);
        assert_snapshot!("stdout_requirements_stderr", stderr);
    });
}

#[test]
fn test_stdout_mode_preserves_bundled_structure() {
    let (stdout, _, exit_code) = run_cribo(&[
        "--entry",
        &get_fixture_path("simple_project/main.py"),
        "--stdout",
    ]);

    // Should succeed
    assert_eq!(exit_code, 0);

    // The bundled structure assertions will be in the snapshot itself
    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_bundled_structure", stdout);
    });
}

#[test]
fn test_stdout_error_handling() {
    let (stdout, stderr, exit_code) = run_cribo(&["--entry", "nonexistent_file.py", "--stdout"]);

    // Should fail
    assert_ne!(exit_code, 0);

    with_settings!({
        filters => get_cli_filters(),
    }, {
        assert_snapshot!("stdout_error_stdout", stdout);
        assert_snapshot!("stdout_error_stderr", stderr);
    });

    // Stdout should be empty or minimal
    assert!(stdout.is_empty() || stdout.len() < 100);
}
