pub mod tempopb {
    #![allow(clippy::all, non_snake_case)]
    tonic::include_proto!("tempopb");

    pub mod common {
        pub mod v1 {
            #![allow(clippy::all)]
            tonic::include_proto!("tempopb.common.v1");
        }
    }

    pub mod resource {
        pub mod v1 {
            #![allow(clippy::all)]
            tonic::include_proto!("tempopb.resource.v1");
        }
    }

    pub mod trace {
        pub mod v1 {
            #![allow(clippy::all)]
            tonic::include_proto!("tempopb.trace.v1");
        }
    }
}
