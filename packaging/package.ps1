#Requires -Version 7
<#
.SYNOPSIS
  Build Kiln's release binaries, then the MSI, the portable ZIP, the source
  ZIPs, the symbols ZIP and SHA256SUMS.txt.

.DESCRIPTION
  Three steps, each of which can be run on its own:

    -Stage  Builds the programs and lame_enc.dll (cargo --release --locked)
            and puts everything a package holds in dist\stage.
    -Pack   Makes the packages from dist\stage:
              dist\Kiln-<version>-x64-portable.zip
              dist\Kiln-<version>-x64.msi           (not with -SkipMsi)
              dist\Kiln-<version>-source.zip        (Kiln's source at HEAD)
              dist\Kiln-<version>-ffmpeg-source.zip (not with -WithoutFfmpeg)
              dist\Kiln-<version>-symbols.zip       (the programs' PDBs)
    -Sums   Writes dist\SHA256SUMS.txt for every package in dist.

  With none of the three, all three run in order. The release workflow runs
  them one at a time, because the files in dist\stage are signed between
  -Stage and -Pack, and the MSI between -Pack and -Sums. This script signs
  nothing: signing happens only in .github\workflows\release.yml, through
  SignPath or Azure Artifact Signing (see packaging\SIGNING.md).

  lame_enc.dll is LAME, the MP3 encoder, built from the kiln-lame crate. It
  goes beside the executables, with LAME-COPYING.txt (its LGPL) and
  THIRD_PARTY_NOTICES.md.

  FFmpeg is an LGPL build of its own, made once by
  .github/workflows/ffmpeg-lgpl.yml and kept on the release ffmpeg-lgpl of
  this repository. packaging\ffmpeg\unpack-stored.ps1 checks that release's
  files against their SHA-256 and unpacks them into packaging\ffmpeg\staged
  (bin\, LICENSE.txt, BUILD.txt and licenses\, ffmpeg-source.zip; not in
  git). It goes in ffmpeg\ beside the executables: ffmpeg\bin (ffmpeg.exe,
  ffprobe.exe and FFmpeg's own DLLs, and nothing else from the staged bin),
  ffmpeg\licenses (the notices of the libraries built into it),
  ffmpeg\LICENSE.txt, ffmpeg\BUILD.txt, and ffmpeg\SOURCE.txt, filled in from
  packaging\ffmpeg\SOURCE.txt, which says where its source is: the
  Kiln-<version>-ffmpeg-source.zip on the same release page. A staged build
  with GPL or nonfree parts (by its configuration, its licence, its encoders
  or a postproc DLL), or with libraries from another build, stops the
  packaging. With -WithoutFfmpeg the packages leave FFmpeg out.

  Needs PowerShell 7 (pwsh): Windows PowerShell 5.1 stops at FFmpeg's first
  line on stderr.

  Needs: Rust (the version rust-toolchain.toml names, MSVC), git, and WiX 5
  for the MSI with its UI and Util extensions:
    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
  With -SkipMsi no MSI is made.
#>
[CmdletBinding()]
param(
    [switch]$Stage,
    [switch]$Pack,
    [switch]$Sums,
    [switch]$SkipMsi,
    [switch]$WithoutFfmpeg
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not ($Stage -or $Pack -or $Sums)) { $Stage = $Pack = $Sums = $true }

# The version comes from the workspace, so tags and packages can't drift.
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
# The MSI and the programs' version resources carry numbers only: 0.1.0 for
# 0.1.0-rc.1.
$numeric = $version -replace '-.*$', ''
Write-Host "Packaging Kiln $version"

$dist = Join-Path $root 'dist'
$stageDir = Join-Path $dist 'stage'
$ffmpegStaged = Join-Path $root 'packaging\ffmpeg\staged'

if ($Stage) {
    # FFmpeg is checked before anything is built: an LGPL build only, with
    # its notices and source.
    if (-not $WithoutFfmpeg) {
        $sourceNote = Get-Content (Join-Path $root 'packaging\ffmpeg\SOURCE.txt') -Raw
        $sourceNote = $sourceNote.Replace('{{KILN_VERSION}}', $version)

        $ffmpegExe = Join-Path $ffmpegStaged 'bin\ffmpeg.exe'
        if (-not (Test-Path $ffmpegExe)) {
            throw "No FFmpeg staged at $ffmpegExe. Download the files of the ffmpeg-lgpl release and run packaging\ffmpeg\unpack-stored.ps1 -From <that folder>, or pass -WithoutFfmpeg."
        }
        foreach ($name in 'bin\ffprobe.exe', 'LICENSE.txt', 'BUILD.txt', 'ffmpeg-source.zip') {
            if (-not (Test-Path (Join-Path $ffmpegStaged $name))) {
                throw "The staged FFmpeg has no $name."
            }
        }
        # The notices of the libraries built into FFmpeg, one file each.
        $ffmpegLicenses = @(Get-ChildItem (Join-Path $ffmpegStaged 'licenses') -Filter *.txt -File -ErrorAction SilentlyContinue)
        if ($ffmpegLicenses.Count -eq 0) {
            throw "The staged FFmpeg has no library notices in licenses."
        }
        $buildconf = (& $ffmpegExe -hide_banner -buildconf 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw "The staged ffmpeg.exe didn't run." }
        if ($buildconf -match '--enable-(gpl|nonfree)\b') {
            throw "The staged FFmpeg has GPL or nonfree parts ($($Matches[0])). Kiln ships only an LGPL build."
        }
        # -buildconf is ffmpeg.exe's own. The libraries beside it must be from
        # the same build, and the build must say it is under the LGPL.
        $versionText = (& $ffmpegExe -version 2>&1) -join "`n"
        if ($versionText -match 'configuration mismatch') {
            throw "The staged FFmpeg's libraries come from another build (library configuration mismatch)."
        }
        $licenceText = (& $ffmpegExe -hide_banner -L 2>&1) -join "`n"
        if ($licenceText -cmatch 'GNU General Public License') {
            throw "The staged FFmpeg says it is under the GNU General Public License. Kiln ships only an LGPL build."
        }
        $encoderText = (& $ffmpegExe -hide_banner -encoders 2>&1) -join "`n"
        foreach ($encoder in 'libx264', 'libx265', 'libxvid', 'libfdk_aac') {
            if ($encoderText -match "(?m)^\s*[A-Z.]{6}\s+$([regex]::Escape($encoder))\s") {
                throw "The staged FFmpeg has the $encoder encoder. Kiln ships only an LGPL build."
            }
        }
        # Only FFmpeg's own programs and libraries are shipped from bin.
        # libpostproc is GPL only, so an LGPL build never has it.
        $shippedBin = '^(ffmpeg|ffprobe)\.exe$|^(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale)-\d+\.dll$'
        $stagedBin = @(Get-ChildItem (Join-Path $ffmpegStaged 'bin') -File)
        $postproc = @($stagedBin | Where-Object { $_.Name -like 'postproc*.dll' })
        if ($postproc.Count -gt 0) {
            throw "The staged FFmpeg has $($postproc[0].Name), which is GPL only. Kiln ships only an LGPL build."
        }
        $ffmpegBin = @($stagedBin | Where-Object { $_.Name -match $shippedBin })
        foreach ($file in $stagedBin | Where-Object { $_.Name -notmatch $shippedBin }) {
            Write-Warning "Leaving $($file.Name) out of the package: it isn't one of FFmpeg's own programs or libraries."
        }
    }

    cargo build --release --locked -p kiln-app -p kiln-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }

    # The DLL is a separate build on purpose: the programs don't depend on
    # kiln-lame, so the LGPL code never gets linked into them.
    cargo build --release --locked -p kiln-lame
    if ($LASTEXITCODE -ne 0) { throw "cargo build of lame_enc.dll failed." }

    # Where cargo put them: target\release, or CARGO_TARGET_DIR's.
    $target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
    if ($LASTEXITCODE -ne 0 -or -not $target) { throw "cargo metadata failed." }
    $release = Join-Path $target 'release'

    Remove-Item -Recurse -Force $stageDir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stageDir | Out-Null
    $symbols = Join-Path $dist 'symbols'
    Remove-Item -Recurse -Force $symbols -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $symbols | Out-Null

    Copy-Item (Join-Path $release 'kiln-app.exe'), (Join-Path $release 'kiln.exe'), (Join-Path $release 'lame_enc.dll') $stageDir
    Copy-Item (Join-Path $release 'kiln_app.pdb'), (Join-Path $release 'kiln.pdb'), (Join-Path $release 'lame_enc.pdb') $symbols
    Copy-Item LICENSE, THIRD_PARTY_LICENSES, THIRD_PARTY_NOTICES.md, PRIVACY.md, 'packaging\Read me.txt' $stageDir
    Copy-Item crates\kiln-lame\COPYING (Join-Path $stageDir 'LAME-COPYING.txt')
    Copy-Item crates\kiln-app\assets\kiln.ico $stageDir
    # The MSI's licence page shows the GPL, as RTF: LICENSE with RTF's three
    # special characters escaped and each line a paragraph.
    $licence = (Get-Content LICENSE -Raw) -replace "`r`n", "`n"
    $licence = $licence.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
    $rtf = '{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fmodern Consolas;}}\f0\fs16 ' +
        (($licence.TrimEnd("`n") -split "`n") -join "\par`r`n") + '\par}'
    [IO.File]::WriteAllText((Join-Path $stageDir 'License.rtf'), $rtf, [Text.Encoding]::ASCII)

    if (-not $WithoutFfmpeg) {
        $ffmpegStage = Join-Path $stageDir 'ffmpeg'
        New-Item -ItemType Directory -Force (Join-Path $ffmpegStage 'bin') | Out-Null
        Copy-Item $ffmpegBin.FullName (Join-Path $ffmpegStage 'bin')
        New-Item -ItemType Directory -Force (Join-Path $ffmpegStage 'licenses') | Out-Null
        Copy-Item $ffmpegLicenses.FullName (Join-Path $ffmpegStage 'licenses')
        Copy-Item (Join-Path $ffmpegStaged 'LICENSE.txt'), (Join-Path $ffmpegStaged 'BUILD.txt') $ffmpegStage
        [IO.File]::WriteAllText((Join-Path $ffmpegStage 'SOURCE.txt'), $sourceNote)
    }
    Write-Host "Staged $stageDir"
}

if ($Pack) {
    if (-not (Test-Path (Join-Path $stageDir 'kiln-app.exe'))) {
        throw "Nothing is staged in $stageDir. Run with -Stage first."
    }
    $hasFfmpeg = Test-Path (Join-Path $stageDir 'ffmpeg\bin\ffmpeg.exe')
    if ($hasFfmpeg -and $WithoutFfmpeg) {
        throw "$stageDir has FFmpeg, but -WithoutFfmpeg was given. Stage again with -WithoutFfmpeg."
    }

    $zip = Join-Path $dist "Kiln-$version-x64-portable.zip"
    Remove-Item -Force $zip -ErrorAction SilentlyContinue
    # The icon and License.rtf are the MSI's; the ZIP has LICENSE itself.
    Get-ChildItem $stageDir -Exclude kiln.ico, License.rtf | Compress-Archive -DestinationPath $zip
    Write-Host "Wrote $zip"

    # The source of everything in the packages, as the GPL and the LGPL ask
    # of a binary release: the files git has at HEAD (with the changed
    # Symphonia and libwavpack in crates\kiln-media\vendor), and the source
    # of every crate Cargo.lock names, LAME's (mp3lame-sys) among them, in
    # third-party\crates, so the release builds offline as it was built.
    $source = Join-Path $dist "Kiln-$version-source.zip"
    $sourceRoot = Join-Path $dist 'source'
    $sourceDir = Join-Path $sourceRoot "kiln-$version"
    Remove-Item -Force $source -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $sourceRoot -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $sourceDir | Out-Null
    $tree = Join-Path $sourceRoot 'tree.tar'
    git archive --format=tar -o $tree HEAD
    if ($LASTEXITCODE -ne 0) { throw "git archive failed." }
    tar -xf $tree -C $sourceDir
    if ($LASTEXITCODE -ne 0) { throw "Unpacking the source tree failed." }
    Remove-Item -Force $tree
    $crates = Join-Path $sourceDir 'third-party\crates'
    # cargo vendor prints the lines .cargo\config.toml needs; its progress goes
    # to stderr.
    $vendorNote = cargo vendor --locked --versioned-dirs $crates
    if ($LASTEXITCODE -ne 0) { throw "cargo vendor failed." }
    $offline = @(
        "third-party\crates holds the source of every crate Kiln $version is built"
        "with, exactly as Cargo.lock names them, LAME 3.100's among them"
        "(mp3lame-sys-0.1.11\lame-3.100)."
        ""
        "To build offline from them, add this to .cargo\config.toml:"
        ""
    ) + ($vendorNote | ForEach-Object {
        # cargo vendor names the folder by this PC's full path; the note
        # names it from the source's top folder.
        if ($_ -match '^directory = ') { 'directory = "third-party/crates"' } else { $_ }
    })
    [IO.File]::WriteAllText((Join-Path $sourceDir 'third-party\README.txt'), (($offline -join "`r`n") + "`r`n"))
    # Some crates carry file times before 1980, which a ZIP stores as 1980.
    Compress-Archive -Path $sourceDir -DestinationPath $source -WarningAction SilentlyContinue
    Remove-Item -Recurse -Force $sourceRoot
    Write-Host "Wrote $source"

    # FFmpeg's source, every source its build fetched with the scripts that
    # built it, goes beside the packages under the name ffmpeg\SOURCE.txt
    # gives.
    $wixDefines = @()
    if ($hasFfmpeg) {
        $ffmpegSource = Join-Path $dist "Kiln-$version-ffmpeg-source.zip"
        $stagedSource = Get-Item (Join-Path $ffmpegStaged 'ffmpeg-source.zip')
        # GitHub takes release files under 2 GiB.
        if ($stagedSource.Length -ge 2GB) {
            throw "FFmpeg's source zip is $([math]::Round($stagedSource.Length / 1GB, 2)) GiB, over the 2 GiB a release file may be."
        }
        Copy-Item $stagedSource.FullName $ffmpegSource -Force
        Write-Host "Wrote $ffmpegSource"
    } else {
        $wixDefines = @('-d', 'NoFfmpeg=1')
    }

    $symbolsZip = Join-Path $dist "Kiln-$version-symbols.zip"
    Remove-Item -Force $symbolsZip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $dist 'symbols\*.pdb') -DestinationPath $symbolsZip
    Write-Host "Wrote $symbolsZip"

    if (-not $SkipMsi) {
        $msi = Join-Path $dist "Kiln-$version-x64.msi"
        wix build packaging\kiln.wxs packaging\open-with-types.wxs -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext -arch x64 -d "Version=$numeric" -d "StageDir=$stageDir" @wixDefines -o $msi
        if ($LASTEXITCODE -ne 0) { throw "wix build failed." }
        Write-Host "Wrote $msi"
    }
}

if ($Sums) {
    # One line per package, as sha256sum writes them: the hash, two spaces
    # and the file name.
    $packages = @(Get-ChildItem $dist -File | Where-Object { $_.Name -like "Kiln-$version-*" -and $_.Extension -in '.msi', '.zip' } | Sort-Object Name)
    if ($packages.Count -eq 0) { throw "No packages of Kiln $version in $dist." }
    $lines = foreach ($p in $packages) {
        '{0}  {1}' -f (Get-FileHash $p.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $p.Name
    }
    $sumsFile = Join-Path $dist 'SHA256SUMS.txt'
    [IO.File]::WriteAllText($sumsFile, (($lines -join "`n") + "`n"))
    Write-Host "Wrote $sumsFile"
}
