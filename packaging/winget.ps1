#Requires -Version 7
<#
.SYNOPSIS
  Write the winget manifests for a published Kiln release.

.DESCRIPTION
  Reads the MSI's SHA-256 from SHA256SUMS.txt in -Dist and writes the three
  manifests winget-pkgs takes, under
  <Dist>\winget\manifests\c\ChandlerCrisp\Kiln\<version>\:
    ChandlerCrisp.Kiln.yaml               the version file
    ChandlerCrisp.Kiln.installer.yaml     the MSI, its address and SHA-256
    ChandlerCrisp.Kiln.locale.en-US.yaml  the name, publisher and links

  Run it on the files of the release as published (the signed MSI), after
  the release is public: winget-pkgs checks that the address answers. It
  writes files only. Submitting them (for example with `wingetcreate
  submit`) is the maintainer's step.
#>
[CmdletBinding()]
param(
    [string]$Dist = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist'),
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $Version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
}
$id = 'ChandlerCrisp.Kiln'
$msi = "Kiln-$Version-x64.msi"
$sums = Join-Path $Dist 'SHA256SUMS.txt'
if (-not (Test-Path -LiteralPath $sums)) { throw "No SHA256SUMS.txt in $Dist." }
$line = Get-Content -LiteralPath $sums | Where-Object { $_ -match "^([0-9a-f]{64})  $([regex]::Escape($msi))$" } | Select-Object -First 1
if (-not $line) { throw "SHA256SUMS.txt names no $msi." }
$sha = ($line -split '  ')[0].ToUpperInvariant()

# The MSI's UpgradeCode, so winget knows an installed Kiln as this package.
$upgrade = ([xml](Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'kiln.wxs'))).Wix.Package.UpgradeCode.ToUpperInvariant()
if (-not $upgrade) { throw 'No UpgradeCode in kiln.wxs.' }

$out = Join-Path $Dist "winget\manifests\c\ChandlerCrisp\Kiln\$Version"
New-Item -ItemType Directory -Force $out | Out-Null
$schema = '1.9.0'
$repo = 'https://github.com/chancrisp/kiln'

$files = [ordered]@{
    "$id.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@
    "$id.installer.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
MinimumOSVersion: 10.0.19045.0
InstallerType: wix
Scope: machine
UpgradeBehavior: install
Installers:
- Architecture: x64
  InstallerUrl: $repo/releases/download/v$Version/$msi
  InstallerSha256: $sha
  AppsAndFeaturesEntries:
  - UpgradeCode: '{$upgrade}'
ManifestType: installer
ManifestVersion: $schema
"@
    "$id.locale.en-US.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Chandler Crisp
PublisherUrl: https://github.com/chancrisp
PublisherSupportUrl: $repo/blob/main/SUPPORT.md
PrivacyUrl: $repo/blob/main/PRIVACY.md
PackageName: Kiln
PackageUrl: $repo
License: GPL-3.0-or-later
LicenseUrl: $repo/blob/main/LICENSE
ShortDescription: Converts photos, sound, video, documents and e-books from one format to another on Windows.
Tags:
- converter
- file-converter
- audio
- video
- image
- pdf
ReleaseNotesUrl: $repo/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: $schema
"@
}
foreach ($name in $files.Keys) {
    $path = Join-Path $out $name
    [IO.File]::WriteAllText($path, ($files[$name] -replace "`r`n", "`n") + "`n")
    Write-Host "Wrote $path"
}
