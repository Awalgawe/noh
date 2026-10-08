[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('windows-x64','macos-arm64')][string]$Platform,
    [ValidateSet('compile-only','portable')][string]$BuildKind = 'compile-only'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This runner modifies toolchains/Homebrew; use only on a disposable GitHub runner' }
$dirty = git status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Delivery requires a clean public checkout' }
if (Test-Path -LiteralPath (Join-Path $root 'dist')) { throw 'A fresh delivery checkout must not contain dist/' }
if (Test-Path -LiteralPath (Join-Path $root '.mcp-dev')) { throw 'A fresh delivery checkout must not contain maintainer inputs' }
$python = if ($IsWindows) { 'python' } else { 'python3' }
& $python -c 'import sys; assert sys.version_info >= (3,12), "Python 3.12+ is required for safe source extraction"'
$policy = Get-Content assets/delivery-policy.json -Raw | ConvertFrom-Json
$target = $policy.platforms.$Platform.target
$toolchain = "$($policy.rust_version)-$target"
$logRoot = Join-Path $root 'logs/delivery'
New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
Start-Transcript -Path (Join-Path $logRoot 'build.log') -NoClobber | Out-Null
try {
    if ($env:NOH_SIGN_MACOS -eq 'true') {
        if ($IsWindows -or $Platform -ne 'macos-arm64' -or $BuildKind -ne 'portable') { throw 'Signing requires a macOS portable candidate' }
    }
    rustup toolchain install $toolchain --profile minimal --component rustfmt
    rustup default $toolchain
    $env:RUSTUP_TOOLCHAIN = $toolchain
    # Keep the checked-in Windows GNU/LLD target configuration intact.
    $env:CARGO_NET_OFFLINE = 'false'
    $environment = @{ platform=$Platform; build_kind=$BuildKind; commit=(git rev-parse HEAD); runner=$env:ImageOS; image=$env:ImageVersion; rust=(rustc -Vv | Out-String); cargo=(cargo -V); python=(& $python --version); linker='checked-in .cargo/config.toml'; features=@('gui','mcp'); profile='release'; status='unqualified' }
    $environment | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $logRoot 'BUILD-ENVIRONMENT.json') -Encoding utf8NoBOM
    if ($BuildKind -eq 'compile-only') {
        cargo build --release --locked --features gui,mcp --bins
        # Executables stay on the disposable runner; this mode distributes no binaries.
        return
    }
    & $python tools/delivery.py acquire --platform $Platform --cache (Join-Path $env:RUNNER_TEMP 'noh-downloads') --output (Join-Path $logRoot 'inputs')
    $inputs = Join-Path $logRoot 'inputs'
    $speech = Join-Path $inputs 'speech'
    if ($IsWindows) {
        $ffmpeg = Join-Path $inputs 'native/export/ffmpeg.exe'
        $env:NOH_LIBMPV = Join-Path $inputs 'native/preview/libmpv-2.dll'
        $env:NOH_NATIVE_NOTICES = Join-Path $inputs 'materials/native-notices'
        $bundle = Join-Path $root 'dist/noh'
    } else {
        $paths = Get-Content (Join-Path $inputs 'PATHS.json') -Raw | ConvertFrom-Json
        $ffmpeg = $paths.ffmpeg
        $env:NOH_LIBMPV = $paths.libmpv
        $bundle = Join-Path $root 'dist/NOH-macos-arm64'
    }
    $env:NOH_FFMPEG = $ffmpeg
    cargo fetch --locked
    # Prepare original notices before packaging/signatures/manifests are created.
    & $python tools/delivery.py sources --cache (Join-Path $env:RUNNER_TEMP 'noh-downloads') --output (Join-Path $logRoot 'materials')
    $env:NOH_RUST_NOTICES = Join-Path $logRoot 'materials/rust-notices'
    $arguments = @('dev','build','--gui','--mcp','--offline','--ffmpeg',$ffmpeg,'--speech',$speech)
    if (-not $IsWindows) { $arguments += '--portable' }
    & cargo @arguments
    if ($IsWindows) {
        & $python tools/delivery.py audit-pe --bundle $bundle --output (Join-Path $logRoot 'PE-IMPORTS.json')
    }
    $env:NOH_EXE = if ($IsWindows) { Join-Path $bundle 'bin/noh-cli.exe' } else { Join-Path $bundle 'NOH.app/Contents/MacOS/bin/noh-cli' }
    $env:NOH_APP_EXE = if ($IsWindows) { Join-Path $bundle 'noh.exe' } else { Join-Path $bundle 'NOH.app/Contents/MacOS/noh-app' }
    $env:NOH_MCP_EXE = if ($IsWindows) { Join-Path $bundle 'bin/noh-mcp.exe' } else { Join-Path $bundle 'NOH.app/Contents/MacOS/bin/noh-mcp' }
    if ($env:NOH_SIGN_MACOS -ne 'true') { cargo dev verify --media --mcp --offline }
    # Sources are kept as separate assets rather than executable startup inputs.
    Copy-Item -LiteralPath (Join-Path $inputs 'materials') -Destination (Join-Path $logRoot 'materials/native') -Recurse
    if ($IsWindows) {
        Copy-Item -LiteralPath (Join-Path $logRoot 'PE-IMPORTS.json') -Destination (Join-Path $logRoot 'materials/PE-IMPORTS.json')
    }
    Copy-Item -LiteralPath (Join-Path $logRoot 'BUILD-ENVIRONMENT.json') -Destination (Join-Path $logRoot 'materials/BUILD-ENVIRONMENT.json')
    if ($env:NOH_SIGN_MACOS -eq 'true') {
        Write-Output 'Candidate prepared; native signing, tests and final seal are deferred to separate steps without build-time credentials'
    } else {
        & $python tools/delivery.py seal --platform $Platform --bundle $bundle --materials (Join-Path $logRoot 'materials') --output (Join-Path $logRoot 'assets') --commit $env:GITHUB_SHA
    }
    # This is explicitly an unqualified candidate, never a publication gate bypass.
} finally {
    Stop-Transcript | Out-Null
}
