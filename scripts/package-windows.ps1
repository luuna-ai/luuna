# Builds a Windows development distribution of Luuna: an NSIS installer that
# carries the Tauri desktop, the release runtime, the Windows helper scripts,
# the pinned Pi tree (ADR 0018) and the Rust terminal client (luuna-tui.exe, one
# self-contained executable that carries Missions, Atlas and Beacon natively, ADR 0066).
# Node.js (for the pinned Pi engine), Git and Python stay host prerequisites that
# `luuna-runtime doctor` reports. The installer is unsigned unless a
# certificate thumbprint is supplied; an unsigned installer is a development
# distribution, never release evidence. See docs/PACKAGING.md for the layout
# and the clean-machine procedure.
#
#   powershell -NoProfile -File scripts/package-windows.ps1 [-DebugBuild] [-SkipPi] [-CertificateThumbprint <sha1>] [-TimestampUrl <url>]
[CmdletBinding()]
param(
    [switch]$DebugBuild,
    [switch]$SkipPi,
    [string]$CertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tauriDirectory = Join-Path $root 'apps\desktop\src-tauri'
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$stage = Join-Path $tauriDirectory ('target\stage-' + $profile)
$started = Get-Date

function Step([string]$text) { Write-Host ("==> " + $text) }
function Require-Command([string]$name, [string]$hint) {
    if (!(Get-Command $name -ErrorAction SilentlyContinue)) { throw ($name + ' is required: ' + $hint) }
}
function Sha256([string]$path) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() }

