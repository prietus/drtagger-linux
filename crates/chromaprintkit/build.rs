//! Builds the vendored Chromaprint 1.5.1 the way drtagger for Mac does
//! (internal avresample resampler, NDEBUG, HAVE_LRINTF), with KissFFT in
//! place of Accelerate's vDSP.

use std::path::Path;

fn main() {
    let root = Path::new("vendor/chromaprint-1.5.1");
    let src = root.join("src");
    let kiss = root.join("kissfft");
    println!("cargo:rerun-if-changed=vendor");

    let mut cpp = cc::Build::new();
    cpp.cpp(true)
        .std("c++17")
        .define("NDEBUG", "1")
        .define("HAVE_LRINTF", "1")
        .define("USE_KISSFFT", "1")
        .define("CHROMAPRINT_NODLL", "1")
        .include(&src)
        // The vendored public header, not a system chromaprint.h.
        .include(src.join("include"))
        .include(src.join("utils"))
        .include(&kiss)
        .warnings(false);
    for f in [
        "audio_processor.cpp",
        "chroma.cpp",
        "chroma_filter.cpp",
        "chroma_resampler.cpp",
        "chromaprint.cpp",
        "fft.cpp",
        "fft_lib_kissfft.cpp",
        "fingerprint_calculator.cpp",
        "fingerprint_compressor.cpp",
        "fingerprint_decompressor.cpp",
        "fingerprint_matcher.cpp",
        "fingerprinter.cpp",
        "fingerprinter_configuration.cpp",
        "image_builder.cpp",
        "silence_remover.cpp",
        "simhash.cpp",
        "spectrum.cpp",
        "utils/base64.cpp",
    ] {
        cpp.file(src.join(f));
    }
    cpp.compile("chromaprint");

    // After the C++ library: static archives link in emission order and
    // the C++ code calls into these.
    // glibc hides M_PI under strict C11; Apple's headers do not.
    let mut c = cc::Build::new();
    c.std("c11")
        .define("_DEFAULT_SOURCE", None)
        .define("NDEBUG", "1")
        .define("HAVE_LRINTF", "1")
        .define("USE_KISSFFT", "1")
        .include(&src)
        .include(src.join("include"))
        .include(&kiss)
        .warnings(false)
        .file(src.join("avresample/resample2.c"))
        .file(kiss.join("kiss_fft.c"))
        .file(kiss.join("tools/kiss_fftr.c"))
        .compile("chromaprint_c");
}
