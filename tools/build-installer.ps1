# Build a thin wizard from authenticated CI outputs. Never builds the application.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$CandidateDirectory,
    [Parameter(Mandatory)][string]$ExpectedCandidateSha256,
    [Parameter(Mandatory)][string]$ExpectedCommit,
    [Parameter(Mandatory)][string]$PublicKeys,
    [Parameter(Mandatory)][string]$CompilerDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [ValidateSet('web','offline')][string]$Mode = 'web',
    [ValidateSet('minimal','standard','complete')][string]$Profile = 'complete',
    [string]$DownloadSources
)
. "$PSScriptRoot/update-qualification/common.ps1"
$candidateRoot = Assert-PlainPath $CandidateDirectory
$outputRoot = Assert-PlainPath $OutputDirectory
$compilerRoot = Assert-PlainPath $CompilerDirectory
$metadataPath = Join-Path $candidateRoot 'candidate.json'
$metadataFile = [IO.File]::Open($metadataPath, 'Open', 'Read', 'Read')
try {
    if ($metadataFile.Length -gt 1MB) { throw 'Candidate catalog exceeds limit' }
    $metadataBytes = [byte[]]::new($metadataFile.Length)
    $metadataFile.ReadExactly($metadataBytes)
} finally { $metadataFile.Dispose() }
if ($ExpectedCandidateSha256 -notmatch '^[a-f0-9]{64}$' -or
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($metadataBytes)).ToLowerInvariant() -ne $ExpectedCandidateSha256) { throw 'Candidate catalog does not match independent provenance' }
$catalog = [Text.Encoding]::UTF8.GetString($metadataBytes) | ConvertFrom-Json
if ($catalog.schema -ne 1 -or $catalog.source_commit -ne $ExpectedCommit -or
    $ExpectedCommit -notmatch '^[a-f0-9]{40}$' -or $catalog.package_id -ne 'NOH' -or
    $catalog.repository -ne 'Awalgawe/noh' -or $catalog.version_b -notmatch '^\d+\.\d+\.\d+$') { throw 'Unexpected candidate identity' }
$version = $catalog.version_b
$held = [Collections.Generic.List[IDisposable]]::new()
function CandidateFile([string]$Relative) {
    $members = @($catalog.files | Where-Object path -eq $Relative)
    if ($members.Count -ne 1) { throw "Missing or ambiguous candidate member: $Relative" }
    $path = Assert-ChildPath "$candidateRoot/$Relative" $candidateRoot
    $file = [IO.File]::Open($path, 'Open', 'Read', 'Read')
    $held.Add($file)
    if ([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($file)).ToLowerInvariant() -ne $members[0].sha256) { throw "Candidate member mismatch: $Relative" }
    return $path
}
function IssString([string]$Value) {
    if ($Value.Contains('"') -or $Value.Contains("`r") -or $Value.Contains("`n")) { throw 'Unsafe installer source value' }
    return '"' + $Value + '"'
}
function PascalString([string]$Value) { return "'" + $Value.Replace("'", "''") + "'" }

