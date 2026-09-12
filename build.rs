use std::{env, fs, path::Path, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // The bundle references ui/*.ui, compiled here from the Blueprint files, the icons and
    // the stylesheet under data/, so all of them are gathered into one directory for it.
    let resource_dir = out.join("resources");
    let ui_out = resource_dir.join("ui");
    fs::create_dir_all(&ui_out).unwrap();

    let mut blps: Vec<PathBuf> = fs::read_dir("data/ui")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "blp"))
        .collect();
    blps.sort();
    let status = Command::new("blueprint-compiler")
        .arg("batch-compile")
        .arg(&ui_out)
        .arg("data/ui")
        .args(&blps)
        .status()
        .expect("blueprint-compiler not found; install it (dnf install blueprint-compiler)");
    assert!(status.success(), "blueprint-compiler failed");

    copy_dir(Path::new("data/icons"), &resource_dir.join("icons"));
    fs::copy("data/style.css", resource_dir.join("style.css")).unwrap();
    fs::copy(
        "data/io.github.sachesi.locus.metainfo.xml",
        resource_dir.join("io.github.sachesi.locus.metainfo.xml"),
    )
    .unwrap();
    fs::copy(
        "data/locus.gresource.xml",
        resource_dir.join("locus.gresource.xml"),
    )
    .unwrap();

    glib_build_tools::compile_resources(
        &[resource_dir.to_str().unwrap()],
        resource_dir.join("locus.gresource.xml").to_str().unwrap(),
        "locus.gresource",
    );

    println!("cargo:rerun-if-changed=data/ui");
    println!("cargo:rerun-if-changed=data/icons");
    println!("cargo:rerun-if-changed=data/style.css");
    println!("cargo:rerun-if-changed=data/io.github.sachesi.locus.metainfo.xml");
    println!("cargo:rerun-if-changed=data/locus.gresource.xml");
    // option_env! bakes this in, so a prefix change has to force a rebuild.
    println!("cargo:rerun-if-env-changed=LOCUS_LOCALEDIR");
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            fs::copy(e.path(), to).unwrap();
        }
    }
}
