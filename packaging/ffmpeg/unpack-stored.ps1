#Requires -Version 7
<#
.SYNOPSIS
  Check the stored FFmpeg build against its record and unpack it for
  packaging.

.DESCRIPTION
  .github/workflows/ffmpeg-lgpl.yml builds the LGPL FFmpeg once and keeps it
  on the release ffmpeg-lgpl of this repository, as four files:

    ffmpeg-lgpl-win64.zip  bin\, licenses\, LICENSE.txt and BUILD.txt
    ffmpeg-source.zip      the source of that build
    SHA256SUMS             the SHA-256 of the two zips
    builds-commit.txt      the FFmpeg-Builds commit it was built with

  Give the folder those files were downloaded to as -From. Each zip must
  match the SHA-256 recorded for it, and the BUILD.txt inside the win64 zip
  must name the commit in builds-commit.txt; otherwise nothing is unpacked.
  The win64 zip is then unpacked into -To (packaging\ffmpeg\staged by
  default), with ffmpeg-source.zip beside it, which is where
  packaging\package.ps1 takes FFmpeg from. A -To folder that already has
  files is left alone.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$From,
    [string]$To = (Join-Path (Split-Path -Parent $PSScriptRoot) 'ffmpeg' 'staged')
)

$ErrorActionPreference = 'Stop'

foreach ($name in 'ffmpeg-lgpl-win64.zip', 'ffmpeg-source.zip', 'SHA256SUMS', 'builds-commit.txt') {
    if (-not (Test-Path -LiteralPath (Join-Path $From $name) -PathType Leaf)) {
        throw "The stored FFmpeg in $From has no $name."
    }
}

# SHA256SUMS is sha256sum's own format: the hash, two spaces, the name.
$recorded = @{}
foreach ($line in Get-Content -LiteralPath (Join-Path $From 'SHA256SUMS')) {
    if ($line -match '^([0-9a-fA-F]{64}) [ *](\S+)\s*$') {
        $recorded[$Matches[2]] = $Matches[1].ToLowerInvariant()
    }
}
foreach ($name in 'ffmpeg-lgpl-win64.zip', 'ffmpeg-source.zip') {
    if (-not $recorded.ContainsKey($name)) {
        throw "SHA256SUMS has no SHA-256 for $name."
    }
    $actual = (Get-FileHash -LiteralPath (Join-Path $From $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $recorded[$name]) {
        throw "$name doesn't match the SHA-256 recorded for it in SHA256SUMS: it is $actual, the record says $($recorded[$name])."
    }
}

$pin = (Get-Content -LiteralPath (Join-Path $From 'builds-commit.txt') -Raw).Trim()
if ($pin -notmatch '^[0-9a-f]{40}$') {
    throw "builds-commit.txt doesn't hold a full commit hash: '$pin'."
}

# BUILD.txt, read from inside the zip, must name the recorded commit.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath (Join-Path $From 'ffmpeg-lgpl-win64.zip')).Path)
try {
    $entry = $zip.Entries | Where-Object { $_.FullName -eq 'BUILD.txt' } | Select-Object -First 1
    if (-not $entry) { throw "ffmpeg-lgpl-win64.zip has no BUILD.txt." }
    $reader = [IO.StreamReader]::new($entry.Open())
    try { $build = $reader.ReadToEnd() } finally { $reader.Dispose() }
} finally {
    $zip.Dispose()
}
if (-not $build.Contains($pin)) {
    throw "BUILD.txt in ffmpeg-lgpl-win64.zip doesn't name the FFmpeg-Builds commit $pin that builds-commit.txt records."
}

if ((Test-Path -LiteralPath $To) -and @(Get-ChildItem -LiteralPath $To -Force).Count -gt 0) {
    throw "$To already has files. Move them aside, then run this again."
}
New-Item -ItemType Directory -Force $To | Out-Null
Expand-Archive -LiteralPath (Join-Path $From 'ffmpeg-lgpl-win64.zip') -DestinationPath $To
Copy-Item -LiteralPath (Join-Path $From 'ffmpeg-source.zip') -Destination $To
Write-Host "Unpacked the FFmpeg built with FFmpeg-Builds $pin into $To"
