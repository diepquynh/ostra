//! `ostra plugin add` against a scratch data dir (HANDOVER 10.10, Rule PL1).

use ostra_core::config::{WorkspaceSettings, load_toml_required};
use ostra_core::ids::WorkspaceId;
use ostra_core::plugin::PluginConfig;
use ostra_server::app::{add_plugin, open_registry};

fn entry(name: &str) -> PluginConfig {
    PluginConfig {
        name: name.into(),
        command: vec!["./bin/gate".into(), "--stdio".into()],
        env: [("GATE_MODE".to_string(), "strict".to_string())].into(),
        enabled: true,
        timeout_secs: 60,
    }
}

#[test]
fn add_registers_the_plugin_and_keeps_the_workspace_approved() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: this test binary runs one test, before any thread reads the environment.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", tmp.path().join("config.toml"));
        std::env::set_var("OSTRA_DATA_DIR", tmp.path().join("data"));
        std::env::set_var("OSTRA_MASTER_KEY_FILE", tmp.path().join("master.key"));
    }
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(root.join("sub/dir")).unwrap();
    let registry = open_registry().unwrap();
    registry
        .add_workspace(&WorkspaceId::new(), "ws", &root)
        .unwrap();
    let mut settings = WorkspaceSettings::seeded("ws");
    settings.limits.session_budget_usd = 7.5;
    ostra_workspace::trust::create_workspace(&registry, &root, &settings, false).unwrap();

    let added = add_plugin(&root.join("sub/dir"), entry("release-gate")).unwrap();
    assert!(
        ostra_core::paths::is_inside(&added.workspace, &root)
            && ostra_core::paths::is_inside(&root, &added.workspace)
    );
    // Rule PL1: the user's own registration keeps an approved folder file approved.
    assert!(added.approved);
    let file: WorkspaceSettings =
        load_toml_required(&ostra_core::paths::workspace_toml(&root)).unwrap();
    assert_eq!(file.plugins, vec![entry("release-gate")]);
    // Rule A2: the save keeps the registry's limits.
    let mut s = file;
    ostra_workspace::trust::overlay(&registry, &root, &mut s);
    assert_eq!(s.limits.session_budget_usd, 7.5);

    let dup = add_plugin(&root, entry("release-gate")).err().unwrap();
    assert!(dup.to_string().contains("already has a plugin"), "{dup}");
    let own = add_plugin(&root, entry("ostra")).err().unwrap();
    assert!(own.to_string().contains("is Ostra's own"), "{own}");
    let mut bad = entry("gate");
    bad.timeout_secs = 0;
    let bad = add_plugin(&root, bad).err().unwrap();
    assert!(bad.to_string().contains("timeout"), "{bad}");
    let outside = add_plugin(tmp.path(), entry("gate")).err().unwrap();
    assert!(
        outside.to_string().contains("no registered workspace"),
        "{outside}"
    );
}
