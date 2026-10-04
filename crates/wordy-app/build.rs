//! Embeds the icon and version information into the Windows executable.
//! Nothing to do on the other platforms: the Mac bundle and the Linux
//! launcher entry carry their icons beside the binary.

fn main() {
    #[cfg(windows)]
    {
        let ico = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/windows/wordy.ico");
        println!("cargo:rerun-if-changed={ico}");
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico)
            .set("ProductName", "Wordy")
            .set("FileDescription", "Wordy, a private writing desk")
            .set("LegalCopyright", "MIT licence");
        // rc.exe comes with the Windows SDK. Without it the build still
        // succeeds; the executable just shows the default icon.
        if let Err(e) = res.compile() {
            println!("cargo:warning=Windows resources not embedded: {e}");
        }
    }
}
