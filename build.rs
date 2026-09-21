fn main() {
    // app icon + version info inside the Windows .exe (checks the *target*, not the machine building it)
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Right Panel");
        res.set("FileDescription", "Right Panel");
        if let Err(error) = res.compile() {
            println!("cargo:warning=Windows resources were skipped: {error}");
        }
    }
}
