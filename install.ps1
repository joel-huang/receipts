# Receipts installer for Windows.
#
#   irm https://github.com/joel-huang/receipts/releases/latest/download/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Repo = if ($env:RECEIPTS_REPO) { $env:RECEIPTS_REPO } else { 'joel-huang/receipts' }
$Version = if ($env:RECEIPTS_VERSION) { $env:RECEIPTS_VERSION } else { 'latest' }
$InstallDir = if ($env:RECEIPTS_INSTALL_DIR) { $env:RECEIPTS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'receipts\bin' }

$Target = 'x86_64-pc-windows-msvc'
$Archive = "receipts-$Target.zip"
$Base = if ($Version -eq 'latest') { "https://github.com/$Repo/releases/latest/download" } else { "https://github.com/$Repo/releases/download/$Version" }

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("receipts-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    Write-Host "receipts: downloading $Archive ($Version)"
    Invoke-WebRequest "$Base/$Archive" -OutFile "$Tmp\$Archive" -UseBasicParsing
    Invoke-WebRequest "$Base/$Archive.sha256" -OutFile "$Tmp\$Archive.sha256" -UseBasicParsing

    $Expected = (Get-Content "$Tmp\$Archive.sha256").Split(' ')[0].Trim().ToLower()
    $Actual = (Get-FileHash "$Tmp\$Archive" -Algorithm SHA256).Hash.ToLower()
    if ($Expected -ne $Actual) { throw "checksum mismatch (expected $Expected, got $Actual)" }

    Expand-Archive "$Tmp\$Archive" -DestinationPath $Tmp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item "$Tmp\receipts.exe" "$InstallDir\receipts.exe" -Force

    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not ($UserPath -split ';' | Where-Object { $_ -eq $InstallDir })) {
        [Environment]::SetEnvironmentVariable('Path', "$UserPath;$InstallDir", 'User')
        Write-Host "receipts: added $InstallDir to your PATH (restart your terminal)"
    }
    Write-Host "receipts: installed to $InstallDir\receipts.exe. Run 'receipts' to open your chat history."
} finally {
    Remove-Item -Recurse -Force $Tmp
}
