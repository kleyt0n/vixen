# Vixen installer for Windows (CLI).
#
#   irm https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.ps1 | iex
#
# Installs vixen.exe to %LOCALAPPDATA%\vixen\bin and adds it to your user PATH.
# Remote targets need the OpenSSH client (built into Windows 10+).
#
# Environment:
#   VIXEN_VERSION      release tag to install (default: latest), e.g. v0.1.0
#   VIXEN_INSTALL_DIR  where vixen.exe goes
#   VIXEN_REPO         GitHub repo to download from (default: kleyt0n/vixen)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo    = if ($env:VIXEN_REPO) { $env:VIXEN_REPO } else { 'kleyt0n/vixen' }
$version = if ($env:VIXEN_VERSION) { $env:VIXEN_VERSION } else { 'latest' }
$binDir  = if ($env:VIXEN_INSTALL_DIR) { $env:VIXEN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'vixen\bin' }

$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
  'X64'   { 'x86_64' }
  'Arm64' { 'x86_64' }   # x64 build runs under emulation on Windows on ARM
  default { throw "vixen: unsupported CPU $_" }
}
$target = "$arch-pc-windows-msvc"
$asset  = "vixen-$target.zip"
$base   = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" }
          else { "https://github.com/$repo/releases/download/$version" }

Write-Host "Installing vixen ($version, $target)" -ForegroundColor White
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("vixen-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp $asset
  Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip

  # Verify against SHA256SUMS when the release has one.
  try {
    $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS").Content
    if ($sums -is [byte[]]) { $sums = [System.Text.Encoding]::UTF8.GetString($sums) }
    $line = ($sums -split "`n") | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))\s*$" } | Select-Object -First 1
    if ($line) {
      $want = ($line -split '\s+')[0].ToLower()
      $got  = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
      if ($got -ne $want) { throw "vixen: checksum mismatch for $asset" }
    }
  } catch [System.Net.WebException] { }

  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force -Path $binDir | Out-Null
  Copy-Item (Join-Path $tmp 'vixen.exe') (Join-Path $binDir 'vixen.exe') -Force
  Write-Host "  CLI  -> $binDir\vixen.exe"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not (($userPath -split ';') -contains $binDir)) {
    [Environment]::SetEnvironmentVariable('Path', ($(if ($userPath) { "$userPath;" } else { '' }) + $binDir), 'User')
    $env:Path = "$env:Path;$binDir"
    Write-Host "  Added $binDir to your user PATH (open a new terminal to use it)."
  }

  & (Join-Path $binDir 'vixen.exe') init | Out-Null
  Write-Host "Done. Edit $env:USERPROFILE\.config\vixen\config.toml, then run: vixen" -ForegroundColor White
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
