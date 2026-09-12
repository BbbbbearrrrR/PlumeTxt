param([string]$CertificateThumbprint)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$vswhere = "${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
$vs = & $vswhere -all -products '*' -property installationPath | Where-Object { Test-Path -LiteralPath "$_/VC/Tools/MSVC" } | Select-Object -First 1
if (-not $vs) { throw 'Visual Studio C++ Build Tools are required.' }
$vc = Get-ChildItem -LiteralPath "$vs/VC/Tools/MSVC" -Directory | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
$kit = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots').KitsRoot10
$sdk = Get-ChildItem -LiteralPath "$kit/Include" -Directory | Where-Object Name -Match '^10\.0\.' | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
$cl = "$($vc.FullName)/bin/Hostx64/x64/cl.exe"
$tools = "$kit/bin/$($sdk.Name)/x64"
$output = Join-Path $repo 'target/shell-menu'
$package = Join-Path $output 'package'
[void](New-Item -ItemType Directory -Force -Path $package)
$certificate = $null
if ($CertificateThumbprint) {
    $certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint"
    if (-not $certificate.HasPrivateKey) { throw 'The signing certificate has no private key.' }
}
$include = @("/I$($vc.FullName)/include")
foreach ($folder in @('ucrt', 'shared', 'um', 'winrt')) { $include += "/I$($sdk.FullName)/$folder" }
$libraries = @("/LIBPATH:$($vc.FullName)/lib/x64", "/LIBPATH:$kit/Lib/$($sdk.Name)/ucrt/x64", "/LIBPATH:$kit/Lib/$($sdk.Name)/um/x64", 'ole32.lib', 'shell32.lib', 'shlwapi.lib', 'user32.lib', 'uuid.lib', 'runtimeobject.lib')
$flags = @('/nologo', '/std:c++17', '/EHsc', '/MT', '/O1', '/W4', '/WX', '/guard:cf', '/DUNICODE', '/D_UNICODE', '/D_WIN32_WINNT=0x0A00')
$source = Join-Path $repo 'installer/shell-menu.cpp'
& $cl @flags @include /LD $source "/Fo$output/menu.obj" "/Fe$output/PlumeTxt.ShellMenu.dll" /link @libraries /DYNAMICBASE /NXCOMPAT '/EXPORT:DllGetClassObject,PRIVATE' '/EXPORT:DllCanUnloadNow,PRIVATE'
if ($LASTEXITCODE -ne 0) { throw 'Shell menu DLL compilation failed.' }
& $cl @flags @include /DSHELL_MENU_TEST $source "/Fo$output/check.obj" "/Fe$output/check.exe" /link @libraries /DYNAMICBASE /NXCOMPAT
if ($LASTEXITCODE -ne 0) { throw 'Shell menu check compilation failed.' }
& "$output/check.exe"
if ($LASTEXITCODE -ne 0) { throw 'Shell menu COM/argument checks failed.' }
[xml]$manifest = Get-Content -LiteralPath "$repo/installer/ShellMenu.xml" -Raw
$metadata = cargo metadata --manifest-path "$repo/Cargo.toml" --no-deps --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
$manifest.Package.Identity.Version = ($metadata.packages | Where-Object name -eq 'plumetxt').version + '.0'
if ($certificate) { $manifest.Package.Identity.Publisher = $certificate.Subject }
$manifest.Save("$package/AppxManifest.xml")
Copy-Item -LiteralPath "$repo/assets/feather.png" -Destination "$output/ShellMenu.png" -Force
& "$tools/makeappx.exe" pack /o /d $package /nv /p "$output/PlumeTxt.ShellMenu.msix"
if ($LASTEXITCODE -ne 0) { throw 'Shell menu identity packaging failed.' }
if ($certificate) {
    foreach ($file in @('PlumeTxt.ShellMenu.dll', 'PlumeTxt.ShellMenu.msix')) {
        & "$tools/signtool.exe" sign /fd SHA256 /sha1 $certificate.Thumbprint /s My "$output/$file"
        if ($LASTEXITCODE -ne 0) { throw "Signing $file failed." }
        & "$tools/signtool.exe" verify /pa "$output/$file"
        if ($LASTEXITCODE -ne 0) { throw "Signature verification for $file failed." }
    }
} else {
    Write-Warning 'Unsigned development output only. A trusted signing certificate is required before including this menu in Setup.'
}
Get-Item -LiteralPath "$output/PlumeTxt.ShellMenu.dll", "$output/PlumeTxt.ShellMenu.msix" | Select-Object FullName, Length
