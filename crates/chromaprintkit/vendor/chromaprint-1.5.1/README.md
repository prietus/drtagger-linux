Chromaprint 1.5.1 (https://github.com/acoustid/chromaprint, release tarball
sha256 a1aad8fa3b8b18b78d3755b3767faff9abb67242e01b478ec9a64e190f335e1c).

`src/` is the same file set drtagger for Mac vendors (unmodified upstream
sources), with the Accelerate FFT backend (fft_lib_vdsp.*) swapped for the
KissFFT one (fft_lib_kissfft.* and `kissfft/`, from the same tarball's
src/3rdparty). Chromaprint is LGPL 2.1+ (see LICENSE.md); KissFFT is
BSD-3-Clause (kissfft/COPYING).
