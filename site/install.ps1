$ErrorActionPreference = 'Stop'

$AppName = 'meerai'
$Repo = 'packvisionxyz/meerai'
$BaseUrl = if ($env:MEERAI_DOWNLOAD_BASE_URL) { $env:MEERAI_DOWNLOAD_BASE_URL.TrimEnd('/') } else { "https://github.com/$Repo/releases" }
$Version = if ($env:MEERAI_VERSION) { $env:MEERAI_VERSION } else { 'latest' }
$InstallDir = if ($env:MEERAI_INSTALL_DIR) { $env:MEERAI_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.local\bin' }

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Test-Requirement {
    param(
        [string]$Command,
        [string]$Reason,
        [string]$Setup
    )
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) {
        Write-Warning "Missing requirement '$Command' ($Reason). $Setup"
    }
}

function Test-Requirements {
    # BEGIN GENERATED REQUIREMENTS
    # END GENERATED REQUIREMENTS
}

function Get-TargetTriple {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Architecture
    )

    switch -Exact ($Architecture) {
        'X64' { return 'x86_64-pc-windows-msvc' }
        'Arm64' { return 'x86_64-pc-windows-msvc' }
        default {
            throw "Unsupported Windows architecture '$Architecture'. meerai publishes the x64 Windows build only (meerai-x86_64-pc-windows-msvc.zip). Download it manually from $BaseUrl/latest and run it under emulation, or open an issue at https://github.com/$Repo/issues."
        }
    }
}

$HostArchitecture = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
    'Arm64'
} else {
    [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
}
$Target = Get-TargetTriple -Architecture $HostArchitecture
$Archive = "$AppName-$Target.zip"

if ($HostArchitecture -eq 'Arm64') {
    Write-Warning "$AppName publishes only the x64 Windows build ($Target). The installer downloads that archive; it runs under emulation on Windows on Arm, so expect slower startup and file I/O."
}

if ($Version -eq 'latest') {
    $ArchiveUrl = "$BaseUrl/latest/download/$Archive"
    $ChecksumUrl = "$BaseUrl/latest/download/$Archive.sha256"
} else {
    $Tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
    $ArchiveUrl = "$BaseUrl/download/$Tag/$Archive"
    $ChecksumUrl = "$BaseUrl/download/$Tag/$Archive.sha256"
}

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("$AppName-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    $ArchivePath = Join-Path $TempDir $Archive
    $ChecksumPath = Join-Path $TempDir "$Archive.sha256"

    Write-Host "Downloading $AppName ($Target)..."
    Invoke-WebRequest -Uri $ArchiveUrl -OutFile $ArchivePath -UseBasicParsing
    Invoke-WebRequest -Uri $ChecksumUrl -OutFile $ChecksumPath -UseBasicParsing

    $ExpectedHash = ((Get-Content -Path $ChecksumPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    if (-not $ExpectedHash) {
        throw "Empty checksum file downloaded from $ChecksumUrl"
    }

    $ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "Checksum mismatch for $Archive (expected $ExpectedHash, got $ActualHash)"
    }

    $ExtractDir = Join-Path $TempDir 'extract'
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

    $SourceExe = Join-Path $ExtractDir "$AppName.exe"
    if (-not (Test-Path $SourceExe)) {
        throw "Archive did not contain $AppName.exe"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $DestExe = Join-Path $InstallDir "$AppName.exe"
    Move-Item -Path $SourceExe -Destination $DestExe -Force

    Write-Host "Installed $AppName to $DestExe"

    if (-not $env:MEERAI_NO_MODIFY_PATH) {
        $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $PathEntries = if ($UserPath) { $UserPath -split ';' | Where-Object { $_ } } else { @() }
        if ($PathEntries -notcontains $InstallDir) {
            $NewUserPath = (@($InstallDir) + $PathEntries) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $NewUserPath, 'User')
            Write-Host "Added $InstallDir to your user PATH."
        }
        $SessionEntries = $env:Path -split ';'
        if ($SessionEntries -notcontains $InstallDir) {
            $env:Path = "$InstallDir;$env:Path"
        }
    }

    Test-Requirements
    Write-Host "Run '$AppName' to start."
}
finally {
    Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
