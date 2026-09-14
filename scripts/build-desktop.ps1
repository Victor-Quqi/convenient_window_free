param(
  [switch]$RequireTrustedSignature
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot "hash-file.ps1")

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
  throw "Desktop packages can only be built on Windows"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$desktopDir = Join-Path $repoRoot "apps\desktop"
$tauriDir = Join-Path $desktopDir "src-tauri"
$artifactsDir = Join-Path $repoRoot "artifacts"
$portableDir = Join-Path $artifactsDir "ConvenientWindow-portable"
$thirdPartyNotices = Join-Path $repoRoot "target\THIRD-PARTY-NOTICES.txt"
$rootPackage = [System.IO.File]::ReadAllText((Join-Path $repoRoot "package.json"), [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$desktopPackage = [System.IO.File]::ReadAllText((Join-Path $desktopDir "package.json"), [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$tauriConfig = [System.IO.File]::ReadAllText((Join-Path $tauriDir "tauri.conf.json"), [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$version = [string]$rootPackage.version
if (-not $version -or $desktopPackage.version -ne $version -or $tauriConfig.version -ne $version) {
  throw "Desktop version mismatch: root=$($rootPackage.version), frontend=$($desktopPackage.version), tauri=$($tauriConfig.version)"
}
function Get-SourceChanges {
  $unstaged = @(& git -C $repoRoot diff --name-only --)
  if ($LASTEXITCODE -ne 0) { throw "Unable to inspect unstaged source changes" }
  $staged = @(& git -C $repoRoot diff --cached --name-only --)
  if ($LASTEXITCODE -ne 0) { throw "Unable to inspect staged source changes" }
  $untracked = @(& git -C $repoRoot ls-files --others --exclude-standard)
  if ($LASTEXITCODE -ne 0) { throw "Unable to inspect untracked source files" }
  return @($unstaged + $staged + $untracked | Sort-Object -Unique)
}

$sourceCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sourceCommit -notmatch '^[0-9a-f]{40}$') {
  throw "Unable to read the desktop source commit"
}
$sourceStatus = @(Get-SourceChanges)
$helperToolchain = (Get-Content (Join-Path $repoRoot "rust-toolchain") -Raw).Trim()
$desktopToolchain = "1.96.0-x86_64-pc-windows-msvc"

# 在 $ErrorActionPreference = "Stop" 下，PowerShell 5.1 会把原生命令（cargo、rustup、npm、node）
# 写在 stderr 的任何输出包装成 NativeCommandError 并当作终止错误抛出 —— 即使该命令的退出码是 0。
# cargo 的编译进度、npm 的警告都会走 stderr，所以直接 `& cmd` 会在第一次真正干活时就中断脚本。
#
# 统一通过这个包装调用，两个细节都不能省：
#   1. 临时把 ErrorActionPreference 放宽为 Continue，只用退出码判断成败；
#   2. 参数用**数组**传入，不能用 ValueFromRemainingArguments —— 否则 PowerShell 的参数绑定器
#      会把 `-e`、`-p` 这类短选项当成自身参数的简写（例如 node -e 会撞上 -ErrorAction 而报
#      "parameter name 'e' is ambiguous"）。
# 另外 `return $LASTEXITCODE` 前必须把命令输出交给 Out-Host：PowerShell 函数会把所有未捕获
# 输出并入返回值，否则调用方拿到的不是数字而是被 stdout 污染的对象。
function Invoke-Native {
  param([Parameter(Mandatory = $true)][string[]]$CommandLine)
  $previous = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $executable = $CommandLine[0]
    $arguments = @()
    if ($CommandLine.Count -gt 1) { $arguments = $CommandLine[1..($CommandLine.Count - 1)] }
    & $executable @arguments 2>&1 | Out-Host
    return $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previous
  }
}

. (Join-Path $PSScriptRoot "windows-toolchain.ps1")
Initialize-MsvcEnvironment
$buildEnvironmentPath = $env:PATH

$exitCode = Invoke-Native @("npm", "--prefix", $desktopDir, "ci")
if ($exitCode -ne 0) { throw "desktop npm ci failed with exit code $exitCode" }
$exitCode = Invoke-Native @("node", (Join-Path $PSScriptRoot "generate-third-party-notices.mjs"), $thirdPartyNotices)
if ($exitCode -ne 0) { throw "third-party notice generation failed with exit code $exitCode" }
$exitCode = Invoke-Native @((Join-Path $PSScriptRoot "prepare-desktop-sidecar.ps1"))
if ($exitCode -ne 0) { throw "sidecar preparation failed with exit code $exitCode" }
$env:PATH = $buildEnvironmentPath
Remove-Item Env:RUSTC -ErrorAction SilentlyContinue
Remove-Item Env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER -ErrorAction SilentlyContinue

$exitCode = Invoke-Native @("npm", "--prefix", $desktopDir, "test")
if ($exitCode -ne 0) { throw "desktop tests failed with exit code $exitCode" }
$exitCode = Invoke-Native @("npm", "--prefix", $desktopDir, "run", "check")
if ($exitCode -ne 0) { throw "desktop check failed with exit code $exitCode" }
$exitCode = Invoke-Native @("npm", "--prefix", $desktopDir, "run", "build")
if ($exitCode -ne 0) { throw "desktop frontend build failed with exit code $exitCode" }

Push-Location (Join-Path $repoRoot "helper")
try {
  $exitCode = Invoke-Native @("rustup", "run", $helperToolchain, "cargo", "fmt", "--check")
  if ($exitCode -ne 0) { throw "helper rustfmt failed with exit code $exitCode" }
  $exitCode = Invoke-Native @("rustup", "run", $helperToolchain, "cargo", "test")
  if ($exitCode -ne 0) { throw "helper tests failed with exit code $exitCode" }
} finally {
  Pop-Location
}

Push-Location $tauriDir
try {
  $exitCode = Invoke-Native @("rustup", "run", $desktopToolchain, "cargo", "fmt", "--check")
  if ($exitCode -ne 0) { throw "desktop rustfmt failed with exit code $exitCode" }
  $exitCode = Invoke-Native @("rustup", "run", $desktopToolchain, "cargo", "test")
  if ($exitCode -ne 0) { throw "desktop Rust tests failed with exit code $exitCode" }
} finally {
  Pop-Location
}

$exitCode = Invoke-Native @("npm", "--prefix", $desktopDir, "run", "tauri:build")
if ($exitCode -ne 0) { throw "Tauri NSIS build failed with exit code $exitCode" }

$releaseDir = Join-Path $tauriDir "target\release"
$appExe = Join-Path $releaseDir "convenient-window.exe"
$nsisInstaller = Get-ChildItem (Join-Path $releaseDir "bundle\nsis") -Filter "*.exe" -File |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
if (-not (Test-Path $appExe)) { throw "Tauri executable is missing: $appExe" }
if (-not $nsisInstaller) { throw "NSIS installer was not produced" }

if (Test-Path $artifactsDir) { Remove-Item -Recurse -Force $artifactsDir }
New-Item -ItemType Directory -Force -Path (Join-Path $portableDir "helper") | Out-Null
Copy-Item -Force $appExe (Join-Path $portableDir "ConvenientWindow.exe")
Copy-Item -Force (Join-Path $repoRoot "LICENSE") (Join-Path $portableDir "LICENSE")
Copy-Item -Force $thirdPartyNotices (Join-Path $portableDir "THIRD-PARTY-NOTICES.txt")
$payloadDir = Join-Path $tauriDir "resources\helper"
Get-ChildItem $payloadDir -File |
  Where-Object { $_.Extension -in ".exe", ".dll" -or $_.Name -eq "payload-manifest.json" } |
  Copy-Item -Destination (Join-Path $portableDir "helper") -Force
$portableReadme = @"
Convenient Window portable package for Windows 11 x64.
Run ConvenientWindow.exe. Application data remains in the current user's Local AppData directory.
Noncommercial use is governed by the included LICENSE file. Commercial use requires separate written permission.
Exit from the tray menu before removing this directory.
"@
[System.IO.File]::WriteAllText(
  (Join-Path $portableDir "README.txt"),
  $portableReadme,
  [System.Text.UTF8Encoding]::new($false)
)

$portableZip = Join-Path $artifactsDir "convenient-window-$version-windows-x64-portable.zip"
Compress-Archive -Path (Join-Path $portableDir "*") -DestinationPath $portableZip -CompressionLevel Optimal
$installerCopy = Join-Path $artifactsDir "convenient-window-$version-windows-x64-setup.exe"
Copy-Item -Force $nsisInstaller.FullName $installerCopy

if ($RequireTrustedSignature) {
  $signatureCommand = Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue
  if (-not $signatureCommand) { throw "Get-AuthenticodeSignature is required when -RequireTrustedSignature is set" }
  $signedBinaries = @(
    (Join-Path $portableDir "ConvenientWindow.exe"),
    (Join-Path $portableDir "helper\magic-corners-helper.exe"),
    $installerCopy
  )
  foreach ($binary in $signedBinaries) {
    $signature = & $signatureCommand $binary
    if (-not $signature -or $signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
      throw "Trusted Authenticode signature required for $binary; status=$($signature.Status)"
    }
  }
}

$deliverables = @($installerCopy, $portableZip) | ForEach-Object {
  $file = Get-Item $_
  [ordered]@{
    name = $file.Name
    bytes = $file.Length
    sha256 = (Get-Sha256 $file.FullName)
  }
}
$finalSourceStatus = @(Get-SourceChanges)
if (($sourceStatus -join "`n") -ne ($finalSourceStatus -join "`n")) {
  throw "Desktop build changed the source worktree"
}

$artifactManifest = [ordered]@{
  schemaVersion = 1
  version = $version
  platform = "windows-x64"
  sourceRepository = "https://github.com/ximizhou/convenient_window_free"
  sourceCommit = $sourceCommit
  dirty = ($finalSourceStatus.Count -gt 0)
  deliverables = @($deliverables)
}
$artifactManifestPath = Join-Path $artifactsDir "artifact-manifest.json"
[System.IO.File]::WriteAllText(
  $artifactManifestPath,
  ($artifactManifest | ConvertTo-Json -Depth 5),
  [System.Text.UTF8Encoding]::new($false)
)

$checksumPath = Join-Path $artifactsDir "SHA256SUMS"
$checksumLines = [string[]]@($deliverables | ForEach-Object { "$($_.sha256)  $($_.name)" })
[System.IO.File]::WriteAllLines(
  $checksumPath,
  $checksumLines,
  [System.Text.UTF8Encoding]::new($false)
)

$deliverables | Format-Table -AutoSize
Write-Output "portable directory: $portableDir"
Write-Output "artifact manifest: $artifactManifestPath"
Write-Output "checksums: $checksumPath"
