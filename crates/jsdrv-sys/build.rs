//! Builds the vendor driver with CMake and generates the bindings.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy(&entry.path(), &target)?;
        } else if entry.file_name() != ".git" {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=CMakeLists.txt");
    println!("cargo:rerun-if-changed=joulescope_driver/CMakeLists.txt");
    println!("cargo:rerun-if-changed=joulescope_driver/src");
    println!("cargo:rerun-if-changed=joulescope_driver/include");
    println!("cargo:rerun-if-changed=patches");
    if !Path::new("joulescope_driver/CMakeLists.txt").exists() {
        panic!("the Joulescope driver is missing; run `git submodule update --init`");
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // The submodule stays as the vendor released it; the fixes in patches/ go
    // onto a copy.
    let source = out_dir.join("joulescope_driver");
    let _ = fs::remove_dir_all(&source);
    copy(Path::new("joulescope_driver"), &source).expect("copying the driver");
    let mut patches: Vec<_> = fs::read_dir("patches")
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    patches.sort();
    for patch in patches {
        let ok = Command::new("patch")
            .args(["-p1", "-s", "-d"])
            .arg(&source)
            .arg("-i")
            .arg(fs::canonicalize(&patch).unwrap())
            .status()
            .expect("running `patch`")
            .success();
        assert!(ok, "{} does not apply", patch.display());
    }
    let out = cmake::Config::new(".")
        .define("JSDRV_SOURCE", &source)
        .build_target("jsdrv")
        .build();
    let build = out.join("build/jsdrv");
    println!(
        "cargo:rustc-link-search=native={}",
        build.join("src").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        build.join("third-party/miniz").display()
    );
    println!("cargo:rustc-link-lib=static=jsdrv");
    println!("cargo:rustc-link-lib=static=miniz");
    // The driver's bundled libusb finds devices through udev.
    println!("cargo:rustc-link-lib=udev");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=m");

    bindgen::Builder::default()
        .header("joulescope_driver/include/jsdrv.h")
        .clang_arg("-Ijoulescope_driver/include")
        .allowlist_function("jsdrv_.*")
        .allowlist_type("jsdrv_.*")
        .allowlist_var("JSDRV_.*")
        .prepend_enum_name(false)
        .generate_comments(false)
        .generate()
        .expect("bindings for jsdrv.h")
        .write_to_file(out_dir.join("bindings.rs"))
        .unwrap();
}
