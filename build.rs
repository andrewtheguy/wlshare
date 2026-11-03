//! libva's C API, for the GPU's H.264 encoder (`src/h264/vaapi.rs`): bindings
//! generated with bindgen from the system's `va/va.h` and `va/va_drm.h`, and
//! libva and libva-drm linked dynamically, both found through pkg-config.
//!
//! And NVENC's and CUDA's, for NVIDIA's HEVC 4:4:4 encoder
//! (`src/hevc/nvenc.rs`): bindings generated from nv-codec-headers'
//! `nvEncodeAPI.h` and `dynlink_cuda.h`, found through pkg-config as
//! `ffnvcodec`, and nothing linked, since the libraries are the NVIDIA
//! driver's and the daemon loads them at run time where the host has them.
//!
//! Generated here rather than taken from a crate so that the bindings are of
//! the headers the daemon is built against and of what it calls alone: the
//! general API and H.264's buffers. A crate of every codec's safe wrappers
//! compiles them all, and one written against an older libva stops compiling
//! when a newer one adds a field to a codec the daemon never uses.

use std::env;
use std::path::PathBuf;

use anyhow::Context;

/// The struct versions NVENC checks every call's argument by, which the
/// header writes as function-like macros bindgen cannot evaluate: each an
/// enumerator here, which clang does, under the macro's name prefixed `WL_`.
const NVENC_VERSIONS: &[&str] = &[
    "NVENCAPI_VERSION",
    "NV_ENCODE_API_FUNCTION_LIST_VER",
    "NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER",
    "NV_ENC_CAPS_PARAM_VER",
    "NV_ENC_PRESET_CONFIG_VER",
    "NV_ENC_CONFIG_VER",
    "NV_ENC_INITIALIZE_PARAMS_VER",
    "NV_ENC_RECONFIGURE_PARAMS_VER",
    "NV_ENC_CREATE_BITSTREAM_BUFFER_VER",
    "NV_ENC_REGISTER_RESOURCE_VER",
    "NV_ENC_MAP_INPUT_RESOURCE_VER",
    "NV_ENC_PIC_PARAMS_VER",
    "NV_ENC_LOCK_BITSTREAM_VER",
];

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR is not set")?);
    let mut includes = Vec::new();
    for library in ["libva", "libva-drm"] {
        let found = pkg_config::probe_library(library).with_context(|| format!("finding {library} through pkg-config"))?;
        includes.extend(found.include_paths);
    }
    let bindings = bindgen::builder()
        .header_contents("libva.h", "#include <va/va.h>\n#include <va/va_drm.h>\n")
        .clang_args(includes.iter().map(|path| format!("-I{}", path.display())))
        // Every header the two include, so a changed libva reruns this.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_edition(bindgen::RustEdition::Edition2024)
        .derive_default(true)
        .layout_tests(false)
        .constified_enum_module("VA.*")
        .allowlist_function("va.*")
        .allowlist_var("VA.*")
        .allowlist_type("VA.*H264.*|VACodedBufferSegment|VAEncPackedHeaderParameterBuffer")
        .generate()
        .context("generating libva's bindings")?;
    bindings.write_to_file(out.join("libva.rs")).context("writing libva's bindings")?;

    // Headers alone: the library is not linked, only found for its includes.
    let nvenc = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("ffnvcodec")
        .context("finding nv-codec-headers (ffnvcodec, libffmpeg-nvenc-dev) through pkg-config")?;
    let versions: String = NVENC_VERSIONS.iter().map(|name| format!("    WL_{name} = {name},\n")).collect();
    let header = format!(
        "#include <ffnvcodec/dynlink_cuda.h>\n#include <ffnvcodec/nvEncodeAPI.h>\nenum wl_nvenc_versions : uint32_t {{\n{versions}}};\n"
    );
    let bindings = bindgen::builder()
        .header_contents("nvenc.h", &header)
        .clang_args(nvenc.include_paths.iter().map(|path| format!("-I{}", path.display())))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_edition(bindgen::RustEdition::Edition2024)
        .derive_default(true)
        .layout_tests(false)
        .constified_enum_module("_?NV_ENC.*|_?NVENCSTATUS|cudaError_enum|wl_nvenc_versions")
        .allowlist_type("_?NV_ENC.*|_?NVENCSTATUS|_?NV_ENCODE_API_FUNCTION_LIST|GUID|tcu.*|CU.*|cudaError_enum|wl_nvenc_versions")
        .allowlist_var("NVENC.*|NV_ENC_.*")
        // The GUIDs are `static const` objects, which bindgen cannot give
        // values; the few the daemon names are written out in nvenc.rs.
        .blocklist_item("NV_ENC_.*_GUID")
        .generate()
        .context("generating NVENC's bindings")?;
    bindings.write_to_file(out.join("nvenc.rs")).context("writing NVENC's bindings")?;
    Ok(())
}
