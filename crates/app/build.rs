// Embeds the app icon into phpcraft.exe on Windows.
fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icon/phpcraft.ico");
        resource.set("ProductName", "PhpCraft");
        resource.set("FileDescription", "PhpCraft — PHP & Laravel IDE");
        if let Err(err) = resource.compile() {
            println!("cargo:warning=could not embed the icon: {err}");
        }
    }
}
