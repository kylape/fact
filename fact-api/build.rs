fn main() -> anyhow::Result<()> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &["../third_party/stackrox/proto/internalapi/sensor/virtual_machine_iservice.proto"],
            &["../third_party/stackrox/proto"],
        )?;
    Ok(())
}
