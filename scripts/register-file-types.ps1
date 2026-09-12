$ErrorActionPreference = 'Stop'
$appPath = (Resolve-Path -LiteralPath "$PSScriptRoot/../FeatherPad.exe").Path
$types = Import-Csv -LiteralPath "$PSScriptRoot/../assets/file-types.tsv" -Delimiter "`t"

if (-not ('FeatherPadFileIcons' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class FeatherPadFileIcons {
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconEx(string file, int index, out IntPtr large, out IntPtr small, uint count);
    [DllImport("user32.dll")]
    public static extern bool DestroyIcon(IntPtr icon);
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    public static extern int AssocQueryString(uint flags, uint kind, string extension, string extra, StringBuilder output, ref uint length);
}
'@
}

# Check the packaged executable before publishing any icon references.
foreach ($type in $types) {
    $large = [IntPtr]::Zero
    $small = [IntPtr]::Zero
    $count = [FeatherPadFileIcons]::ExtractIconEx($appPath, -[int]$type.id, [ref]$large, [ref]$small, 1)
    $valid = $count -gt 0 -and $large -ne [IntPtr]::Zero -and $small -ne [IntPtr]::Zero
    if ($large -ne [IntPtr]::Zero) { [void][FeatherPadFileIcons]::DestroyIcon($large) }
    if ($small -ne [IntPtr]::Zero) { [void][FeatherPadFileIcons]::DestroyIcon($small) }
    if (-not $valid) { throw "Missing embedded icon for $($type.label). Build and package FeatherPad first." }
}

function Set-RegistryText($path, $name, $value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($path)
    try { $key.SetValue($name, $value, [Microsoft.Win32.RegistryValueKind]::String) }
    finally { $key.Dispose() }
}

$capabilities = 'Software\FeatherPad\Capabilities'
Set-RegistryText $capabilities 'ApplicationName' 'FeatherPad'
Set-RegistryText $capabilities 'ApplicationDescription' 'Markdown editor, PDF and image viewer'
Set-RegistryText $capabilities 'ApplicationIcon' ('"{0}",-1' -f $appPath)
$registered = 0
$iconPaths = @{}
# A content-addressed ICO path gives Explorer a fresh cache key after an update.
$iconDirectory = Join-Path $env:LOCALAPPDATA 'FeatherPad\FileIcons'
[void][System.IO.Directory]::CreateDirectory($iconDirectory)
foreach ($type in $types) {
    $sourceIcon = Join-Path "$PSScriptRoot/../assets/file-icons" ($type.label.ToLowerInvariant() + '.ico')
    $iconHash = (Get-FileHash -LiteralPath $sourceIcon -Algorithm SHA256).Hash.ToLowerInvariant()
    $installedIcon = Join-Path $iconDirectory ($iconHash + '.ico')
    if (-not (Test-Path -LiteralPath $installedIcon)) {
        Copy-Item -LiteralPath $sourceIcon -Destination $installedIcon
    }
    if ((Get-FileHash -LiteralPath $installedIcon).Hash.ToLowerInvariant() -ne $iconHash) {
        throw "Installed icon differs from source: $($type.label)"
    }
    $icon = '"{0}",0' -f $installedIcon
    $iconPaths[$type.id] = $icon
    foreach ($extension in $type.extensions.Split(' ')) {
        $progId = "FeatherPad.$extension"
        $path = "Software\Classes\$progId"
        Set-RegistryText $path '' ("{0} document" -f $type.name)
        Set-RegistryText "$path\DefaultIcon" '' $icon
        Set-RegistryText "$path\shell\open\command" '' ('"{0}" "%1"' -f $appPath)
        Set-RegistryText "$path\Application" 'ApplicationName' ("FeatherPad ({0})" -f $type.label)
        Set-RegistryText "$path\Application" 'ApplicationIcon' $icon
        # Explorer can retain an extension-specific auto ProgID after Open with.
        # Only repair its icon when it still opens this exact executable.
        $legacy = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\${extension}_auto_file\shell\open\command")
        if ($legacy) {
            try { $ours = $legacy.GetValue('') -eq ('"{0}" "%1"' -f $appPath) }
            finally { $legacy.Dispose() }
            if ($ours) { Set-RegistryText "Software\Classes\${extension}_auto_file\DefaultIcon" '' $icon }
        }
        Set-RegistryText "Software\Classes\.$extension\OpenWithProgids" $progId ''
        Set-RegistryText "$capabilities\FileAssociations" ".$extension" $progId
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("$path\DefaultIcon")
        try { if ($key.GetValue('') -ne $icon) { throw "Icon registration failed: $extension" } }
        finally { $key.Dispose() }
        $registered++
    }
}
Set-RegistryText 'Software\RegisteredApplications' 'FeatherPad' $capabilities
# Reuse the application's folder argument for Explorer workspace entry points.
foreach ($entry in @(
    @{ Class = 'Directory'; Argument = '%1' },
    @{ Class = 'Directory\Background'; Argument = '%V' },
    @{ Class = 'Drive'; Argument = '%1' }
)) {
    $verb = "Software\Classes\$($entry.Class)\shell\FeatherPad"
    # Appending \. keeps a drive root's trailing slash away from the closing quote.
    $folderCommand = '"{0}" "{1}\."' -f $appPath, $entry.Argument
    Set-RegistryText $verb '' 'Open with FeatherPad'
    Set-RegistryText $verb 'Icon' ('"{0}",-1' -f $appPath)
    Set-RegistryText "$verb\command" '' $folderCommand
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("$verb\command")
    try {
        if ($key.GetValue('') -ne $folderCommand) { throw "Folder menu registration failed: $($entry.Class)" }
    } finally { $key.Dispose() }
}
[FeatherPadFileIcons]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output 'Registered Open with FeatherPad for folders, folder backgrounds and drives.'
Write-Output "Verified $($types.Count) embedded icons; registered $registered extensions for the current user."
foreach ($type in $types) {
    foreach ($extension in $type.extensions.Split(' ')) {
        $command = [System.Text.StringBuilder]::new(2048)
        $length = [uint32]$command.Capacity
        $result = [FeatherPadFileIcons]::AssocQueryString(0, 2, ".$extension", 'open', $command, [ref]$length)
        if ($result -ne 0 -or $command.ToString() -ne $appPath) { continue }
        $actual = [System.Text.StringBuilder]::new(2048)
        $length = [uint32]$actual.Capacity
        $result = [FeatherPadFileIcons]::AssocQueryString(0, 15, ".$extension", $null, $actual, [ref]$length)
        $expected = $iconPaths[$type.id]
        if ($result -eq 0 -and $actual.ToString() -eq $expected) {
            Write-Output ".$extension : type icon active"
        } else {
            Write-Warning ".$extension : still using a legacy icon. Choose 'FeatherPad ($($type.label))' in Default apps, not 'FeatherPad.exe'."
        }
    }
}
Write-Output 'Other applications and protected default choices have been preserved.'
