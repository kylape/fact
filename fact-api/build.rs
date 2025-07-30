fn main() -> anyhow::Result<()> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "../third_party/stackrox/proto/internalapi/sensor/virtual_machine_iservice.proto",
                "../third_party/stackrox/proto/internalapi/sensor/sfa.proto",
                "../third_party/stackrox/proto/internalapi/sensor/sfa_iservice.proto",
                "../third_party/stackrox/proto/internalapi/sensor/collector.proto",
                "../third_party/stackrox/proto/storage/virtual_machine.proto",
                "../third_party/stackrox/proto/storage/image.proto",
                "../third_party/stackrox/proto/storage/cve.proto",
                "../third_party/stackrox/proto/storage/vulnerability.proto",
            ],
            &["../third_party/stackrox/proto", "/usr/local/include"],
        )?;
    Ok(())
}
