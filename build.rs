fn main() {
    slint_build::compile("ui/app.slint").expect("Failed to compile Slint UI definition");

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icons/app_icon.ico");
        let _ = res.compile();
    }
}
