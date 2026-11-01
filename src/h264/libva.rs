//! libva's C API as bindgen writes it from the system's headers (`build.rs`):
//! the general API and H.264's buffers, each enum a module of its constants.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/libva.rs"));
