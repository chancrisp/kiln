#Requires -Version 7
<#
.SYNOPSIS
  Stop unless every program, DLL and MSI under -Path has a valid Authenticode
  signature.

.DESCRIPTION
  Checks each .exe, .dll and .msi under -Path, in every subfolder, with
  Get-AuthenticodeSignature. A file passes only when its status is Valid: it
  is signed, the signature matches the file, and the certificate chains to a
  root Windows trusts.

  With -Packages it also opens what will be shipped: each .zip under -Path
  is unpacked and the programs in it are checked, and each MSI is unpacked
  by an administrative install (msiexec /a, which installs nothing on the
  PC) and the programs it would install are checked.

  Files under a folder named in -Upstream (by default ffmpeg\bin, wherever
  it is) whose names match -UpstreamNames (FFmpeg's own programs and DLLs)
  are another project's binaries, which SignPath Foundation's terms don't
  allow Kiln to sign. They are listed as upstream and never fail the check.
  Any other file there is checked like the rest.

  With -AllowTestCertificate a file also passes when it is signed and the
  signature matches it, but its certificate's root isn't trusted, as with
  SignPath's test signing. The release workflow never passes it for a tag.

  Prints one line per file and exits 1 when any file fails.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Path,
    [switch]$Packages,
    [switch]$AllowTestCertificate,
    [string[]]$Upstream = @('ffmpeg\bin'),
    # Only these names count as upstream there: FFmpeg's own programs and
    # libraries, as package.ps1 ships them. Anything else is checked.
    [string]$UpstreamNames = '^(ffmpeg|ffprobe)\.exe$|^(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale)-\d+\.dll$'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path $Path).Path
$failed = [System.Collections.Generic.List[string]]::new()
$checked = 0

$upstreamFiles = 0

function Test-Signed([System.IO.FileInfo]$file, [string]$shown) {
    $rel = $shown.Substring($shown.IndexOf('!') + 1)
    foreach ($u in $Upstream) {
        # '.' is the top folder of -Path, a ZIP or the installed files.
        $inFolder = if ($u -eq '.') { (Split-Path $rel -Leaf) -eq $rel -or $rel -match '\\Kiln\\[^\\]+$' } else { ('\' + $rel).Contains('\' + $u.TrimEnd('\') + '\') }
        if ($inFolder -and $file.Name -match $UpstreamNames) {
            $s = Get-AuthenticodeSignature -LiteralPath $file.FullName
            Write-Host ('{0,-8} {1}  ({2})' -f 'upstream', $shown, $s.Status)
            $script:upstreamFiles++
            return
        }
    }
    $s = Get-AuthenticodeSignature -LiteralPath $file.FullName
    $ok = $s.Status -eq 'Valid'
    if (-not $ok -and $AllowTestCertificate -and $s.SignerCertificate -and $s.Status -eq 'UnknownError') {
        # Signed and intact; only the root is unknown.
        $ok = $true
    }
    $who = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { 'no signer' }
    Write-Host ('{0,-8} {1}  ({2}; {3})' -f ($(if ($ok) { 'ok' } else { 'FAIL' })), $shown, $s.Status, $who)
    $script:checked++
    if (-not $ok) { $script:failed.Add($shown) }
}

function Test-Folder([string]$folder, [string]$label) {
    Get-ChildItem -LiteralPath $folder -Recurse -File |
        Where-Object { $_.Extension -in '.exe', '.dll', '.msi' } |
        Sort-Object FullName |
        ForEach-Object {
            $rel = [IO.Path]::GetRelativePath($folder, $_.FullName)
            Test-Signed $_ ($label + $rel)
        }
}

Test-Folder $root ''

if ($Packages) {
    $temp = Join-Path ([IO.Path]::GetTempPath()) ("kiln-signatures-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory $temp | Out-Null
    try {
        foreach ($zip in Get-ChildItem -LiteralPath $root -Recurse -File -Filter *.zip) {
            # The source ZIPs hold no programs that are shipped to run.
            if ($zip.Name -match '-(source|ffmpeg-source|symbols)\.zip$') { continue }
            $into = Join-Path $temp $zip.BaseName
            Expand-Archive -LiteralPath $zip.FullName -DestinationPath $into
            Test-Folder $into "$($zip.Name)!"
        }
        foreach ($msi in Get-ChildItem -LiteralPath $root -Recurse -File -Filter *.msi) {
            $into = Join-Path $temp $msi.BaseName
            $p = Start-Process msiexec.exe -ArgumentList @('/a', "`"$($msi.FullName)`"", '/qn', "TARGETDIR=`"$into`"") -Wait -PassThru
            if ($p.ExitCode -ne 0) { throw "msiexec /a $($msi.Name) failed with $($p.ExitCode)." }
            Test-Folder $into "$($msi.Name)!"
        }
    } finally {
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($checked -eq 0) {
    Write-Host "No programs, DLLs or MSIs under $root."
    exit 1
}
if ($failed.Count -gt 0) {
    Write-Host "$($failed.Count) of $checked files are not validly signed: $($failed -join ', ')"
    exit 1
}
$also = if ($upstreamFiles) { " $upstreamFiles upstream files were left as they are." } else { '' }
Write-Host "All $checked files are validly signed.$also"
