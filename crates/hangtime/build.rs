//! Embeds the app icon and version information in the Windows exe, so
//! Explorer, the Start menu, and Add/Remove Programs show them.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../../icons/windows/hangtime.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (numbers[0], numbers[1], numbers[2]);

    let mut rc = String::new();
    writeln!(rc, "1 ICON \"{}\"", rc_path(&icon)).unwrap();
    write!(
        rc,
        r#"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Hangtime contributors"
      VALUE "FileDescription", "Hangtime push-to-talk radio"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "hangtime"
      VALUE "LegalCopyright", "MIT OR Apache-2.0"
      VALUE "OriginalFilename", "hangtime.exe"
      VALUE "ProductName", "Hangtime"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
    .unwrap();

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("hangtime.rc");
    fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

/// A path as a resource-script string: backslashes doubled.
fn rc_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}
