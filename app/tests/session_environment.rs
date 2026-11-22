#[test]
fn session_test_environment_requires_opt_in() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../session/tests/test-environment.sh");
    let status = std::process::Command::new("bash").arg(script).status().unwrap();
    assert!(status.success());
}
