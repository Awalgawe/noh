# Build once in CI, then accept those exact Setup bytes locally. No publication.
[CmdletBinding()]
param(
    [ValidateSet('All','Build','Accept')][string]$Stage = 'All',
    [string]$CandidateDirectory,
    [string]$ExpectedCommit,
    [string]$PreviousCandidateDirectory,
    [string]$ExpectedPreviousCommit,
    [string]$ExpectedPreviousCandidateSha256,
    [switch]$NewVersionOnly,
    [string]$BaselineLock,
    [switch]$ValidateInputsOnly,
    [string]$PublicKeys,
    [string]$SigningKey,
    [string]$PackageId,
    [string]$KeyId = 'noh-release',
    [string]$VersionA = '0.1.0',
    [string]$VersionB = '0.1.1',
    [ValidateSet('minimal','standard','complete')][string]$Profile = 'complete',
    [string]$Repository = 'Awalgawe/noh',
    [string]$RuntimeDirectory,
    [string]$Ffmpeg,
    [string]$Speech,
    [string]$Libmpv,
    [string]$NativeNotices,
    [string]$CompilerDirectory,
    [string]$ToolDirectory
)
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'Windows PowerShell 7 is required' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($ValidateInputsOnly -and $Stage -ne 'Accept') { throw 'Input validation is only available for Accept' }
if (($PreviousCandidateDirectory -or $ExpectedPreviousCommit -or $ExpectedPreviousCandidateSha256) -and
    ($Stage -notin @('Accept','All') -or -not $PreviousCandidateDirectory -or
     $ExpectedPreviousCommit -notmatch '^[0-9a-f]{40}$' -or $ExpectedPreviousCandidateSha256 -notmatch '^[0-9a-f]{64}$')) {
    throw 'Historical acceptance requires a previous candidate and its independent commit and catalog digest'
}
$baseline = $null
if ($NewVersionOnly) {
    if ($Stage -eq 'Accept' -or -not $BaselineLock -or $PackageId -ne 'NOH') { throw 'B-only production requires release mode and a retained baseline lock' }
    $BaselineLock = Assert-PlainPath $BaselineLock
    $baseline = Get-Content -LiteralPath $BaselineLock -Raw | ConvertFrom-Json
    if ($baseline.schema -ne 1 -or $baseline.repository -ne $Repository -or $baseline.package_id -ne 'NOH' -or
        $baseline.source_commit -notmatch '^[0-9a-f]{40}$' -or $baseline.candidate_sha256 -notmatch '^[0-9a-f]{64}$') { throw 'Invalid retained baseline identity' }
    $VersionA = $baseline.version
    if ($Stage -eq 'All' -and -not $PreviousCandidateDirectory) { throw 'B-only native acceptance needs the acquired historical candidate' }
}
$buildLabels = if ($NewVersionOnly) { @('B') } else { @('A','B') }
$case = Join-Path $workspace ('.mcp-dev/setup-clients/case-' + [guid]::NewGuid().ToString('N'))
if ($Stage -eq 'Accept') {
    if (-not $CandidateDirectory) { throw 'Accept requires the downloaded candidate directory' }
    $inputCase = Assert-PlainPath $CandidateDirectory
    $candidate = Get-Content -LiteralPath "$inputCase/candidate.json" -Raw | ConvertFrom-Json
    if ($candidate.schema -ne 1 -or $candidate.package_id -notmatch '^(NOH|NohSetupClients[0-9a-f]{32})$') { throw 'Invalid private candidate' }
    if ($ExpectedCommit -notmatch '^[0-9a-f]{40}$' -or $candidate.source_commit -ne $ExpectedCommit) { throw 'Accept requires the independently observed successful CI commit' }
    $id = $candidate.package_id
    $VersionA = $candidate.version_a; $VersionB = $candidate.version_b
    foreach ($entry in $candidate.files) {
        $file = Assert-ChildPath "$inputCase/$($entry.path)" $inputCase
        if ((Get-FileHash -LiteralPath $file).Hash.ToLowerInvariant() -ne $entry.sha256) { throw "Changed candidate file: $($entry.path)" }
    }
    if ($candidate.PSObject.Properties['baseline_sha256']) {
        if ((Get-FileHash -LiteralPath "$inputCase/baseline.json").Hash.ToLowerInvariant() -ne $candidate.baseline_sha256) { throw 'Candidate baseline lock changed' }
        $baseline = Get-Content -LiteralPath "$inputCase/baseline.json" -Raw | ConvertFrom-Json
        if (-not $PreviousCandidateDirectory -or $baseline.version -ne $VersionA) { throw 'B-only acceptance requires its recorded historical baseline' }
    }
} else {
    if ($CandidateDirectory) { $case = Assert-PlainPath $CandidateDirectory }
    if (Test-Path -LiteralPath $case) { throw 'Build requires a new candidate directory' }
    $inputCase = $case
    if ($PublicKeys -or $SigningKey -or $PackageId) {
        if (-not $PublicKeys -or -not $SigningKey -or $PackageId -ne 'NOH') { throw 'Release mode requires independent public keys, signing key and package ID NOH' }
        $id = $PackageId
    } else {
        $id = 'NohSetupClients' + [guid]::NewGuid().ToString('N')
    }
}
$previousCase = $null
if ($baseline -and $Stage -ne 'Build' -and
    ($ExpectedPreviousCommit -ne $baseline.source_commit -or $ExpectedPreviousCandidateSha256 -ne $baseline.candidate_sha256)) {
    throw 'Historical acceptance must use the exact baseline recorded by the producer'
}
if ($PreviousCandidateDirectory) {
    $previousCase = Assert-PlainPath $PreviousCandidateDirectory
    $previousCatalogBytes = [IO.File]::ReadAllBytes("$previousCase/candidate.json")
    $previousCatalogHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($previousCatalogBytes)).ToLowerInvariant()
    if ($previousCatalogHash -ne $ExpectedPreviousCandidateSha256) { throw 'Historical catalog differs from independent provenance' }
    $previous = [Text.Encoding]::UTF8.GetString($previousCatalogBytes) | ConvertFrom-Json
    if ($previous.schema -ne 1 -or $previous.repository -ne $Repository -or
        $previous.package_id -ne $id -or $previous.source_commit -ne $ExpectedPreviousCommit) {
        throw 'Historical candidate identity differs from the reviewed source'
    }
    # B is the historical release. Keep its original catalog, without requiring
    # the superseded synthetic A payloads to be downloaded or retained again.
    $historicalSuffix = if ($Profile -eq 'complete') { '' } else { "-$Profile" }
    $requiredHistorical = @('keys.json','source-inventory.json','tools/bootstrap-B.exe','tools/update-release.exe',"release-B$historicalSuffix.json","envelope-B$historicalSuffix.json")
    $historicalPackage = @($previous.files | Where-Object { $_.path -like "packages-B$historicalSuffix/*-Setup.exe" })
    if ($historicalPackage.Count -ne 1) { throw 'Historical profile package is missing or ambiguous' }
    $requiredHistorical += $historicalPackage[0].path
    foreach ($name in $requiredHistorical) {
        $matches = @($previous.files | Where-Object path -eq $name)
        if ($matches.Count -ne 1) { throw "Historical member missing or ambiguous: $name" }
        $entry = $matches[0]
        $file = Assert-ChildPath "$previousCase/$($entry.path)" $previousCase
        if ((Get-FileHash -LiteralPath $file).Hash.ToLowerInvariant() -ne $entry.sha256) {
            throw "Changed historical candidate file: $($entry.path)"
        }
    }
    $VersionA = $previous.version_b
    if ($baseline -and $VersionA -ne $baseline.version) { throw 'Historical version differs from the retained baseline' }
    $historicalRelease = Get-Content -LiteralPath "$previousCase/release-B$historicalSuffix.json" -Raw | ConvertFrom-Json -AsHashtable
    $historicalProfile = if ($historicalRelease.ContainsKey('profile')) { $historicalRelease.profile } else { 'complete' }
    if ($historicalRelease.version -ne $VersionA -or $historicalRelease.package_id -ne $id -or $historicalProfile -ne $Profile -or
        $historicalRelease.artifacts.Count -ne 1 -or "packages-B$historicalSuffix/$($historicalRelease.artifacts[0].file_name)" -ne $historicalPackage[0].path -or
        $historicalRelease.artifacts[0].sha256 -ne $historicalPackage[0].sha256) { throw 'Historical profile descriptor differs' }
}
Assert-VersionPair $VersionA $VersionB
if ($ValidateInputsOnly) {
    # A cheap transport/preflight check, not signature or native qualification.
    @{profile=$Profile;version_a=$VersionA;version_b=$VersionB;historical=([bool]$previousCase);baseline_commit=$ExpectedPreviousCommit;baseline_sha256=$ExpectedPreviousCandidateSha256} | ConvertTo-Json
    return
}
if ($Stage -ne 'Build' -and $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run without Windows elevation' }
if (Get-Process -Name cargo,rustc -ErrorAction SilentlyContinue) { throw 'Wait for the existing build' }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Invalid GitHub repository' }
$releaseTag = "v$VersionB"
$base = "$case/installation-base"
$root = "$base/application"
$userProfile = "$case/userdata"
$registry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$id"
foreach ($folder in @('logs','tools','userdata/noh','installation-base')) { [void][IO.Directory]::CreateDirectory("$case/$folder") }
$result = [ordered]@{case=$case;package_id=$id;profile=$Profile;passed=$false;registry_cleaned=$false;scope='Real NOH GUI/CLI/MCP; exact signed content profile';stage=$Stage;candidate=$inputCase}
if ($Stage -eq 'Accept') {
    $result.candidate_sha256 = (Get-FileHash -LiteralPath "$inputCase/candidate.json").Hash.ToLowerInvariant()
    $result.source_commit = $candidate.source_commit
}
if ($previousCase) {
    $result.previous_source_commit = $previous.source_commit
    $result.previous_candidate_sha256 = $previousCatalogHash
    $result.previous_version = $VersionA
}
$gui = $null; $guardian = $null; $restarted = $null
function NormalPath([string]$Path) {
    if ($Path.StartsWith('\\?\')) { $Path = $Path.Substring(4) }
    [IO.Path]::GetFullPath($Path).TrimEnd('\')
}
function Stop-Owned($Process, [string]$Expected) {
    if (-not $Process -or $Process.HasExited) { return }
    [void]$Process.SafeHandle
    if ((NormalPath $Process.MainModule.FileName) -ne (NormalPath $Expected)) { throw 'Refusing to terminate an unrelated process' }
    $Process.Kill($true)
    if (-not $Process.WaitForExit(10000)) { throw 'Owned process did not terminate' }
}
function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path).Hash.ToLowerInvariant() }
function Check-Identity([string]$Label, [string]$Fingerprint, [string]$Version) {
    $installedManifest = Get-Content -LiteralPath "$root/current/manifest.json" -Raw | ConvertFrom-Json -AsHashtable
    $installedProfile = if ($installedManifest.ContainsKey('distribution_profile')) { $installedManifest.distribution_profile } else { 'complete' }
    $anchorIdentity = Get-Content -LiteralPath "$base/.noh-update/identity.json" -Raw | ConvertFrom-Json -AsHashtable
    $anchorProfile = if ($anchorIdentity.ContainsKey('profile')) { $anchorIdentity.profile } else { 'complete' }
    if ($installedProfile -ne $Profile -or $anchorProfile -ne $Profile) { throw 'Installed content profile changed' }
    if ($Profile -ne 'complete' -and (Test-Path -LiteralPath "$root/current/bin/speech")) { throw 'Unexpected speech in a smaller profile' }
    if ($Profile -eq 'minimal' -and ((Test-Path -LiteralPath "$root/current/bin/ffmpeg.exe") -or (Test-Path -LiteralPath "$root/current/bin/preview"))) { throw 'Unexpected bundled media tools in Minimal' }
    foreach ($relative in @('noh.exe','bin/noh-cli.exe','bin/noh-mcp.exe')) {
        $log = "$case/logs/identity-$Label-" + $relative.Replace('/','-')
        $json = Invoke-RecordedProcess "$root/current/$relative" @('--build-info') $case $log 15 -PassOutput | ConvertFrom-Json
        if ($json.package_version -ne $Version -or $json.build_fingerprint -ne $Fingerprint) { throw "Client identity mismatch: $relative" }
    }
}
try {
    if (Test-Path -LiteralPath $registry) { throw 'Disposable identity collision' }
    if ($Stage -ne 'Accept') {
        $runtime = if ($RuntimeDirectory) { Assert-PlainPath $RuntimeDirectory } else { "$workspace/dist/noh" }
        $ffmpegInput = if ($Ffmpeg) { Assert-PlainPath $Ffmpeg } else { "$runtime/bin/ffmpeg.exe" }
        $speechInput = if ($Speech) { Assert-PlainPath $Speech } else { $runtime }
        $mpvInput = if ($Libmpv) { Assert-PlainPath $Libmpv } else { "$runtime/bin/preview/libmpv-2.dll" }
        if ($ToolDirectory) { $qualificationBase = Assert-PlainPath $ToolDirectory }
        $compiler = if ($CompilerDirectory) { Assert-PlainPath $CompilerDirectory } else { "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin" }
        foreach ($file in @($ffmpegInput,$mpvInput)) { [void](Get-Item -LiteralPath $file) }
        $requiredMultiplier = if ($Stage -eq 'Build') { 6 } else { 12 }
        $runtimeBytes = (Get-ChildItem -LiteralPath $speechInput -Recurse -File | Measure-Object Length -Sum).Sum
        if (([IO.DriveInfo]::new([IO.Path]::GetPathRoot($case))).AvailableFreeSpace -lt ($requiredMultiplier*$runtimeBytes + 8GB)) { throw 'Insufficient qualification space' }
        $archivePath = "$qualificationBase/vpk.1.2.161.zip"
        if ((Hash $archivePath) -ne '2b56ce117f803fc70c103cb423bd040e395e40370f6ff6e10818e9ff9c26a323') { throw 'Unexpected official packager archive' }
        $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
        try {
            foreach ($entry in $archive.Entries) {
                if ($entry.FullName.EndsWith('/') -or $entry.FullName -notmatch '^(tools/net8.0/any/|vendor/)') { continue }
                $stream = $entry.Open()
                try { $expected = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)) } finally { $stream.Dispose() }
                $path = Assert-ChildPath "$qualificationBase/vpk/$($entry.FullName)" "$qualificationBase/vpk"
                if ((Get-FileHash -LiteralPath $path).Hash -ne $expected) { throw "Packager closure mismatch: $($entry.FullName)" }
            }
        } finally { $archive.Dispose() }
        foreach ($key in @(Get-ChildItem Env: | Where-Object {$_.Name -like 'NOH_*'})) { Remove-Item -LiteralPath "Env:$($key.Name)" }
        $env:CC = "$compiler/clang.exe"
        $env:AR = "$compiler/llvm-ar.exe"
        $env:CARGO_TARGET_DIR = "$workspace/target"
        # Freeze all build/package sources before compiling the requested versions.
        $paths = @(& git -C $workspace -c core.quotepath=false ls-files --cached --others --exclude-standard | Sort-Object -Unique)
        if ($LASTEXITCODE -ne 0) { throw 'Source enumeration failed' }
        $inventory = @()
        foreach ($relative in $paths) {
            if ($relative -notmatch '^(src/|tools/|tests/|assets/|locales/|docs/|\.cargo/|Cargo\.toml$|Cargo\.lock$|build\.rs$|README\.md$|LICENSE[^/]*$)') { continue }
            if ($relative -match '(?i)(\.pk8$|\.pem$|\.key$|private[-_]key)') { throw 'Private key among source inputs' }
            $source = Assert-ChildPath "$workspace/$relative" $workspace
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { continue }
            foreach ($label in $buildLabels) {
                $destination = Assert-ChildPath "$case/source-$label/$relative" "$case/source-$label"
                [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
                [IO.File]::Copy($source,$destination,$false)
            }
            $inventory += @{path=$relative;sha256_source=(Hash $source)}
        }
        if ('A' -in $buildLabels) { Set-SnapshotVersion "$case/source-A" $VersionA }
        Set-SnapshotVersion "$case/source-B" $VersionB
        foreach ($item in $inventory) {
            foreach ($label in $buildLabels) { $item["sha256_$label"] = Hash "$case/source-$label/$($item.path)" }
        }
        Write-Json $inventory "$case/source-inventory.json"
        # Match the QA feature graph instead of compiling a second dependency set.
        Invoke-RecordedProcess (Get-Command cargo).Source @('build','--locked','--offline','--profile','qa','--features','gui,mcp,updates','--example','update-release') $workspace "$case/logs/verifier" 300
        $tool = "$case/tools/update-release.exe"
        Copy-Item -LiteralPath "$workspace/target/qa/examples/update-release.exe" -Destination $tool
        # Compile once in the already verified QA graph, before injecting release
        # options. Explicit source roots keep every build/package in its snapshot.
        Invoke-RecordedProcess (Get-Command cargo).Source @('build','--locked','--offline','--profile','qa','--features','gui,mcp,updates','--example','dev') $workspace "$case/logs/helper" 900
        foreach ($item in $inventory) { if ((Hash "$workspace/$($item.path)") -ne $item.sha256_source) { throw 'Helper source changed during compilation' } }
        $devHelper = "$case/tools/dev.exe"
        Copy-Item -LiteralPath "$workspace/target/qa/examples/dev.exe" -Destination $devHelper
        Write-Json @{sha256=(Hash $devHelper);source_commit=(git -C $workspace rev-parse HEAD).Trim();compiled_from=$workspace;features='gui,mcp,updates';profile='qa'} "$case/logs/helper-identity.json"
        if ($PublicKeys) {
            Copy-Item -LiteralPath (Assert-PlainPath $PublicKeys) -Destination "$case/keys.json"
            $signingKeyPath = Assert-PlainPath $SigningKey
        } else {
            $signingKeyPath = "$case/key.pk8"
            Invoke-RecordedProcess $tool @('keygen','--private-key',$signingKeyPath,'--public-keys',"$case/keys.json",'--key-id',$KeyId) $case "$case/logs/keygen" 15
        }
        $env:NOH_UPDATE_TRUST_JSON = Get-Content -LiteralPath "$case/keys.json" -Raw
        Invoke-RecordedProcess $tool @('check-signing-key','--private-key',$signingKeyPath,'--public-keys',"$case/keys.json",'--key-id',$KeyId) $case "$case/logs/check-key" 15
        $env:NOH_UPDATE_PACKAGE_ID = $id
        $env:NOH_UPDATE_CHANNEL = 'stable'
        $env:NOH_UPDATE_FORMAT = 'windows-setup'
        $env:NOH_UPDATE_FEED_URL = "https://github.com/$Repository/releases/latest/download/envelope.json"
        $env:NOH_LIBMPV = $mpvInput
        $env:NOH_NATIVE_NOTICES = if ($NativeNotices) { Assert-PlainPath $NativeNotices } else { "$runtime/licenses/native" }
        foreach ($label in $buildLabels) {
            $version = if ($label -eq 'A') {$VersionA} else {$VersionB}
            Write-Output "Building complete NOH $label"
            if ((Hash $devHelper) -ne (Get-Content "$case/logs/helper-identity.json" -Raw | ConvertFrom-Json).sha256) { throw 'Development helper changed after compilation' }
            Invoke-RecordedProcess $devHelper @('build','--source-root',"$case/source-$label",'--gui','--mcp','--updates','--offline','--ffmpeg',$ffmpegInput,'--speech',$speechInput) "$case/source-$label" "$case/logs/build-$label" 3500
            foreach ($item in $inventory) { if ((Hash "$case/source-$label/$($item.path)") -ne $item["sha256_$label"]) { throw 'Frozen source changed during build' } }
            $bundle = "$case/source-$label/dist/noh"
            $manifest = Get-Content -LiteralPath "$bundle/manifest.json" -Raw | ConvertFrom-Json
            if ($manifest.build.package_version -ne $version) { throw 'Bundle version mismatch' }
            foreach ($content in @('complete','standard','minimal')) {
                $suffix = if ($content -eq 'complete') { '' } else { "-$content" }
                $variant = "$label$suffix"
                $profileBundle = $bundle
                if ($content -ne 'complete') {
                    $profileBundle = "$case/payload-$variant"
                    Invoke-RecordedProcess $tool @('prepare-profile','--source',$bundle,'--output',$profileBundle,'--profile',$content) $case "$case/logs/profile-$variant" 180
                }
                Write-Output "Packing official Setup $variant from the shared $label binaries"
                $packages = "$case/packages-$variant"
                Invoke-RecordedProcess "$qualificationBase/dotnet8/dotnet.exe" @("$qualificationBase/vpk/tools/net8.0/any/vpk.dll",'pack','--packId',$id,'--packVersion',$version,'--packDir',$profileBundle,'--mainExe','noh.exe','--runtime','win-x64','--channel','win-x64-stable','--delta','None','--noPortable','--skipVeloAppCheck','--shortcuts','None','--outputDir',$packages) $case "$case/logs/pack-$variant" 900
                $setups = @(Get-ChildItem -LiteralPath $packages -Filter '*-Setup.exe')
                if ($setups.Count -ne 1) { throw 'Expected one profile Setup' }
                $setupName = "NOH-$version-win-x64$suffix-Setup.exe"
                Move-Item -LiteralPath $setups[0].FullName -Destination "$packages/$setupName"
                Invoke-RecordedProcess $tool @('describe-setup','--bundle',$profileBundle,'--profile',$content,'--setup',"$packages/$setupName",'--package-id',$id,'--version',$version,'--url',"https://github.com/$Repository/releases/download/$releaseTag/$setupName",'--output',"$case/release-$variant.json") $case "$case/logs/describe-$variant" 120
                Invoke-RecordedProcess $tool @('sign','--private-key',$signingKeyPath,'--key-id',$KeyId,'--release',"$case/release-$variant.json",'--packages',$packages,'--output',"$case/envelope-$variant.json") $case "$case/logs/sign-$variant" 120
            }
        }
        foreach ($label in $buildLabels) {
            Copy-Item -LiteralPath "$case/source-$label/dist/noh/bin/noh-update-repair.exe" -Destination "$case/tools/bootstrap-$label.exe"
        }
        $candidateFiles = @('keys.json','source-inventory.json','tools/update-release.exe') + @($buildLabels | ForEach-Object { "tools/bootstrap-$_.exe" })
        foreach ($label in $buildLabels) {
            foreach ($suffix in @('','-standard','-minimal')) {
                $variant = "$label$suffix"
                $release = Get-Content "$case/release-$variant.json" -Raw | ConvertFrom-Json
                $candidateFiles += @("release-$variant.json", "envelope-$variant.json", "packages-$variant/$($release.artifacts[0].file_name)")
            }
        }
        $catalog = @{schema=1;package_id=$id;repository=$Repository;release_tag=$releaseTag;version_a=$VersionA;version_b=$VersionB;source_commit=(git -C $workspace rev-parse HEAD).Trim()}
        if ($NewVersionOnly) {
            Copy-Item -LiteralPath $BaselineLock -Destination "$case/baseline.json"
            $candidateFiles += 'baseline.json'
            $catalog.baseline_sha256 = Hash "$case/baseline.json"
        }
        $catalog.files = @($candidateFiles | ForEach-Object { @{path=$_;sha256=(Hash "$case/$_")} })
        Write-Json $catalog "$case/candidate.json"
        $result.build_passed = $true
        if ($Stage -eq 'Build') { $result.passed = $true; return }
    }
    foreach ($key in @(Get-ChildItem Env: | Where-Object {$_.Name -like 'NOH_*'})) { Remove-Item -LiteralPath "Env:$($key.Name)" }
    $tool = "$inputCase/tools/update-release.exe"
    [IO.File]::WriteAllText("$userProfile/sentinel", 'preserve-user-data')
    [IO.File]::WriteAllText("$userProfile/noh/language", 'fr')
    Remove-Item -LiteralPath Env:NOH_LIBMPV -ErrorAction SilentlyContinue
    $env:APPDATA = $userProfile; $env:LOCALAPPDATA = $userProfile
    $env:TEMP = $userProfile; $env:TMP = $userProfile
    $suffix = if ($Profile -eq 'complete') { '' } else { "-$Profile" }
    $releaseB = Get-Content -LiteralPath "$inputCase/release-B$suffix.json" -Raw | ConvertFrom-Json
    $setupB = "$inputCase/packages-B$suffix/$($releaseB.artifacts[0].file_name)"
    $profileArguments = if ($Profile -eq 'complete') { @() } else { @('--profile',$Profile) }
    if ($previousCase) {
        $releaseA = Get-Content -LiteralPath "$previousCase/release-B$suffix.json" -Raw | ConvertFrom-Json
        $setupA = "$previousCase/packages-B$suffix/$($releaseA.artifacts[0].file_name)"
        $envelopeA = "$previousCase/envelope-B$suffix.json"
        $bootstrap = "$previousCase/tools/bootstrap-B.exe"
    } else {
        $releaseA = Get-Content -LiteralPath "$inputCase/release-A$suffix.json" -Raw | ConvertFrom-Json
        $setupA = "$inputCase/packages-A$suffix/$($releaseA.artifacts[0].file_name)"
        $envelopeA = "$inputCase/envelope-A$suffix.json"
        $bootstrap = "$inputCase/tools/bootstrap-A.exe"
    }
    Write-Output 'Installing A with the authenticated Setup controller'
    $initial = Invoke-RecordedProcess $bootstrap (@('setup','--base',$base,'--package',$setupA,'--envelope',$envelopeA,'--version',$VersionA,'--confirm-install','--initialize') + $profileArguments) $case "$case/logs/initial" 900 -PassOutput | ConvertFrom-Json
    Check-Identity 'A' $releaseA.artifacts[0].setup.build_fingerprint $VersionA
    # Every real entry point must refuse while the same external exclusive lease is held.
    $lock = [IO.File]::Open("$base/.noh-update/.noh-update-runtime.lock",'Open','ReadWrite','None')
    try {
        foreach ($relative in @('noh.exe','bin/noh-cli.exe','bin/noh-mcp.exe')) {
            $log = "$case/logs/excluded-" + $relative.Replace('/','-')
            try { Invoke-RecordedProcess "$root/current/$relative" @('--build-info') $case $log 15 } catch {}
            $record = Get-Content -LiteralPath "$log.process.json" -Raw | ConvertFrom-Json
            if ($record.exit -ne 1 -or (Get-Content -LiteralPath "$log.stderr.log" -Raw) -notmatch 'managed installation is unavailable') { throw "Client exclusion failed: $relative" }
        }
    } finally { $lock.Dispose() }
    $incoming = Invoke-RecordedProcess $tool (@('retain','--format','windows-setup','--public-keys',"$inputCase/keys.json",'--envelope',"$inputCase/envelope-B$suffix.json",'--package',$setupB,'--archive',"$base/import",'--package-id',$id,'--expected-version',$VersionB) + $profileArguments) $case "$case/logs/retain-B" 120 -PassOutput | ConvertFrom-Json
    $guardianPath = Join-Path $initial.outcome.recovery_tools 'noh-update-guard.exe'
    if ($previousCase) {
        $signedGuardian = @($releaseA.artifacts[0].setup.files | Where-Object path -eq 'bin/noh-update-guard.exe')
        $guardianHash = Hash $guardianPath
        if ($signedGuardian.Count -ne 1 -or $guardianHash -ne $signedGuardian[0].sha256) { throw 'Historical guardian differs from signed A' }
        $result.previous_guardian = @{path=$guardianPath;sha256=$guardianHash;build_fingerprint=$releaseA.artifacts[0].setup.build_fingerprint}
    }
    $app = "$root/current/noh.exe"
    $psi = [Diagnostics.ProcessStartInfo]::new($app)
    $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true; $psi.WorkingDirectory="$root/current"
    $psi.RedirectStandardOutput=$true; $psi.RedirectStandardError=$true
    $psi.ArgumentList.Add('--install-retained-update')
    $psi.ArgumentList.Add($incoming.retained_directory)
    $gui = [Diagnostics.Process]::Start($psi)
    $stdout=$gui.StandardOutput.ReadToEndAsync(); $stderr=$gui.StandardError.ReadToEndAsync()
    $deadline=[DateTime]::UtcNow.AddSeconds(300)
    while (-not $gui.WaitForExit(100)) {
        if (-not $guardian) {
            foreach ($candidate in @(Get-Process -Name noh-update-guard -ErrorAction SilentlyContinue)) {
                try {
                    [void]$candidate.SafeHandle
                    if ((NormalPath $candidate.MainModule.FileName) -eq (NormalPath $guardianPath)) { $guardian=$candidate; break }
                } catch {}
            }
        }
        if ([DateTime]::UtcNow -ge $deadline) { throw 'GUI A handoff deadline' }
    }
    [IO.File]::WriteAllText("$case/logs/gui-A.stdout.log", $stdout.GetAwaiter().GetResult())
    [IO.File]::WriteAllText("$case/logs/gui-A.stderr.log", $stderr.GetAwaiter().GetResult())
    if ($gui.ExitCode -ne 0 -or -not $guardian) { throw 'GUI handoff failed or live guardian was not observed' }
    if (-not $guardian.WaitForExit(660000) -or $guardian.ExitCode -ne 0) { throw 'Guardian did not complete successfully' }
    $transactions = @(Get-ChildItem -LiteralPath "$base/.noh-update/downloads" -Directory | Where-Object {Test-Path -LiteralPath "$($_.FullName)/transaction/completed"})
    if ($transactions.Count -ne 1) { throw 'Expected one completed GUI transaction' }
    $transaction = "$($transactions[0].FullName)/transaction"
    if ([IO.File]::ReadAllText("$transaction/completed") -ne 'result-flushed') { throw 'Invalid completion marker' }
    $update = Get-Content -LiteralPath "$transaction/result.json" -Raw | ConvertFrom-Json
    if (-not $update.success -or $update.outcome.version -ne $VersionB -or -not $update.restart.ready) { throw 'Installation or verified GUI restart failed' }
    $restarted = Get-Process -Id $update.restart.proof.pid
    [void]$restarted.SafeHandle
    if ((NormalPath $restarted.MainModule.FileName) -ne (NormalPath $app)) { throw 'Restarted GUI image mismatch' }
    if (-not $restarted.WaitForInputIdle(30000)) { throw 'Restarted GUI did not become input-idle' }
    if ($update.restart.proof.build_fingerprint -ne $releaseB.artifacts[0].setup.build_fingerprint) { throw 'Readiness generation mismatch' }
    Check-Identity 'B' $releaseB.artifacts[0].setup.build_fingerprint $VersionB
    $result.update = $update
    $result.gui_a_exit = $gui.ExitCode; $result.guardian_exit = $guardian.ExitCode
    # Stop only the identified disposable NOH GUI, then prove repair from its external tool.
    Stop-Owned $restarted $app
    $restarted.Dispose(); $restarted=$null
    $sourceRoot = Assert-ChildPath $root $case
    $savedRoot = Assert-ChildPath "$base/application.saved-by-qualification" $case
    if (Test-Path -LiteralPath $savedRoot) { throw 'Saved qualification root already exists' }
    Move-Item -LiteralPath $sourceRoot -Destination $savedRoot
    $externalRepair = Join-Path $update.outcome.recovery_tools 'noh-update-repair.exe'
    $repair = Invoke-RecordedProcess $externalRepair (@('setup','--base',$base,'--package',$setupB,'--envelope',"$inputCase/envelope-B$suffix.json",'--version',$VersionB,'--confirm-install') + $profileArguments) $case "$case/logs/repair-missing-root" 900 -PassOutput | ConvertFrom-Json
    Check-Identity 'repaired-B' $releaseB.artifacts[0].setup.build_fingerprint $VersionB
    foreach ($member in $releaseB.artifacts[0].setup.files) {
        $file = Get-Item -LiteralPath "$root/current/$($member.path)"
        if ($file.Length -ne $member.size -or (Hash $file.FullName) -ne $member.sha256) { throw "Installed inventory mismatch: $($member.path)" }
    }
    if ([IO.File]::ReadAllText("$userProfile/sentinel") -ne 'preserve-user-data' -or [IO.File]::ReadAllText("$userProfile/noh/language") -ne 'fr') { throw 'User data or settings changed' }
    $result.repair=$repair; $result.passed=$true
} catch {
    $result.error=$_.Exception.Message
    throw
} finally {
    try {
        if ($restarted) { Stop-Owned $restarted "$root/current/noh.exe" }
        if ($gui) { Stop-Owned $gui "$root/current/noh.exe" }
        if ($guardian) { Stop-Owned $guardian $guardianPath }
        if (Test-Path -LiteralPath $registry) {
            $registered=Get-ItemProperty -LiteralPath $registry
            if ((NormalPath $registered.InstallLocation) -ne (NormalPath $root)) { throw 'Refusing unrelated registry cleanup' }
            Remove-Item -LiteralPath $registry -Recurse
        }
        $result.registry_cleaned=-not (Test-Path -LiteralPath $registry)
    } catch { $result.cleanup_error=$_.Exception.Message; $result.passed=$false; throw }
    finally {
        Write-Json $result "$case/result.json"
        Write-Output "Full client qualification: $case/result.json"
    }
}
