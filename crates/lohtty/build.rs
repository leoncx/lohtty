//! Embeds the application icon and version information into the Windows
//! executable. gpui loads the window icon from resource ID 1.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/lohtty.ico");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("../../assets/icons/lohtty.ico", "1")
            .set("ProductName", "lohtty")
            .set("FileDescription", "lohtty terminal")
            .set("InternalName", "lohtty.exe")
            .set("OriginalFilename", "lohtty.exe");
        if let Err(e) = res.compile() {
            // Missing resource compiler (rc.exe / llvm-rc): build without the
            // icon rather than failing.
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
