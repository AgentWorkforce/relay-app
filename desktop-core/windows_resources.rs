//! Windows executable resources, shared by the core's and the app's build scripts.

use std::{env, fs, path::PathBuf, process::Command};

/// The product name the build's site profile fixes (docs/workspace-layout.md,
/// "Site profiles"): `DESKTOP_PRODUCT_NAME`, which `scripts/build-gtk.sh` and
/// `build-windows.sh` export when it differs from the default, "Agent Relay".
pub fn product_name() -> String {
    println!("cargo:rerun-if-env-changed=DESKTOP_PRODUCT_NAME");
    let product = env::var("DESKTOP_PRODUCT_NAME")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Agent Relay".into());
    assert!(
        !product.chars().any(|c| c.is_control() || "/\\\"':*?<>|{},".contains(c)) && product.chars().count() <= 64,
        "DESKTOP_PRODUCT_NAME must be a name with no control characters, slashes, quotes or the characters : * ? < > | {{ }} , (a Windows file and installer name), got: {product}"
    );
    product
}

/// On Windows, embeds the app's icon, the product name and the release
/// version in the executables when `AGENT_RELAY_WINDOWS_ICON` names an
/// `.ico`, as `scripts/build-windows.sh` does: Explorer, the Start menu
/// shortcut and the taskbar show the icon, and Properties › Details the
/// version. Without it the executables build as before, with no resources.
pub fn embed(version: &str, product: &str, description: &str) {
    println!("cargo:rerun-if-env-changed=AGENT_RELAY_WINDOWS_ICON");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(icon) = env::var_os("AGENT_RELAY_WINDOWS_ICON").filter(|icon| !icon.is_empty()) else {
        return;
    };
    let icon = PathBuf::from(icon);
    assert!(
        icon.is_file(),
        "AGENT_RELAY_WINDOWS_ICON is not a file: {}",
        icon.display()
    );
    println!("cargo:rerun-if-changed={}", icon.display());

    // FILEVERSION takes four 16-bit numbers: 2026.10.9-rc.1 is 2026,10,9,0.
    let mut numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    numbers.resize(4, 0);
    let numeric = numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let quoted = |text: &str| text.replace('\\', "/").replace('"', "\"\"");
    let script = format!(
        "1 ICON \"{icon}\"\n\
         1 VERSIONINFO\n\
         FILEVERSION {numeric}\n\
         PRODUCTVERSION {numeric}\n\
         BEGIN\n\
         \x20 BLOCK \"StringFileInfo\"\n\
         \x20 BEGIN\n\
         \x20   BLOCK \"040904B0\"\n\
         \x20   BEGIN\n\
         \x20     VALUE \"FileDescription\", \"{description}\"\n\
         \x20     VALUE \"ProductName\", \"{product}\"\n\
         \x20     VALUE \"FileVersion\", \"{version}\"\n\
         \x20     VALUE \"ProductVersion\", \"{version}\"\n\
         \x20   END\n\
         \x20 END\n\
         \x20 BLOCK \"VarFileInfo\"\n\
         \x20 BEGIN\n\
         \x20   VALUE \"Translation\", 0x409, 1200\n\
         \x20 END\n\
         END\n",
        icon = quoted(&icon.display().to_string()),
        product = quoted(product),
        version = quoted(version),
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("agent-relay.rc");
    let object = out.join("agent-relay-resources.o");
    fs::write(&rc, script).unwrap();
    // MSYS2's clang toolchain ships windres (llvm-windres); the script needs
    // no C preprocessor.
    let windres = env::var("WINDRES").unwrap_or_else(|_| "windres".into());
    let status = Command::new(&windres)
        .args(["--no-preprocess", "-O", "coff", "-i"])
        .arg(&rc)
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|error| panic!("could not run {windres}: {error}"));
    assert!(
        status.success(),
        "{windres} could not compile {}",
        rc.display()
    );
    println!("cargo:rustc-link-arg-bins={}", object.display());
}
