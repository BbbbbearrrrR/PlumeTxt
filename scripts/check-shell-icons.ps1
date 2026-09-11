$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/register-file-types.ps1"
Add-Type -AssemblyName System.Drawing
if (-not ('FeatherPadShellCheck' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class FeatherPadShellCheck {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct FileInfo {
        public IntPtr icon;
        public int index;
        public uint attributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string name;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string type;
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SHGetFileInfo(string path, uint attributes, ref FileInfo info, uint size, uint flags);
}
'@
}
$outputDirectory = [System.IO.Path]::GetFullPath("$PSScriptRoot/../tmp/shell-icon-check")
[void][System.IO.Directory]::CreateDirectory($outputDirectory)
foreach ($extension in 'md', 'toml') {
    $type = $types | Where-Object { $_.extensions.Split(' ') -contains $extension }
    $expectedPath = $iconPaths[$type.id].Substring(1).Split('"')[0]
    $file = Join-Path $outputDirectory "example.$extension"
    if (-not (Test-Path -LiteralPath $file)) { [System.IO.File]::WriteAllText($file, '') }
    $info = New-Object FeatherPadShellCheck+FileInfo
    $result = [FeatherPadShellCheck]::SHGetFileInfo($file, 0, [ref]$info, [System.Runtime.InteropServices.Marshal]::SizeOf($info), 0x100)
    if ($result -eq [IntPtr]::Zero -or $info.icon -eq [IntPtr]::Zero) { throw "Shell returned no icon: .$extension" }
    $large = [IntPtr]::Zero
    $small = [IntPtr]::Zero
    $actualBitmap = $null
    $expectedBitmap = $null
    try {
        [void][FeatherPadFileIcons]::ExtractIconEx($expectedPath, 0, [ref]$large, [ref]$small, 1)
        $actualBitmap = [System.Drawing.Icon]::FromHandle($info.icon).ToBitmap()
        $expectedBitmap = [System.Drawing.Icon]::FromHandle($large).ToBitmap()
        $actualBitmap.Save((Join-Path $outputDirectory "$extension-shell.png"))
        $expectedBitmap.Save((Join-Path $outputDirectory "$extension-expected.png"))
        if ($actualBitmap.Size -ne $expectedBitmap.Size) { throw "Shell icon size mismatch: .$extension" }
        for ($y=0; $y -lt $actualBitmap.Height; $y++) {
            for ($x=0; $x -lt $actualBitmap.Width; $x++) {
                $actualPixel = $actualBitmap.GetPixel($x,$y)
                $expectedPixel = $expectedBitmap.GetPixel($x,$y)
                # Shell alpha premultiplication can round RGB channels by one.
                if ($actualPixel.A -ne $expectedPixel.A -or
                    [Math]::Abs([int]$actualPixel.R - $expectedPixel.R) -gt 1 -or
                    [Math]::Abs([int]$actualPixel.G - $expectedPixel.G) -gt 1 -or
                    [Math]::Abs([int]$actualPixel.B - $expectedPixel.B) -gt 1) {
                    throw "Shell still returns an unexpected icon for .$extension; see $outputDirectory"
                }
            }
        }
        Write-Output ".$extension : actual Shell icon matches the new feather icon (RGB rounding tolerance: 1/255)"
    } finally {
        if ($actualBitmap) { $actualBitmap.Dispose() }
        if ($expectedBitmap) { $expectedBitmap.Dispose() }
        foreach ($handle in $info.icon,$large,$small) {
            if ($handle -ne [IntPtr]::Zero) { [void][FeatherPadFileIcons]::DestroyIcon($handle) }
        }
    }
}
