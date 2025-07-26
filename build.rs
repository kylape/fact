fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile_protos(
        &["./stackrox/proto/internalapi/sensor/virtual_machine_iservice.proto"],
        &["./stackrox/proto", "./proto/"],
    )?;
    Ok(())
}
