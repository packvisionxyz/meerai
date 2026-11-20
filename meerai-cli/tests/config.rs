use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use meerai_common::settings::AppSettings;
use meerai_session::SessionId;
use meerai_store::Store;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("meerai-config-cli-{}", SessionId::new()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_meerai"))
            .current_dir(&self.0)
            .env("MEERAI_DB_PATH", self.0.join("config.db"))
            .env("CONFIG_TEST_API_KEY", "expanded-key-not-for-export")
            .args(args)
            .output()
            .unwrap()
    }

    fn import(&self, yaml: &str) -> Output {
        fs::write(self.0.join("import.yml"), yaml).unwrap();
        self.run(&["config", "import", "import.yml"])
    }

    fn export(&self) -> String {
        let out = self.run(&["config", "export"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn config_round_trips_only_user_settings() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join(".meerai")).unwrap();
    fs::write(
        fixture.0.join(".meerai/settings.yml"),
        "context: {max_steps_per_turn: 1}\n",
    )
    .unwrap();
    let store = Store::open(fixture.0.join("config.db")).unwrap();
    store.config().set("ui_tabs", "unrelated-config").unwrap();
    store.immediate(|tx| {
        tx.execute(
            "INSERT INTO vault_secrets VALUES ('vault-only-provider', 'api', zeroblob(12), zeroblob(32), 0)",
            [],
        )?;
        Ok(())
    }).unwrap();

    let input = "\
active_provider: example
providers:
  example:
    apiKey: '${CONFIG_TEST_API_KEY}'
    options:
      api_key: '{env:CONFIG_TEST_API_KEY}'
    model: user-model
";
    let out = fixture.import(input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let exported = fixture.export();
    for excluded in [
        "context:",
        "unrelated-config",
        "vault-only-provider",
        "expanded-key-not-for-export",
    ] {
        assert!(!exported.contains(excluded));
    }
    let expected: AppSettings = serde_yaml::from_str(input).unwrap();
    assert_eq!(
        serde_yaml::from_str::<AppSettings>(&exported).unwrap(),
        expected
    );

    let dest = Fixture::new();
    assert!(dest.import("active_model: old-model").status.success());
    assert!(dest.import(&exported).status.success());
    assert_eq!(dest.export(), exported);
}

#[test]
fn config_rejects_invalid_imports_without_changing_settings_or_echoing_keys() {
    let fixture = Fixture::new();
    assert!(fixture.import("active_model: keep-me").status.success());
    let previous = fixture.export();
    for (yaml, message) in [
        (
            "providers: {example: {api_key: literal-secret}}",
            "bare environment references",
        ),
        (
            "providers: {example: {options: {apiKey: '${KEY|literal-secret}'}}}",
            "bare environment references",
        ),
        (
            "providers: {example: {options: {api_key: literal-secret}}}",
            "bare environment references",
        ),
        ("providers: [literal-secret", "invalid config YAML"),
        ("providers: literal-secret", "invalid user settings"),
        ("", "expected a user settings mapping"),
    ] {
        let out = fixture.import(yaml);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(!out.status.success());
        assert!(stderr.contains(message), "{stderr}");
        assert!(!stderr.contains("literal-secret"));
        assert!(out.stdout.is_empty());
    }
    assert_eq!(fixture.export(), previous);
}

#[test]
fn config_help_and_invalid_arguments_do_not_need_a_database() {
    let fixture = Fixture::new();
    for args in [
        &["config", "--help"][..],
        &["config", "export", "--help"],
        &["config", "import", "--help"],
    ] {
        let out = fixture.run(args);
        assert!(out.status.success());
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("config export"));
        assert!(help.contains("config import <PATH>"));
    }
    for args in [
        &["config"][..],
        &["config", "import"],
        &["config", "import", "one.yml", "two.yml"],
        &["config", "export", "file.yml"],
    ] {
        assert!(!fixture.run(args).status.success());
    }
    assert!(!fixture.0.join("config.db").exists());
}
