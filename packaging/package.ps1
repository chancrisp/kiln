<#
.SYNOPSIS
  Build Kiln's release binaries, then the MSI and the portable ZIP.

.DESCRIPTION
  Writes dist\Kiln-<version>-x64.msi and dist\Kiln-<version>-x64-portable.zip.

  Code signing is optional and driven by environment variables:
    KILN_SIGN_CERT           Path to a .pfx certificate. Unset: no signing.
    KILN_SIGN_PASSWORD       The certificate's password, if it has one.
    KILN_SIGN_TIMESTAMP_URL  RFC 3161 timestamp server.
                             Default: http://timestamp.digicert.com
  When KILN_SIGN_CERT is set, both executables and the MSI are signed with
  signtool from the Windows SDK, and a signing failure stops the build.

  Needs: Rust (stable, MSVC), and WiX 5 for the MSI
  (`dotnet tool install --global wix --version 5.*`). With -SkipMsi only
  the ZIP is made.
#>
[CmdletBinding()]
param(
    [switch]$SkipMsi
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# The version comes from the workspace, so tags and packages can't drift.
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Packaging Kiln $version"

cargo build --release -p kiln-app -p kiln-cli
if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item target\release\kiln-app.exe, target\release\kiln.exe $stage
Copy-Item LICENSE, THIRD_PARTY_LICENSES, README.md $stage
Copy-Item crates\kiln-app\assets\fonts\OFL.txt $stage
Copy-Item crates\kiln-app\assets\kiln.ico $stage

function Find-SignTool {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $found = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $found) { throw "KILN_SIGN_CERT is set but signtool.exe wasn't found. Install the Windows SDK." }
    return $found.FullName
}

function Sign-Files([string[]]$files) {
    if (-not $env:KILN_SIGN_CERT) { return }
    $signtool = Find-SignTool
    $ts = if ($env:KILN_SIGN_TIMESTAMP_URL) { $env:KILN_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $ts, '/f', $env:KILN_SIGN_CERT)
    if ($env:KILN_SIGN_PASSWORD) { $signArgs += @('/p', $env:KILN_SIGN_PASSWORD) }
    & $signtool @signArgs @files
    if ($LASTEXITCODE -ne 0) { throw "Signing failed." }
}

Sign-Files @((Join-Path $stage 'kiln-app.exe'), (Join-Path $stage 'kiln.exe'))

$zip = Join-Path $dist "Kiln-$version-x64-portable.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Get-ChildItem $stage -Exclude kiln.ico | Compress-Archive -DestinationPath $zip
Write-Host "Wrote $zip"

if (-not $SkipMsi) {
    $msi = Join-Path $dist "Kiln-$version-x64.msi"
    wix build packaging\kiln.wxs -arch x64 -d "Version=$version" -d "StageDir=$stage" -o $msi
    if ($LASTEXITCODE -ne 0) { throw "wix build failed." }
    Sign-Files @($msi)
    Write-Host "Wrote $msi"
}
