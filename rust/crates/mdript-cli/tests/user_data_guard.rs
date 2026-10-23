use std::{fs, path::PathBuf, process::Command};

#[test]
fn write_commands_reject_live_user_data_aliases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local-results")
        .join(format!("user-guard-{}", std::process::id()));
    let appdata = root.join("isolated-appdata");
    fs::create_dir_all(&appdata).unwrap();
    let paths = [
        appdata.join("MDRIPT/child"),
        appdata.join("other/../mdript/child"),
        appdata.join("mdript/missing/child"),
    ];
    for path in paths {
        for command in ["learn", "update-names", "update-hashes"] {
            let mut run = Command::new(env!("CARGO_BIN_EXE_mdript-cli"));
            run.env("APPDATA", &appdata).arg(command);
            if command == "learn" {
                run.arg("4138").arg(root.join("absent.png"));
            }
            let output = run.arg("--user-data").arg(&path).output().unwrap();
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{command}: {path:?}");
            assert!(
                error.contains("live APPDATA/mdript is not allowed"),
                "{command}: {error}"
            );
            assert!(!path.exists(), "{command}: {path:?}");
        }
    }
    for command in ["learn", "update-names", "update-hashes"] {
        let mut run = Command::new(env!("CARGO_BIN_EXE_mdript-cli"));
        run.env("APPDATA", &appdata).arg(command);
        if command == "learn" {
            run.arg("4138").arg(root.join("absent.png"));
        }
        let output = run.arg("--user-data").arg("relative").output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--user-data must be absolute"));
    }
}
