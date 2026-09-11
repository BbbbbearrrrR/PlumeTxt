fn main() {
    println!("cargo:rerun-if-changed=assets/feather.ico");
    println!("cargo:rerun-if-changed=assets/file-types.tsv");
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon("assets/feather.ico");
    for line in std::fs::read_to_string("assets/file-types.tsv")
        .unwrap()
        .lines()
        .skip(1)
    {
        let fields: Vec<_> = line.split('\t').collect();
        let icon = format!("assets/file-icons/{}.ico", fields[1].to_ascii_lowercase());
        println!("cargo:rerun-if-changed={icon}");
        resources.set_icon_with_id(&icon, fields[0]);
    }
    resources
        .set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<assemblyIdentity version="0.2.0.0" processorArchitecture="*" name="FeatherPad" type="win32"/>
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>"#)
        .set("ProductName", "FeatherPad")
        .set("FileDescription", "FeatherPad — Markdown & PDF")
        .compile()
        .expect("compile Windows icon resource");
}
