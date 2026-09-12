param([string]$CompilerPath, [string]$ShellCertificateThumbprint)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (-not $CompilerPath) {
    $candidates = @(
        "$env:LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe",
        "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe"
    )
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) { $candidates += $command.Source }
    $CompilerPath = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $CompilerPath -or -not (Test-Path -LiteralPath $CompilerPath)) {
    throw 'Install Inno Setup 6.6+ or pass -CompilerPath C:\path\ISCC.exe.'
}
$CompilerPath = (Resolve-Path -LiteralPath $CompilerPath).Path
Push-Location $repo
$oldRustFlags = $env:RUSTFLAGS
try {
    if ($env:CARGO_ENCODED_RUSTFLAGS) { throw 'Unset CARGO_ENCODED_RUSTFLAGS so Setup can build with the static CRT.' }
    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $version = ($metadata.packages | Where-Object name -eq 'plumetxt').version
    $env:RUSTFLAGS = "$oldRustFlags -C target-feature=+crt-static".Trim()
    cargo build --release --locked --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    $payload = Join-Path $metadata.target_directory 'x86_64-pc-windows-msvc/release'
    $shellDefine = @()
    if ($ShellCertificateThumbprint) {
        & "$PSScriptRoot/build-shell-menu.ps1" -CertificateThumbprint $ShellCertificateThumbprint
        $shellDefine = @("/DShellMenuDir=$repo/target/shell-menu")
    }
    $generated = Join-Path $metadata.target_directory 'setup-associations.iss'
    $output = Join-Path $repo 'dist'
    [void](New-Item -ItemType Directory -Force -Path $output)
    $files = [System.Collections.Generic.List[string]]::new()
    $registry = [System.Collections.Generic.List[string]]::new()
    $types = Import-Csv -LiteralPath 'assets/file-types.tsv' -Delimiter "`t"
    foreach ($type in $types) {
        $icon = Join-Path $repo ('assets/file-icons/' + $type.label.ToLowerInvariant() + '.ico')
        $hash = (Get-FileHash -LiteralPath $icon).Hash.ToLowerInvariant()
        $files.Add(('Source: "{0}"; DestDir: "{{app}}\FileIcons"; DestName: "{1}.ico"; Flags: ignoreversion; Tasks: associations' -f $icon, $hash))
        foreach ($extension in $type.extensions.Split(' ')) {
            $id = "PlumeTxt.$extension"
            $base = "Software\Classes\$id"
            $registry.Add(('Root: HKCU; Subkey: "{0}"; ValueType: string; ValueData: "{1} document"; Flags: uninsdeletekey; Tasks: associations' -f $base, $type.name))
            $registry.Add(('Root: HKCU; Subkey: "{0}\DefaultIcon"; ValueType: string; ValueData: """{{app}}\FileIcons\{1}.ico"",0"; Tasks: associations' -f $base, $hash))
            $registry.Add(('Root: HKCU; Subkey: "{0}\shell\open\command"; ValueType: string; ValueData: """{{app}}\PlumeTxt.exe"" ""%1"""; Tasks: associations' -f $base))
            $registry.Add(('Root: HKCU; Subkey: "{0}\Application"; ValueType: string; ValueName: "ApplicationName"; ValueData: "PlumeTxt ({1})"; Tasks: associations' -f $base, $type.label))
            $registry.Add(('Root: HKCU; Subkey: "Software\Classes\.{0}\OpenWithProgids"; ValueType: string; ValueName: "{1}"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty; Tasks: associations' -f $extension, $id))
            $registry.Add(('Root: HKCU; Subkey: "Software\PlumeTxt\Capabilities\FileAssociations"; ValueType: string; ValueName: ".{0}"; ValueData: "{1}"; Tasks: associations' -f $extension, $id))
        }
    }
    $registry.Add('Root: HKCU; Subkey: "Software\PlumeTxt\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "PlumeTxt"; Flags: uninsdeletekey; Tasks: associations')
    $registry.Add('Root: HKCU; Subkey: "Software\PlumeTxt\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Markdown editor, PDF and image viewer"; Tasks: associations')
    $registry.Add('Root: HKCU; Subkey: "Software\PlumeTxt\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\PlumeTxt.exe"",-1"; Tasks: associations')
    $registry.Add('Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "PlumeTxt"; ValueData: "Software\PlumeTxt\Capabilities"; Flags: uninsdeletevalue; Tasks: associations')
    foreach ($entry in @(@('Directory', '%1'), @('Directory\Background', '%V'), @('Drive', '%1'))) {
        $base = "Software\Classes\$($entry[0])\shell\PlumeTxt"
        $registry.Add(('Root: HKCU; Subkey: "{0}"; ValueType: string; ValueData: "Open with PlumeTxt"; Flags: uninsdeletekey; Tasks: foldermenu' -f $base))
        $registry.Add(('Root: HKCU; Subkey: "{0}"; ValueType: string; ValueName: "Icon"; ValueData: """{{app}}\PlumeTxt.exe"",-1"; Tasks: foldermenu' -f $base))
        $registry.Add(('Root: HKCU; Subkey: "{0}\command"; ValueType: string; ValueData: """{{app}}\PlumeTxt.exe"" ""{1}\."""; Tasks: foldermenu' -f $base, $entry[1]))
    }
    [System.IO.File]::WriteAllLines($generated, [string[]](@('[Files]') + $files + @('[Registry]') + $registry), [System.Text.UTF8Encoding]::new($true))
    & $CompilerPath /Q "/DAppVersion=$version" "/DPayloadDir=$payload" "/DOutputDir=$output" "/DGeneratedAssociations=$generated" @shellDefine 'installer/PlumeTxt.iss'
    if ($LASTEXITCODE -ne 0) { throw 'Setup compilation failed.' }
    $setup = Join-Path $output "PlumeTxt-$version-Setup-x64.exe"
    $sha = (Get-FileHash -LiteralPath $setup).Hash.ToLowerInvariant()
    "$sha  $([System.IO.Path]::GetFileName($setup))" | Set-Content -LiteralPath "$setup.sha256" -Encoding ascii
    Get-Item -LiteralPath $setup | Select-Object FullName, Length
} finally {
    $env:RUSTFLAGS = $oldRustFlags
    Pop-Location
}
