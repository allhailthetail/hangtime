# Link libopus against the static C runtime on Windows, matching Rust's
# crt-static in .cargo/config.toml, so the exe needs no Visual C++
# redistributable. libopus chooses its runtime from this option.
set(OPUS_STATIC_RUNTIME ON CACHE BOOL "")
