fn main() {
    // The UI tests locate elements by accessible label through the ElementHandle API,
    // which needs debug info in the generated code. Keep release builds lean.
    let debug_info = std::env::var("PROFILE").map(|p| p != "release").unwrap_or(true);
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug_info);
    slint_build::compile_with_config("ui/main.slint", config).expect("compile Slint UI");

    // The exe's icon and manifest. Fail the build rather than ship an exe without them (no
    // resource compiler). Every executable of this package gets them, test executables
    // included: the lib imports TaskDialogIndirect, which only comctl32 v6 has, so without the
    // manifest's Common Controls 6 a test executable does not start (STATUS_ENTRYPOINT_NOT_FOUND).
    println!("cargo:rerun-if-changed=assets/negotium.ico");
    println!("cargo:rerun-if-changed=assets/negotium.exe.manifest");
    embed_resource::compile_for_everything("assets/negotium.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed the exe icon and manifest");
}
