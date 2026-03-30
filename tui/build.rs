fn main() {
    connectrpc_build::Config::new()
        .files(&[
            "proto/types/v1/types.proto",
            "proto/querier/v1/querier.proto",
        ])
        .includes(&["proto/"])
        .include_file("_connectrpc.rs")
        .compile()
        .unwrap();
}
