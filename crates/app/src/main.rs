#![windows_subsystem = "windows"]

fn main() -> Result<(), slint::PlatformError> {
    use slint::ComponentHandle;
    let ui = negotium::MainWindow::new()?;
    ui.run()
}
