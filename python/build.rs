fn main() {
    // A Python extension resolves the interpreter's symbols when it is loaded (macOS needs
    // `-undefined dynamic_lookup` for that).
    pyo3_build_config::add_extension_module_link_args();
}
