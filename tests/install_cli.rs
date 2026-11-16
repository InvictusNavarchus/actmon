use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    root: tempfile::TempDir,
    repo: PathBuf,
    bin: PathBuf,
    tools: PathBuf,
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("source checkout");
        let bin = root.path().join("installed bin");
        let tools = root.path().join("tools");
        fs::create_dir_all(repo.join("scripts")).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&tools).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install.sh"),
            repo.join("scripts/install.sh"),
        )
        .unwrap();
        executable(
            &tools.join("rustc"),
            "#!/bin/sh\necho 'host: native-test'\n",
        );
        executable(
            &tools.join("cargo"),
            r#"#!/bin/bash
set -eu
printf '%s\n' "$@" > "$FIXTURE_ROOT/build-args"
[[ ${FAIL_BUILD:-0} == 0 ]] || exit 1
target_dir=''
target=''
while (($#)); do
    case "$1" in
        --target-dir) target_dir=$2; shift 2 ;;
        --target) target=$2; shift 2 ;;
        *) shift ;;
    esac
done
mkdir -p "$target_dir/$target/release"
cp "$FIXTURE_BINARY" "$target_dir/$target/release/actmon"
"#,
        );
        // Any accidental attempt to deploy or request privileges fails the test.
        for name in ["sudo", "systemctl", "udevadm"] {
            executable(
                &tools.join(name),
                "#!/bin/sh\ntouch \"$FIXTURE_ROOT/unexpected-action\"\nexit 99\n",
            );
        }
        Self {
            root,
            repo,
            bin,
            tools,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("bash");
        let path = std::env::join_paths(
            [self.tools.clone(), self.bin.clone()]
                .into_iter()
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        command
            .arg(self.repo.join("scripts/install.sh"))
            .env("PATH", path)
            .env("FIXTURE_ROOT", self.root.path())
            .env("FIXTURE_BINARY", env!("CARGO_BIN_EXE_actmon"))
            .env("CARGO_TARGET_DIR", self.root.path().join("other-target"))
            .env("CARGO_BUILD_TARGET", "foreign-target")
            .current_dir(self.root.path());
        command
    }

    fn install(&self) -> Command {
        let mut command = self.command();
        command.arg("--bin-dir").arg(&self.bin);
        command
    }

    fn assert_clean(&self) {
        assert!(!self.root.path().join("unexpected-action").exists());
        assert!(fs::read_dir(&self.bin).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".actmon-install.")
        }));
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn installer_builds_from_any_directory_and_replaces_the_binary_atomically() {
    let fixture = Fixture::new();
    success(fixture.install().output().unwrap());
    let binary = fixture.bin.join("actmon");
    let old = fixture.root.path().join("previous-executable");
    fs::hard_link(&binary, &old).unwrap();
    let output = success(fixture.install().output().unwrap());
    assert!(output.contains("Installed actmon"));
    assert!(!output.contains("Note:"));
    assert_ne!(
        fs::metadata(&old).unwrap().ino(),
        fs::metadata(&binary).unwrap().ino()
    );
    assert_eq!(fs::read(&old).unwrap(), fs::read(&binary).unwrap());
    assert_eq!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(
        success(Command::new(&binary).arg("--version").output().unwrap()).starts_with("actmon ")
    );
    let args = fs::read_to_string(fixture.root.path().join("build-args")).unwrap();
    assert!(args.contains("--locked\n--release\n"));
    assert!(args.contains("--target\nnative-test\n"));
    assert!(args.contains(fixture.repo.join("target").to_str().unwrap()));
    fixture.assert_clean();
}

#[test]
fn build_validation_and_copy_failures_preserve_the_installed_binary() {
    let fixture = Fixture::new();
    let binary = fixture.bin.join("actmon");
    executable(&binary, "#!/bin/sh\necho previous\n");
    let before = fs::read(&binary).unwrap();
    assert!(
        !fixture
            .install()
            .env("FAIL_BUILD", "1")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&binary).unwrap(), before);
    let broken = fixture.root.path().join("broken-executable");
    executable(&broken, "#!/bin/sh\nexit 1\n");
    assert!(
        !fixture
            .install()
            .env("FIXTURE_BINARY", broken)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&binary).unwrap(), before);
    executable(
        &fixture.tools.join("install"),
        "#!/bin/bash\nprintf partial > \"${@: -1}\"\nexit 1\n",
    );
    assert!(!fixture.install().output().unwrap().status.success());
    assert_eq!(fs::read(&binary).unwrap(), before);
    fixture.assert_clean();
}

#[test]
fn installer_rejects_symlinks_and_directories_before_building() {
    let fixture = Fixture::new();
    let binary = fixture.bin.join("actmon");
    let other = fixture.root.path().join("other");
    executable(&other, "#!/bin/sh\necho other\n");
    symlink(&other, &binary).unwrap();
    let output = fixture.install().output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
    assert_eq!(fs::read_link(&binary).unwrap(), other);
    fs::remove_file(&binary).unwrap();
    fs::create_dir(&binary).unwrap();
    assert!(!fixture.install().output().unwrap().status.success());
    assert!(binary.is_dir());
    assert!(!fixture.root.path().join("build-args").exists());
    fixture.assert_clean();
}

#[test]
fn installer_help_invalid_options_and_path_shadowing_are_reported() {
    let fixture = Fixture::new();
    assert!(success(fixture.command().arg("--help").output().unwrap()).contains("/usr/local/bin"));
    for option in ["--bin-dir", "--unknown"] {
        assert!(
            !fixture
                .command()
                .arg(option)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert!(!fixture.root.path().join("build-args").exists());
    executable(&fixture.tools.join("actmon"), "#!/bin/sh\necho shadowed\n");
    let output = success(fixture.install().output().unwrap());
    assert!(output.contains("PATH selects"));
    assert!(output.contains(fixture.tools.join("actmon").to_str().unwrap()));
    fixture.assert_clean();
}