Step 'Prerequisites'
Require-Command cargo 'install the pinned Rust toolchain (rust-toolchain.toml)'
Require-Command node.exe 'install Node.js 24.19 or newer'
Require-Command npm.cmd 'install npm with Node.js'
Require-Command git.exe 'install Git 2.43 or newer'
if (!(Test-Path -LiteralPath (Join-Path $root 'node_modules\.bin\tauri.cmd'))) { throw 'Run npm ci --ignore-scripts at the repository root first (the Tauri CLI is a pinned devDependency).' }
$webview = @(
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (!$webview) { Write-Warning 'The WebView2 Runtime registration was not found on this machine; the installer still requires it on the target.' }
$rustVersion = (& cargo --version)
$nodeVersion = (& node.exe --version)

Step 'Runtime and Rust terminal (locked workspace toolchain)'
$cargoArguments = @('build', '--locked', '-p', 'luuna-runtime', '-p', 'luuna-tui', '-p', 'luuna-sandbox')
if (!$DebugBuild) { $cargoArguments += '--release' }
& cargo @cargoArguments
if ($LASTEXITCODE -ne 0) { throw 'The runtime build failed.' }
$cargoTarget = if (!$env:CARGO_TARGET_DIR) { Join-Path $root 'target' } elseif ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { $env:CARGO_TARGET_DIR } else { Join-Path $root $env:CARGO_TARGET_DIR }
$runtime = Join-Path $cargoTarget ($profile + '\luuna-runtime.exe')
$terminal = Join-Path $cargoTarget ($profile + '\luuna-tui.exe')
if (!(Test-Path -LiteralPath $runtime)) { throw ('Runtime executable missing: ' + $runtime) }
if (!(Test-Path -LiteralPath $terminal)) { throw ('Rust terminal executable missing: ' + $terminal) }
$sandboxLauncher = Join-Path $cargoTarget ($profile + '\luuna-sbx.exe')
if (!(Test-Path -LiteralPath $sandboxLauncher)) { throw ('Sandbox launcher executable missing: ' + $sandboxLauncher) }

$piSource = Join-Path $root 'tools\pi'
if (!$SkipPi) {
    Step 'Pinned Pi tree (npm ci --ignore-scripts --omit=dev in tools/pi)'
    Push-Location $piSource
    try {
        & npm.cmd ci --ignore-scripts --omit=dev
        if ($LASTEXITCODE -ne 0) { throw 'Installing the pinned Pi package failed.' }
    } finally { Pop-Location }
    $cli = Join-Path $piSource 'node_modules\@earendil-works\pi-coding-agent\dist\cli.js'
    if (!(Test-Path -LiteralPath $cli)) { throw 'The pinned Pi CLI was not installed.' }
    $piVersion = (& node.exe $cli --version)
    if ($LASTEXITCODE -ne 0) { throw 'The pinned Pi CLI does not answer --version.' }
} else { $piVersion = 'skipped' }

Step ('Staging resources in ' + $stage)
$stage = [IO.Path]::GetFullPath($stage)
$stageParent = [IO.Path]::GetFullPath((Join-Path $tauriDirectory 'target')) + [IO.Path]::DirectorySeparatorChar
if (!$stage.StartsWith($stageParent, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing to replace a stage outside the desktop target directory.' }
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'scripts') | Out-Null
Copy-Item -LiteralPath $runtime -Destination (Join-Path $stage 'luuna-runtime.exe')
foreach ($script in @('windows-common.ps1', 'ensure-windows-runtime.ps1', 'start-windows.ps1', 'stop-windows.ps1', 'set-windows-provider.ps1', 'with-windows-provider.ps1', 'start-terminal.ps1')) {
    Copy-Item -LiteralPath (Join-Path $root ('scripts\' + $script)) -Destination (Join-Path $stage ('scripts\' + $script))
}
foreach ($launcher in @('Configure Luuna.cmd', 'Stop Luuna.cmd')) {
    Copy-Item -LiteralPath (Join-Path $root $launcher) -Destination (Join-Path $stage $launcher)
}
if (!$SkipPi) {
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'pi') | Out-Null
    foreach ($file in @('package.json', 'package-lock.json')) { Copy-Item -LiteralPath (Join-Path $piSource $file) -Destination (Join-Path $stage ('pi\' + $file)) }
    # robocopy returns 0-7 for success.
    & robocopy (Join-Path $piSource 'node_modules') (Join-Path $stage 'pi\node_modules') /E /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -gt 7) { throw 'Copying the pinned Pi tree failed.' }
}
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'LICENSE')
Copy-Item -LiteralPath (Join-Path $root 'THIRD-PARTY-NOTICES.md') -Destination (Join-Path $stage 'THIRD-PARTY-NOTICES.md')

Step 'Rust terminal client'
# One executable: the command table and the capability catalog are embedded, and the
# workbench runs its actions in process, so nothing else is staged for the terminal.
& node.exe (Join-Path $root 'scripts\stage-terminal.mjs') --root $root --out $stage --binary $terminal
if ($LASTEXITCODE -ne 0) { throw 'Staging the Rust terminal payload failed.' }
# The staged terminal client must start without the checkout.
& (Join-Path $stage 'luuna-tui.exe') --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'The staged terminal client did not start (luuna-tui.exe --help failed).' }
$terminalLauncher = @(
    '@echo off',
    'rem Opens the Luuna terminal client against the local runtime.',
    'powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\start-terminal.ps1" %*'
) -join "`r`n"
Set-Content -LiteralPath (Join-Path $stage 'Luuna Terminal.cmd') -Value $terminalLauncher -Encoding ascii
# The same launcher under the short name `luuna` for a PATH entry on the install folder.
Set-Content -LiteralPath (Join-Path $stage 'luuna.cmd') -Value $terminalLauncher -Encoding ascii
# `lu` is the shorter name (ADR 0036); `lu sbx` finds luuna-sbx.exe beside it.
Set-Content -LiteralPath (Join-Path $stage 'lu.cmd') -Value $terminalLauncher -Encoding ascii
Copy-Item -LiteralPath $sandboxLauncher -Destination (Join-Path $stage 'luuna-sbx.exe')

Step 'Bundle configuration overlay'
$stageRelative = ('target/stage-' + $profile)
$bundle = [ordered]@{
    active = $true
    targets = @('nsis')
    resources = [ordered]@{ ($stageRelative + '/') = 'luuna/' }
    windows = [ordered]@{ webviewInstallMode = [ordered]@{ type = 'downloadBootstrapper' } }
}
if ($CertificateThumbprint) {
    $bundle.windows.certificateThumbprint = $CertificateThumbprint
    $bundle.windows.digestAlgorithm = 'sha256'
    $bundle.windows.timestampUrl = $TimestampUrl
}
$overlay = [ordered]@{ bundle = $bundle }
$overlayPath = Join-Path $tauriDirectory ('target\bundle-overlay-' + $profile + '.json')
$overlay | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $overlayPath -Encoding utf8

Step 'tauri build'
$tauriArguments = @('exec', '--workspace', '@luuna/desktop', '--', 'tauri', 'build', '--config', $overlayPath)
if ($DebugBuild) { $tauriArguments += '--debug' }
Push-Location $root
try {
    & npm.cmd @tauriArguments
    if ($LASTEXITCODE -ne 0) { throw 'tauri build failed.' }
} finally { Pop-Location }

Step 'Verifying the packaged layout'
$built = Join-Path $tauriDirectory ('target\' + $profile)
$copied = Join-Path $built 'luuna'
foreach ($required in @('luuna-runtime.exe', 'luuna-tui.exe', 'luuna-sbx.exe', 'lu.cmd', 'scripts\ensure-windows-runtime.ps1', 'scripts\windows-common.ps1', 'scripts\start-terminal.ps1', 'Configure Luuna.cmd', 'Luuna Terminal.cmd', 'luuna.cmd')) {
    if (!(Test-Path -LiteralPath (Join-Path $copied $required))) { throw ('Packaged resource missing after build: ' + $required) }
}
if (!$SkipPi -and !(Test-Path -LiteralPath (Join-Path $copied 'pi\node_modules\@earendil-works\pi-coding-agent\dist\cli.js'))) { throw 'Packaged Pi tree missing after build.' }
$installers = @(Get-ChildItem -LiteralPath (Join-Path $built 'bundle\nsis') -Filter '*.exe' -ErrorAction SilentlyContinue)
if (!$installers.Count) { throw 'No NSIS installer was produced.' }
$desktop = Join-Path $built 'luuna-desktop.exe'
$signed = $false
if ($CertificateThumbprint) {
    $signature = Get-AuthenticodeSignature -LiteralPath $installers[0].FullName
    $signed = $signature.Status -eq 'Valid'
    if (!$signed) { throw ('The installer is not validly signed: ' + $signature.Status) }
}

$report = [ordered]@{
    built_at = $started.ToUniversalTime().ToString('o')
    profile = $profile
    host = [ordered]@{ os = [Environment]::OSVersion.VersionString; architecture = $env:PROCESSOR_ARCHITECTURE; rust = $rustVersion; node = $nodeVersion; pi = $piVersion }
    signed = $signed
    webview_install_mode = 'downloadBootstrapper'
    prerequisites_on_target = @('WebView2 Runtime (bootstrapped by the installer when absent)', 'Node.js 24.19 or newer on PATH for the pinned Pi engine', 'Git 2.43 or newer on PATH', 'Python 3 for Python check recipes', 'a provider key saved with Configure Luuna.cmd')
    components = 'native in luuna-tui.exe and luuna-runtime.exe (ADR 0066)'
    terminal_client = 'Luuna Terminal.cmd (the Rust terminal client, one executable; ADR 0037)'
    terminal_executable = [ordered]@{ path = (Join-Path $copied 'luuna-tui.exe'); sha256 = (Sha256 (Join-Path $copied 'luuna-tui.exe')) }
    sandbox_launcher = [ordered]@{ path = (Join-Path $copied 'luuna-sbx.exe'); sha256 = (Sha256 (Join-Path $copied 'luuna-sbx.exe')) }
    clean_machine_test = 'not executed by this script; see docs/PACKAGING.md'
    artifacts = @(
        foreach ($installer in $installers) { [ordered]@{ path = $installer.FullName; sha256 = (Sha256 $installer.FullName); size_bytes = $installer.Length } }
    )
    desktop_executable = [ordered]@{ path = $desktop; sha256 = (Sha256 $desktop) }
    runtime_executable = [ordered]@{ path = (Join-Path $copied 'luuna-runtime.exe'); sha256 = (Sha256 (Join-Path $copied 'luuna-runtime.exe')) }
}
$reportPath = Join-Path $built ('package-report-' + $started.ToUniversalTime().ToString('yyyyMMddTHHmmssZ') + '.json')
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $reportPath -Encoding utf8
Write-Host ''
Write-Host ('Installer: ' + $installers[0].FullName)
Write-Host ('SHA-256:   ' + $report.artifacts[0].sha256)
Write-Host ('Report:    ' + $reportPath)
if (!$signed) { Write-Warning 'Unsigned development distribution: Windows SmartScreen will warn on a fresh host. Supply -CertificateThumbprint for a signed build.' }
