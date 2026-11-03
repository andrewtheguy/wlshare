//! NVENC's and CUDA's C API as bindgen writes it from nv-codec-headers
//! (`build.rs`): the encoder's structs and enums, each enum a module of its
//! constants, the CUDA driver's function types, and the struct versions every
//! call is checked by as `wl_nvenc_versions`. Nothing is linked: the driver's
//! libraries are loaded at run time ([`super::nvenc`]).

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code, unused_imports)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/nvenc.rs"));