try {
$verifier = CandidateFile 'tools/update-release.exe'
$bootstrap = CandidateFile 'tools/bootstrap-B.exe'
$bootstrapHash = @($catalog.files | Where-Object path -eq 'tools/bootstrap-B.exe')[0].sha256
$keys = Assert-PlainPath $PublicKeys
$compiler = Join-Path $compilerRoot 'ISCC.exe'
$compilerVersion = '7.1.0'
# These files come from the official installer pinned to
# 0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f.
$compilerPins = @{
    'ISCC.exe' = 'd06ebd38f38e3cee60a3c50cc45bd449d77e0bc6a5cabc607ea9886808e4de1a'
    'ISPP.dll' = 'f875ddf920f17dceaaad05280dafd6d5376a1a4111cbd5fe97bfc47c286b5a41'
}
foreach ($name in $compilerPins.Keys) {
    if ((Get-FileHash -LiteralPath "$compilerRoot/$name").Hash.ToLowerInvariant() -ne $compilerPins[$name]) { throw "Inno Setup compiler pin mismatch: $name" }
}
if ((Get-AuthenticodeSignature -FilePath $compiler).Status -ne 'Valid') { throw 'Compiler Authenticode verification failed' }
[void][IO.Directory]::CreateDirectory($outputRoot)
$build = Join-Path $outputRoot ('source-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($build)
Copy-Item -LiteralPath "$PSScriptRoot/installer/noh.iss","$PSScriptRoot/installer/messages.iss" -Destination $build
Copy-Item -LiteralPath "$workspace/assets/noh-icon.ico" -Destination "$build/noh.ico"
Copy-Item -LiteralPath $PSCommandPath -Destination "$build/build-installer.ps1"
Copy-Item -LiteralPath "$PSScriptRoot/update-qualification/common.ps1" -Destination "$build/common.ps1"
$wrapperCommit = (& git -C $workspace rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify wrapper source commit' }
$wrapperChanges = @(& git -C $workspace status --porcelain -- tools/installer tools/build-installer.ps1 tools/update-qualification/common.ps1 assets/noh-icon.ico)
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify wrapper working changes' }
$profiles = if ($Mode -eq 'web') { @('minimal','standard','complete') } else { @($Profile) }
$mirrors = @{}
if ($DownloadSources) {
    if ($Mode -ne 'web') { throw 'Download sources apply only to web installers' }
    $mirrors = Get-Content -LiteralPath (Assert-PlainPath $DownloadSources) -Raw | ConvertFrom-Json -AsHashtable
    foreach ($key in $mirrors.Keys) {
        if ($key -notin $profiles -or $mirrors[$key] -notmatch '^https://api\.github\.com/repos/Awalgawe/noh/releases/assets/[0-9]+$') { throw 'Only same-repository asset API URLs are permitted; credentials must remain runtime-only' }
    }
}
$outputName = if ($Mode -eq 'web') { "NOH-$version-win-x64-Install" } else { "NOH-$version-win-x64-$Profile-Offline" }
if (Test-Path -LiteralPath "$outputRoot/$outputName.exe") { throw 'Installer output already exists' }
$offline = if ($Mode -eq 'offline') { $Profile } else { '' }
$lines = [Collections.Generic.List[string]]::new()
$lines.Add('#define NohVersion ' + (IssString $version))
$lines.Add('#define OutputDirectory ' + (IssString $outputRoot))
$lines.Add('#define OutputName ' + (IssString $outputName))
$lines.Add('#define OfflineProfile ' + (IssString $offline))
$lines.Add('#define NohIcon ' + (IssString "$build/noh.ico"))
$lines.Add('[Files]')
$lines.Add('Source: ' + (IssString $bootstrap) + '; DestName: "bootstrap.exe"; Hash: "' + $bootstrapHash + '"; Flags: dontcopy')
$records = @()
foreach ($content in $profiles) {
    $suffix = if ($content -eq 'complete') { '' } else { "-$content" }
    $releasePath = CandidateFile "release-B$suffix.json"
    $envelope = CandidateFile "envelope-B$suffix.json"
    $release = Get-Content -LiteralPath $releasePath -Raw | ConvertFrom-Json -AsHashtable
    $declaredProfile = if ($release.ContainsKey('profile')) { $release.profile } else { 'complete' }
    if ($release.package_id -ne 'NOH' -or $release.version -ne $version -or $declaredProfile -ne $content -or $release.artifacts.Count -ne 1) { throw 'Profile descriptor mismatch' }
    $artifact = $release.artifacts[0]
    $package = Assert-ChildPath "$candidateRoot/packages-B$suffix/$($artifact.file_name)" $candidateRoot
    Invoke-RecordedProcess $verifier @('verify','--format','windows-setup','--profile',$content,'--public-keys',$keys,'--envelope',$envelope,'--package',$package,'--package-id','NOH','--expected-version',$version) $workspace "$build/verify-$content" 180
    # Bind the parsed producer descriptor to the authenticated envelope, not only CI.
    $signed = Get-Content -LiteralPath $envelope -Raw | ConvertFrom-Json
    $payload = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($signed.payload)) | ConvertFrom-Json -AsHashtable
    if (($payload | ConvertTo-Json -Depth 20 -Compress) -ne ($release | ConvertTo-Json -Depth 20 -Compress)) { throw 'Unsigned descriptor differs from the verified payload' }
    $member = @($artifact.setup.files | Where-Object path -eq 'bin/noh-update-repair.exe')
    if ($member.Count -ne 1 -or (Get-FileHash -LiteralPath $bootstrap).Hash.ToLowerInvariant() -ne $member[0].sha256) { throw 'Bootstrap differs from the signed application controller' }
    $envelopeHash = @($catalog.files | Where-Object path -eq "envelope-B$suffix.json")[0].sha256
    $lines.Add('Source: ' + (IssString $envelope) + '; DestName: "envelope-' + $content + '.json"; Hash: "' + $envelopeHash + '"; Flags: dontcopy')
    if ($Mode -eq 'offline') { $lines.Add('Source: ' + (IssString $package) + '; Hash: "' + $artifact.sha256 + '"; Flags: dontcopy') }
    $records += @{profile=$content;package=$artifact.file_name;size=$artifact.size;sha256=$artifact.sha256;installed_bytes=$artifact.setup.installed_bytes;envelope_sha256=$envelopeHash;source=$mirrors[$content]}
}
$lines.Add('[Code]')
foreach ($function in @('PackageName','DownloadSource')) {
    $lines.Add("function ${function}(const Content: String): String;")
    $lines.Add('begin')
    $lines.Add("  Result := '';")
    foreach ($record in $records) {
        $value = if ($function -eq 'PackageName') { $record.package } else { [string]$record.source }
        $lines.Add('  if Content = ' + (PascalString $record.profile) + ' then Result := ' + (PascalString $value) + ';')
    }
    $lines.Add('end;')
}
foreach ($field in @('size','installed_bytes')) {
    $function = if ($field -eq 'size') { 'PackageBytes' } else { 'InstalledBytes' }
    $lines.Add("function ${function}(const Content: String): Int64;")
    $lines.Add('begin')
    $lines.Add('  Result := 0;')
    foreach ($record in $records) {
        $lines.Add('  if Content = ' + (PascalString $record.profile) + ' then Result := ' + [string]$record[$field] + ';')
    }
    $lines.Add('end;')
}
[IO.File]::WriteAllLines("$build/payload.iss", $lines, [Text.UTF8Encoding]::new($true))
Write-Json @{mode=$Mode;offline_profile=$offline;mirrors=$mirrors;candidate_sha256=$ExpectedCandidateSha256;public_keys_sha256=(Get-FileHash -LiteralPath $keys).Hash.ToLowerInvariant();compiler_pins=$compilerPins} "$build/parameters.json"
$sources = @('noh.iss','messages.iss','payload.iss','build-installer.ps1','common.ps1','noh.ico','parameters.json') | ForEach-Object {
    $file = [IO.File]::Open("$build/$_", 'Open', 'Read', 'Read')
    $held.Add($file)
    @{path=$_;sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($file)).ToLowerInvariant()}
}
Invoke-RecordedProcess $compiler @('/Qp',"$build/noh.iss") $workspace "$build/compiler" 300
$output = "$outputRoot/$outputName.exe"
Write-Json @{schema=1;application_commit=$ExpectedCommit;wrapper_commit=$wrapperCommit;wrapper_dirty=($wrapperChanges.Count -gt 0);wrapper_changes=$wrapperChanges;sources_directory=$build;sources=$sources;candidate_sha256=$ExpectedCandidateSha256;version=$version;mode=$Mode;profiles=$records;compiler_version=$compilerVersion;compiler_sha256=(Get-FileHash -LiteralPath $compiler).Hash.ToLowerInvariant();path=$output;sha256=(Get-FileHash -LiteralPath $output).Hash.ToLowerInvariant();size=(Get-Item -LiteralPath $output).Length;public_distribution=$false} "$output.json"
Write-Output "Installer built: $output"
} finally {
    foreach ($file in $held) { $file.Dispose() }
}
