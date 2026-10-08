# Native exercise of the production Setup adapter with small synthetic application files.
[CmdletBinding()]
param([string]$CleanupCase)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run without Windows elevation' }
function Remove-FixtureRegistration([string]$CasePath, [string]$Identity) {
    $CasePath = Assert-ChildPath $CasePath "$workspace/.mcp-dev/setup-integration"
    if ($Identity -notmatch '^NohSetupIntegration[0-9a-f]{32}$') { throw 'Invalid fixture identity' }
    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$Identity"
    if (Test-Path -LiteralPath $key) {
        $registered = (Get-ItemProperty -LiteralPath $key).InstallLocation
        # Rust canonical paths use the Win32 extended-length prefix. It is a
        # spelling of the same local path, not another installation identity.
        if ($registered.StartsWith('\\?\')) { $registered = $registered.Substring(4) }
        $actual = [IO.Path]::GetFullPath($registered).TrimEnd('\')
        $expected = [IO.Path]::GetFullPath("$CasePath/installation-base/application").TrimEnd('\')
        if ($actual -ne $expected) { throw 'Refusing unrelated registry cleanup' }
        Remove-Item -LiteralPath $key -Recurse
    }
    return -not (Test-Path -LiteralPath $key)
}
if ($CleanupCase) {
    $CleanupCase = Assert-ChildPath $CleanupCase "$workspace/.mcp-dev/setup-integration"
    $identity = [IO.File]::ReadAllText("$CleanupCase/package-id.txt")
    $cleaned = Remove-FixtureRegistration $CleanupCase $identity
    Write-Json @{registry_cleaned=$cleaned;scope='Exact fixture registration cleanup only'} "$CleanupCase/cleanup-recovery.json"
    return
}
$case = Join-Path $workspace ('.mcp-dev/setup-integration/case-' + [guid]::NewGuid().ToString('N'))
$id = 'NohSetupIntegration' + [guid]::NewGuid().ToString('N')
$registry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$id"
if (Test-Path -LiteralPath $registry) { throw 'Disposable identity collision' }
foreach ($name in @('logs','userdata','tools')) { [void][IO.Directory]::CreateDirectory((Join-Path $case $name)) }
[IO.File]::WriteAllText("$case/package-id.txt", $id)
[IO.File]::WriteAllText("$case/userdata/sentinel", 'preserve-user-data')
$result = [ordered]@{case=$case;package_id=$id;passed=$false;registry_cleaned=$false;scope='Setup adapter; synthetic application payload'}
$inputs = @('src/update.rs','src/update/windows.rs','src/update/windows_anchor.rs','src/update/windows_setup.rs','src/update/setup.rs','src/update/retention.rs','src/update/windows_guard.rs','tools/update_release.rs','src/update_repair_main.rs','tools/update-setup-integration.ps1')
try {
    # Freeze source evidence before compiling or executing any consumer.
    $result.sources = @($inputs | ForEach-Object {
        $source = Join-Path $workspace $_
        $destination = Join-Path "$case/sources" $_
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
        Copy-Item -LiteralPath $source -Destination $destination
        @{path=$_;sha256=(Get-FileHash -LiteralPath $destination).Hash}
    })
    Write-Json $result.sources "$case/sources.json"
    # Reuse the cached official archive only after checking its independent pin
    # and every executable/configuration dependency consumed by the packager.
    $archivePath = "$qualificationBase/vpk.1.2.161.zip"
    if ((Get-FileHash -LiteralPath $archivePath).Hash.ToLowerInvariant() -ne '2b56ce117f803fc70c103cb423bd040e395e40370f6ff6e10818e9ff9c26a323') { throw 'Unexpected official packager archive' }
    $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName.EndsWith('/') -or $entry.FullName -notmatch '^(tools/net8.0/any/|vendor/)') { continue }
            $stream = $entry.Open()
            try { $expected = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)) } finally { $stream.Dispose() }
            $path = Assert-ChildPath "$qualificationBase/vpk/$($entry.FullName)" "$qualificationBase/vpk"
            if ((Get-FileHash -LiteralPath $path).Hash -ne $expected) { throw "Packager dependency mismatch: $($entry.FullName)" }
        }
    } finally { $archive.Dispose() }
    $env:CC = "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin/clang.exe"
    $env:AR = "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin/llvm-ar.exe"
    $env:CARGO_TARGET_DIR = "$workspace/target"
    Invoke-RecordedProcess (Get-Command cargo).Source @('build','--locked','--offline','--profile','qa','--features','updates','--example','update-release','--bin','noh-update-repair') $workspace "$case/logs/build-tools" 300
    $tool = "$case/tools/update-release.exe"
    Copy-Item -LiteralPath "$workspace/target/qa/examples/update-release.exe" -Destination $tool
    # This payload exercises packing/publication only; real NOH clients follow in a separate qualification slice.
    [IO.File]::WriteAllText("$case/fixture.rs", 'fn main() { if std::env::args().nth(1).is_some_and(|a| a.starts_with("--veloapp-")) { return; } println!("synthetic Setup integration payload"); }')
    Invoke-RecordedProcess (Get-Command rustc).Source @('--edition','2024',"$case/fixture.rs",'-o',"$case/fixture.exe",'-C','linker=rust-lld','-C','linker-flavor=ld.lld') $workspace "$case/logs/build-fixture" 60
    Invoke-RecordedProcess $tool @('keygen','--private-key',"$case/key.pk8",'--public-keys',"$case/keys.json",'--key-id','local-setup-integration') $case "$case/logs/keygen" 10
    foreach ($label in @('A','B')) {
        $version = if ($label -eq 'A') {'1.0.0'} else {'1.0.1'}
        $bundle = "$case/payload-$label"
        foreach ($name in @('bin','models')) { [void][IO.Directory]::CreateDirectory("$bundle/$name") }
        foreach ($name in @('noh.exe','bin/noh-cli.exe','bin/noh-mcp.exe','bin/noh-update-guard.exe','bin/noh-update-repair.exe')) { Copy-Item -LiteralPath "$case/fixture.exe" -Destination "$bundle/$name" }
        [IO.File]::WriteAllText("$bundle/models/model.bin", "offline-model-$label")
        Write-Json @{build=@{build_fingerprint=($label.ToLowerInvariant()*64);package_version=$version;target='x86_64-pc-windows-gnu';features=@('gui','mcp','updates')}} "$bundle/manifest.json"
        $packages = "$case/packages-$label"
        Invoke-RecordedProcess "$qualificationBase/dotnet8/dotnet.exe" @("$qualificationBase/vpk/tools/net8.0/any/vpk.dll",'pack','--packId',$id,'--packVersion',$version,'--packDir',$bundle,'--mainExe','noh.exe','--runtime','win-x64','--channel','win-x64-stable','--delta','None','--noPortable','--skipVeloAppCheck','--shortcuts','None','--outputDir',$packages) $case "$case/logs/pack-$label" 120
        $setups = @(Get-ChildItem -LiteralPath $packages -Filter '*-Setup.exe')
        if ($setups.Count -ne 1) { throw 'Expected one full Setup' }
        Invoke-RecordedProcess $tool @('describe-setup','--bundle',$bundle,'--setup',$setups[0].FullName,'--package-id',$id,'--version',$version,'--url',"https://github.com/example/noh/releases/download/test/$($setups[0].Name)",'--output',"$case/release-$label.json") $case "$case/logs/describe-$label" 30
        Invoke-RecordedProcess $tool @('sign','--private-key',"$case/key.pk8",'--key-id','local-setup-integration','--release',"$case/release-$label.json",'--packages',$packages,'--output',"$case/envelope-$label.json") $case "$case/logs/sign-$label" 30
    }
    $env:NOH_SETUP_INTEGRATION_CASE = $case
    Invoke-RecordedProcess (Get-Command cargo).Source @('test','--locked','--offline','--profile','qa','--features','updates','--lib','update::windows_setup','--','--include-ignored','--test-threads=1','--nocapture') $workspace "$case/logs/adapter" 300
    $passed = Get-Content -LiteralPath "$case/passed.json" -Raw | ConvertFrom-Json
    if (-not $passed.passed) { throw 'Adapter did not complete native assertions' }
    $installed = Get-ItemProperty -LiteralPath $registry
    if ($installed.DisplayVersion -ne '1.0.1') { throw 'Final registered version mismatch' }
    $result.passed = $true
} catch {
    $result.error = $_.Exception.Message
    throw
} finally {
    try { $result.registry_cleaned = Remove-FixtureRegistration $case $id }
    catch { $result.cleanup_error = $_.Exception.Message; $result.passed = $false; throw }
    finally {
        Write-Json $result "$case/result.json"
        Write-Output "Setup adapter result: $case/result.json"
    }
}
