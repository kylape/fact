use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn compile_bpf(out_dir: &Path) -> anyhow::Result<()> {
    let obj = out_dir
        .join("main.o")
        .into_os_string()
        .into_string()
        .unwrap();
    let ec = Command::new("clang")
        .args([
            "-target",
            "bpf",
            "-O2",
            "-g",
            "-c",
            "-Wall",
            "-Werror",
            "../fact-ebpf/main.c",
            "-o",
            &obj,
        ])
        .status()?;
    if ec.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to compile '{ec}'"))
    }
}

fn generate_bindings(out_dir: &Path) -> anyhow::Result<()> {
    let bindings = bindgen::Builder::default()
        .header("../fact-ebpf/types.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Failed to generate bindings");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings");
    Ok(())
}

fn build_protos() -> anyhow::Result<()> {
    let proto_path = Path::new("../proto");
    let stackrox_path = Path::new("../third_party/stackrox/proto");
    
    if proto_path.exists() && stackrox_path.exists() {
        tonic_build::configure().build_server(true).compile_protos(
            &["../third_party/stackrox/proto/internalapi/sensor/virtual_machine_iservice.proto"],
            &["../third_party/stackrox/proto", "../proto/"],
        )?;
    }
    
    Ok(())
}

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-changed=../fact-ebpf/");
    let out_dir: PathBuf = env::var("OUT_DIR")?.into();
    compile_bpf(&out_dir)?;
    generate_bindings(&out_dir)?;
    build_protos()?;
    Ok(())
}
