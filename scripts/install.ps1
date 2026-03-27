# Relava CLI installer for Windows
# Usage: irm https://raw.githubusercontent.com/relava/relava/main/scripts/install.ps1 | iex
#
# Environment variables:
#   RELAVA_INSTALL_DIR  Override install directory (default: $env:LOCALAPPDATA\relava\bin)
#   RELAVA_VERSION      Install a specific version (default: latest)

$ErrorActionPreference = "Stop"

$Repo = "relava/relava"
$BinaryName = "relava"
$DefaultInstallDir = Join-Path $env:LOCALAPPDATA "relava\bin"

function Write-Info {
    param([string]$Message)
    Write-Host "==> " -ForegroundColor Blue -NoNewline
    Write-Host $Message
}

function Write-Success {
    param([string]$Message)
    Write-Host "==> " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Write-ErrorAndExit {
    param([string]$Message)
    Write-Host "error: " -ForegroundColor Red -NoNewline
    Write-Host $Message
    exit 1
}

function Write-Warn {
    param([string]$Message)
    Write-Host "warning: " -ForegroundColor Yellow -NoNewline
    Write-Host $Message
}

# --- Version resolution ---

function Get-LatestVersion {
    $url = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $response = Invoke-RestMethod -Uri $url
        return $response.tag_name
    }
    catch {
        Write-ErrorAndExit "Failed to fetch latest release from GitHub. Check your network connection. $_"
    }
}

# --- Main ---

function Install-Relava {
    Write-Info "Installing Relava CLI..."

    $Target = "x86_64-pc-windows-msvc"
    $Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLower()
    if ($Arch -ne "x64") {
        Write-ErrorAndExit "Unsupported architecture: $Arch. Relava Windows builds support x86_64 only."
    }

    Write-Info "Detected platform: windows x86_64 ($Target)"

    # Resolve version
    $Version = $env:RELAVA_VERSION
    if (-not $Version) {
        Write-Info "Fetching latest release..."
        $Version = Get-LatestVersion
        if (-not $Version) {
            Write-ErrorAndExit "Could not determine latest version. Set RELAVA_VERSION to install a specific version."
        }
    }

    Write-Info "Installing version: $Version"

    # Build download URL
    $ArchiveName = "$BinaryName-$Version-$Target.zip"
    $DownloadUrl = "https://github.com/$Repo/releases/download/$Version/$ArchiveName"

    # Download to temp directory
    $TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

    try {
        $ArchivePath = Join-Path $TmpDir $ArchiveName

        Write-Info "Downloading $ArchiveName..."
        try {
            Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
        }
        catch {
            Write-ErrorAndExit "Download failed: $DownloadUrl. $_"
        }

        # Extract
        Write-Info "Extracting..."
        $ExtractDir = Join-Path $TmpDir "extracted"
        Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

        # Find binary (handles flat or nested archive structures)
        $BinaryPath = Join-Path $ExtractDir "$BinaryName.exe"
        if (-not (Test-Path $BinaryPath)) {
            $BinaryPath = Get-ChildItem -Path $ExtractDir -Recurse -Filter "$BinaryName.exe" | Select-Object -First 1 -ExpandProperty FullName
            if (-not $BinaryPath) {
                Write-ErrorAndExit "Binary '$BinaryName.exe' not found in archive."
            }
        }

        # Determine install directory
        $InstallDir = if ($env:RELAVA_INSTALL_DIR) { $env:RELAVA_INSTALL_DIR } else { $DefaultInstallDir }
        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        }

        # Install binary
        $DestPath = Join-Path $InstallDir "$BinaryName.exe"
        Copy-Item -Path $BinaryPath -Destination $DestPath -Force

        # Verify installation
        try {
            $InstalledVersion = & $DestPath --version 2>&1
            Write-Success "Relava CLI installed successfully! ($InstalledVersion)"
        }
        catch {
            Write-Success "Relava CLI installed to $DestPath"
        }

        # Check if install dir is in PATH
        $UserPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
        $PathEntries = $UserPath -split ";" | ForEach-Object { $_.TrimEnd("\") }
        $NormalizedInstallDir = $InstallDir.TrimEnd("\")
        if ($PathEntries -notcontains $NormalizedInstallDir) {
            Write-Host ""
            Write-Warn "$InstallDir is not in your PATH."
            Write-Host ""
            Write-Host "To add it permanently, run:"
            Write-Host ""
            Write-Host "  [System.Environment]::SetEnvironmentVariable('Path', `"$InstallDir;`$env:Path`", 'User')" -ForegroundColor Cyan
            Write-Host ""
            Write-Host "Or add it for this session only:"
            Write-Host ""
            Write-Host "  `$env:Path = `"$InstallDir;`$env:Path`"" -ForegroundColor Cyan
            Write-Host ""
            Write-Host "Then restart your terminal."
        }
    }
    finally {
        # Cleanup temp directory
        if (Test-Path $TmpDir) {
            Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Install-Relava
