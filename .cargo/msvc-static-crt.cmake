# CMake toolchain file for the C libraries Kiln builds with CMake (libFLAC
# through libflac-sys, libopus through opusic-sys). It asks for the static C
# runtime, to match `-C target-feature=+crt-static` in config.toml. Without it
# CMake compiles them against the DLL runtime and the link warns or fails.
# Debug builds of Kiln still use the release static runtime, as Rust does.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
# libopus ignores the variable above and reads its own option.
set(OPUS_STATIC_RUNTIME ON CACHE BOOL "Static C runtime" FORCE)
