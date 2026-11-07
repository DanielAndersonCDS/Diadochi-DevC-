fn main() {
    // Embed assets/icon.ico into the .exe (Explorer, taskbar, Alt-Tab).
    // Never fails the build: if no resource compiler is found you just get the default icon.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed exe icon: {e}");
        }
    }
}
