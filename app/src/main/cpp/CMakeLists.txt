cmake_minimum_required(VERSION 3.22.1)
project(dobutsuhook LANGUAGES CXX)

# Rust is compiled by Cargo for the same Android ABI before Gradle runs.
# Its .a is linked into libdobutsuhook.so: no extra JNI library to load.
set(RUST_STATICLIB
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../engine/target/aarch64-linux-android/release/libdobutsu_solver.a")
add_library(dobutsu_solver STATIC IMPORTED)
set_target_properties(dobutsu_solver PROPERTIES IMPORTED_LOCATION "${RUST_STATICLIB}")

add_library(dobutsuhook SHARED dobutsu_hook.cpp)
target_compile_features(dobutsuhook PRIVATE cxx_std_20)
target_compile_options(dobutsuhook PRIVATE -fvisibility=hidden -fno-exceptions -fno-rtti)
target_link_libraries(dobutsuhook PRIVATE dobutsu_solver log android dl)
