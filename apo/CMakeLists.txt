cmake_minimum_required(VERSION 3.20)
project(StreameeApo CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
# Runtime C++ statique : audiodg.exe ne doit dépendre d'aucune DLL VC++ à installer.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

add_library(StreameeApo SHARED
    src/dllmain.cpp
    src/StreameeApo.cpp
    src/StreameeApo.def
)
target_compile_definitions(StreameeApo PRIVATE UNICODE _UNICODE NOMINMAX)
target_compile_options(StreameeApo PRIVATE /W4 /permissive-)
target_link_libraries(StreameeApo PRIVATE ole32)
