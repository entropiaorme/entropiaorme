include!("src/command_acl.rs");

fn main() {
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let permissions_dir = out_dir.join("app-permissions");
    std::fs::create_dir_all(&permissions_dir).expect("create application permissions directory");

    let mut permissions = String::from(
        "\"$schema\" = \"schemas/schema.json\"\n\n[[permission]]\n\
         identifier = \"trusted-commands\"\n\
         description = \"Allows the full application command surface to trusted windows.\"\n\
         commands.allow = [\n",
    );
    for command in app_commands() {
        permissions.push_str(&format!("  \"{command}\",\n"));
    }
    permissions.push_str(
        "]\n\n[[permission]]\n\
         identifier = \"sale-capture-commands\"\n\
         description = \"Allows only the read and the dismissal the capture overlay uses.\"\n\
         commands.allow = [\n",
    );
    for command in SALE_CAPTURE_COMMANDS {
        permissions.push_str(&format!("  \"{command}\",\n"));
    }
    permissions.push_str(
        "]\n\n[[permission]]\n\
         identifier = \"cartography-commands\"\n\
         description = \"Allows only the reads and write used by the cartography overlay.\"\n\
         commands.allow = [\n",
    );
    for command in CARTOGRAPHY_COMMANDS {
        permissions.push_str(&format!("  \"{command}\",\n"));
    }
    permissions.push_str(
        "]\n\n[[permission]]\n\
         identifier = \"navigation-commands\"\n\
         description = \"Allows the route HUD's navigation controls and radar reads.\"\n\
         commands.allow = [\n",
    );
    for command in NAVIGATION_COMMANDS {
        permissions.push_str(&format!("  \"{command}\",\n"));
    }
    permissions.push_str(
        "]\n\n[[permission]]\n\
         identifier = \"radar-guidance-commands\"\n\
         description = \"Allows the click-through radar renderer's two read commands.\"\n\
         commands.allow = [\n",
    );
    for command in RADAR_GUIDANCE_COMMANDS {
        permissions.push_str(&format!("  \"{command}\",\n"));
    }
    permissions.push_str("]\n");
    std::fs::write(permissions_dir.join("commands.toml"), permissions)
        .expect("write application command permissions");

    let permission_pattern: &'static str = Box::leak(
        permissions_dir
            .join("**/*")
            .to_string_lossy()
            .into_owned()
            .into_boxed_str(),
    );
    let manifest = tauri_build::AppManifest::new().permissions_path_pattern(permission_pattern);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("build Tauri application with command ACL");
}
