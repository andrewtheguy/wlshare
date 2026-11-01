//! libva's C API, for the GPU's H.264 encoder (`src/h264/vaapi.rs`): bindings
//! generated with bindgen from the system's `va/va.h` and `va/va_drm.h`, and
//! libva and libva-drm linked dynamically, both found through pkg-config.
//!
//! Generated here rather than taken from a crate so that the bindings are of
//! the headers the daemon is built against and of what it calls alone: the
//! general API and H.264's buffers. A crate of every codec's safe wrappers
//! compiles them all, and one written against an older libva stops compiling
//! when a newer one adds a field to a codec the daemon never uses.

use std::env;
use std::path::PathBuf;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-changed=build.rs");
    let mut includes = Vec::new();
    for library in ["libva", "libva-drm"] {
        let found = pkg_config::probe_library(library).with_context(|| format!("finding {library} through pkg-config"))?;
        includes.extend(found.include_paths);
    }
    let bindings = bindgen::builder()
        .header_contents("libva.h", "#include <va/va.h>\n#include <va/va_drm.h>\n")
        .clang_args(includes.iter().map(|path| format!("-I{}", path.display())))
        .rust_edition(bindgen::RustEdition::Edition2024)
        .derive_default(true)
        .layout_tests(false)
        .constified_enum_module("VA.*")
        .allowlist_function("va.*")
        .allowlist_var("VA.*")
        .allowlist_type("VA.*H264.*|VACodedBufferSegment|VAEncPackedHeaderParameterBuffer")
        .generate()
        .context("generating libva's bindings")?;
    let out = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR is not set")?);
    bindings.write_to_file(out.join("libva.rs")).context("writing libva's bindings")?;
    Ok(())
}
