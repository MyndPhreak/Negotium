fn main() {
    // The UI tests locate elements by accessible label through the ElementHandle API,
    // which needs debug info in the generated code. Keep release builds lean.
    let debug_info = std::env::var("PROFILE").map(|p| p != "release").unwrap_or(true);
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug_info);
    slint_build::compile_with_config("ui/main.slint", config).expect("compile Slint UI");

    // The exe's icon. Fail the build rather than ship an exe without one (no resource compiler).
    println!("cargo:rerun-if-changed=assets/negotium.ico");
    embed_resource::compile("assets/negotium.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed the exe icon");
}
