#Requires -Version 7
<#
.SYNOPSIS
  Write THIRD_PARTY_LICENSES from Cargo.lock with cargo-about.

.DESCRIPTION
  Lists every crate built into kiln-app.exe, kiln.exe and lame_enc.dll with
  its licence, then the full text of every licence, with each crate's
  copyright notice where the crate ships one. about.toml says which licences
  are accepted and about.hbs how the file reads. cargo-about uses only what
  Cargo has already downloaded (--frozen), so the same Cargo.lock gives the
  same file.

  With -Check it writes nothing and stops when THIRD_PARTY_LICENSES is out of
  date, as CI runs it.

  Needs cargo-about 0.9 (`cargo install cargo-about --version 0.9.3 --locked
  --features cli`).
#>
[CmdletBinding()]
param([switch]$Check)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo fetch --locked
if ($LASTEXITCODE -ne 0) { throw "cargo fetch failed." }
# cargo-about won't write to PowerShell's pipeline, so it writes to a file of
# its own in the build folder, replaced on each run.
$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
if ($LASTEXITCODE -ne 0 -or -not $target) { throw "cargo metadata failed." }
New-Item -ItemType Directory -Force $target | Out-Null
$made = Join-Path $target 'THIRD_PARTY_LICENSES.generated'
cargo about generate --frozen -c about.toml about.hbs -o $made
if ($LASTEXITCODE -ne 0) { throw "cargo about failed." }
# LF line ends, as git keeps the file, and one newline at the end.
$text = ((Get-Content -LiteralPath $made -Raw) -replace "`r`n", "`n").TrimEnd("`n") + "`n"

$file = Join-Path $root 'THIRD_PARTY_LICENSES'
if ($Check) {
    $now = (Get-Content -LiteralPath $file -Raw) -replace "`r`n", "`n"
    if ($now -ne $text) {
        throw "THIRD_PARTY_LICENSES is out of date with Cargo.lock. Run .\packaging\licences.ps1 and commit the result."
    }
    Write-Host "THIRD_PARTY_LICENSES is up to date."
} else {
    [IO.File]::WriteAllText($file, $text)
    Write-Host "Wrote $file"
}
