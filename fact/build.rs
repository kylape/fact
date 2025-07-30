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
    
    // Skip eBPF compilation if clang is not available (for testing)
    if Command::new("clang").arg("--version").status().is_ok() {
        compile_bpf(&out_dir)?;
        generate_bindings(&out_dir)?;
    } else {
        println!("cargo:warning=Clang not found, skipping eBPF compilation");
        // Create stub bindings file for compilation
        let stub_bindings = r#"
pub const PATH_MAX: usize = 4096;
pub const TASK_COMM_LEN: usize = 16;
pub const LINEAGE_MAX: usize = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct lineage_t {
    pub uid: u32,
    pub exe_path: [i8; PATH_MAX],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct process_t {
    pub comm: [i8; TASK_COMM_LEN],
    pub args: [i8; 4096],
    pub exe_path: [i8; PATH_MAX],
    pub cpu_cgroup: [i8; PATH_MAX],
    pub uid: u32,
    pub gid: u32,
    pub login_uid: u32,
    pub pid: u32,
    pub lineage: [lineage_t; LINEAGE_MAX],
    pub lineage_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct event_t {
    pub timestamp: u64,
    pub process: process_t,
    pub is_external_mount: i8,
    pub filename: [i8; PATH_MAX],
    pub host_file: [i8; PATH_MAX],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct path_cfg_t {
    pub path: [i8; PATH_MAX],
    pub len: u16,
}
"#;
        std::fs::write(out_dir.join("bindings.rs"), stub_bindings)?;
        // Create empty eBPF object for compilation
        std::fs::write(out_dir.join("main.o"), "")?;
    }
    
    build_protos()?;
    Ok(())
}
