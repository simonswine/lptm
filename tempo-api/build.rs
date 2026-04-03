fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "proto/tempo.proto",
                "proto/common/v1/common.proto",
                "proto/resource/v1/resource.proto",
                "proto/trace/v1/trace.proto",
            ],
            &["proto/"],
        )?;
    Ok(())
}
