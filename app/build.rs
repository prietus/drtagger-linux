// Compiles the Blueprint files to GtkBuilder XML in OUT_DIR, then bundles
// them (and anything else listed in resources.gresource.xml) into the
// GResource that main.rs registers.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ui_out = out.join("ui");
    std::fs::create_dir_all(&ui_out).unwrap();

    let mut blueprints: Vec<PathBuf> = std::fs::read_dir("resources/ui")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "blp"))
        .collect();
    blueprints.sort();
    for bp in &blueprints {
        println!("cargo::rerun-if-changed={}", bp.display());
    }

    let status = Command::new("blueprint-compiler")
        .arg("batch-compile")
        .arg(&ui_out)
        .arg("resources/ui")
        .args(&blueprints)
        .status()
        .expect("blueprint-compiler not found (install blueprint-compiler)");
    assert!(status.success(), "blueprint-compiler failed");

    // Translations: po/<lang>.po → OUT_DIR/locale/<lang>/LC_MESSAGES/drtagger.mo
    // (development runs read them there; installs put them in share/locale).
    println!("cargo::rerun-if-changed=../po");
    let locale = out.join("locale");
    for entry in std::fs::read_dir("../po").into_iter().flatten().flatten() {
        let po = entry.path();
        if po.extension().is_none_or(|e| e != "po") {
            continue;
        }
        let lang = po.file_stem().unwrap().to_string_lossy().into_owned();
        let dir = locale.join(&lang).join("LC_MESSAGES");
        std::fs::create_dir_all(&dir).unwrap();
        let ok = Command::new("msgfmt")
            .arg("--check-format")
            .arg("-o")
            .arg(dir.join("drtagger.mo"))
            .arg(&po)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            println!("cargo::warning=msgfmt failed for {} (install gettext)", po.display());
        }
    }
    println!("cargo::rustc-env=DRTAGGER_DEV_LOCALEDIR={}", locale.display());

    glib_build_tools::compile_resources(
        &[out.to_str().unwrap(), "resources"],
        "resources/resources.gresource.xml",
        "drtagger.gresource",
    );
}
