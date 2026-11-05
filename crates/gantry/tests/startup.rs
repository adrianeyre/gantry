//! Starting the real binary.
use std::process::Command;

fn gantry(data_dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gantry"));
    cmd.env_clear()
        .env("GANTRY_DATA_DIR", data_dir)
        .env("GANTRY_BIND", "127.0.0.1:0")
        .current_dir(data_dir);
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        cmd.env("SYSTEMROOT", root);
    }
    cmd
}

#[test]
fn refuses_to_start_without_a_secret_key() {
    let dir = tempfile::tempdir().unwrap();
    let output = gantry(dir.path()).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GANTRY_SECRET_KEY"), "{stderr}");
    assert!(stderr.contains("openssl rand"), "{stderr}");
    assert!(
        !dir.path().join("gantry.db").exists(),
        "no database without a key"
    );
}

fn create_user(dir: &std::path::Path, username: &str, password: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = gantry(dir)
        .args([
            "user",
            "create",
            "--admin",
            "--password-stdin",
            "--username",
            username,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{password}").unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn user_create_makes_an_admin_once() {
    let dir = tempfile::tempdir().unwrap();
    let first = create_user(dir.path(), "adrian", "a long password");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("Created the Admin account"));

    let again = create_user(dir.path(), "adrian", "another password");
    assert!(!again.status.success());
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(stderr.contains("already exists"), "{stderr}");

    for output in [&first, &again] {
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!all.contains("a long password") && !all.contains("another password"));
    }
}

#[test]
fn user_create_needs_a_role() {
    let dir = tempfile::tempdir().unwrap();
    let output = gantry(dir.path())
        .args(["user", "create", "--username", "adrian", "--password-stdin"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--role"));
}

#[test]
fn user_create_takes_a_role() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    for (name, role, label) in [
        ("watcher", "viewer", "Viewer"),
        ("runner", "operator", "Operator"),
        ("boss", "admin", "Admin"),
    ] {
        let mut child = gantry(dir.path())
            .args([
                "user",
                "create",
                "--password-stdin",
                "--username",
                name,
                "--role",
                role,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "a long password").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("Created the {label} account `{name}`"))
        );
    }
    let output = gantry(dir.path())
        .args(["user", "create", "--username", "x", "--role", "owner"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("owner"));
}

#[test]
fn refuses_an_unknown_config_setting() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gantry.toml");
    std::fs::write(&config, "printers = []\n").unwrap();
    let output = gantry(dir.path())
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("printers"));
}

#[test]
#[cfg(feature = "bambu")]
fn bambu_probe_is_offered_and_takes_no_credentials_on_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let output = gantry(dir.path())
        .args(["bambu-probe", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--host"), "{help}");
    assert!(
        !help.contains("--access-code") && !help.contains("--serial"),
        "{help}"
    );
}
