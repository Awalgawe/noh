# Prepare exact accepted CI bytes as a private GitHub draft. Never publish it.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$CandidateDirectory,
    [Parameter(Mandatory)][string]$CandidateArchive,
    [Parameter(Mandatory)][string]$AcceptanceReport,
    [string[]]$AdditionalAcceptanceReports = @(),
    [Parameter(Mandatory)][string]$PublicKeys,
    [Parameter(Mandatory)][long]$RunId,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedCommit,
    [switch]$CreateDraft
)
. "$PSScriptRoot/update-qualification/common.ps1"
$candidateRoot = Assert-PlainPath $CandidateDirectory
$archive = Assert-PlainPath $CandidateArchive
$catalogBytes = [IO.File]::ReadAllBytes("$candidateRoot/candidate.json")
$record = [Text.Encoding]::UTF8.GetString($catalogBytes) | ConvertFrom-Json
$acceptance = Get-Content (Assert-PlainPath $AcceptanceReport) -Raw | ConvertFrom-Json
$candidateHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($catalogBytes)).ToLowerInvariant()
if ($record.schema -ne 1 -or $record.repository -ne 'Awalgawe/noh' -or $record.package_id -ne 'NOH' -or $record.source_commit -ne $ExpectedCommit) { throw 'Unexpected candidate identity or commit' }
if (-not $acceptance.passed -or -not $acceptance.registry_cleaned -or $acceptance.source_commit -ne $ExpectedCommit -or $acceptance.candidate_sha256 -ne $candidateHash -or -not $acceptance.update.restart.ready) { throw 'Exact candidate native acceptance is missing' }
$profiles = if ('release-B-standard.json' -in $record.files.path) { @('complete','standard','minimal') } else { @('complete') }
if ($profiles.Count -gt 1) {
    $reports = @($acceptance) + @($AdditionalAcceptanceReports | ForEach-Object { Get-Content -LiteralPath (Assert-PlainPath $_) -Raw | ConvertFrom-Json })
    foreach ($profile in $profiles) {
        $matching = @($reports | Where-Object { $_.profile -eq $profile })
        if ($matching.Count -ne 1 -or -not $matching[0].passed -or -not $matching[0].registry_cleaned -or
            $matching[0].source_commit -ne $ExpectedCommit -or $matching[0].candidate_sha256 -ne $candidateHash -or
            -not $matching[0].update.restart.ready) { throw "Exact native acceptance is missing for profile $profile" }
    }
}
$env:GIT_TERMINAL_PROMPT='0'; $env:GCM_INTERACTIVE='Never'
$credential = "protocol=https`nhost=github.com`npath=Awalgawe/noh.git`n`n" | git credential fill
if ($LASTEXITCODE -ne 0) { throw 'Existing GitHub authentication unavailable' }
$password = @($credential | Where-Object { $_.StartsWith('password=') })
if ($password.Count -ne 1) { throw 'GitHub authentication incomplete' }
$headers = @{Authorization=('Bearer '+$password[0].Substring(9));Accept='application/vnd.github+json';'X-GitHub-Api-Version'='2022-11-28';'User-Agent'='NOH-Setup-draft'}
$api = 'https://api.github.com/repos/Awalgawe/noh'
$repository = Invoke-RestMethod $api -Headers $headers
$run = Invoke-RestMethod "$api/actions/runs/$RunId" -Headers $headers
if (-not $repository.private -or $run.repository.full_name -ne 'Awalgawe/noh' -or $run.head_repository.full_name -ne 'Awalgawe/noh' -or $run.head_sha -ne $ExpectedCommit -or $run.path -ne '.github/workflows/windows-setup.yml' -or $run.status -ne 'completed' -or $run.conclusion -ne 'success') { throw 'A successful reviewed private Setup workflow is required' }
$artifacts = Invoke-RestMethod "$api/actions/runs/$RunId/artifacts?per_page=100" -Headers $headers
$artifactMatches = @($artifacts.artifacts | Where-Object { $_.name -eq 'noh-windows-setup-candidate' -and -not $_.expired })
if ($artifactMatches.Count -ne 1 -or $artifactMatches[0].digest -ne ('sha256:'+(Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant())) { throw 'Downloaded archive does not match the authoritative CI artifact digest' }
$zip = [IO.Compression.ZipFile]::OpenRead($archive)
$held = [Collections.Generic.List[IDisposable]]::new()
try {
    $files = @($record.files) + @([pscustomobject]@{path='candidate.json';sha256=$candidateHash})
    if ($zip.Entries.Count -ne $files.Count) { throw 'Unexpected artifact members' }
    foreach ($item in $files) {
        $path = Assert-ChildPath "$candidateRoot/$($item.path)" $candidateRoot
        $entry = $zip.GetEntry($item.path)
        if (-not $entry) { throw "Missing original artifact member: $($item.path)" }
        $stream = $entry.Open()
        try { $original = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() } finally { $stream.Dispose() }
        $file = [IO.File]::Open($path,'Open','Read','Read')
        $held.Add($file)
        $actual = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($file)).ToLowerInvariant()
        if ($original -ne $item.sha256 -or $actual -ne $original) { throw "Changed artifact member: $($item.path)" }
    }
    # Use independently configured trust, never keys chosen by the downloaded feed.
    $assets = @()
    foreach ($profile in $profiles) {
        $suffix = if ($profile -eq 'complete') { '' } else { "-$profile" }
        $release = Get-Content "$candidateRoot/release-B$suffix.json" -Raw | ConvertFrom-Json
        if ($release.version -ne $record.version_b -or $release.artifacts.Count -ne 1 -or $release.artifacts[0].url -ne "https://github.com/Awalgawe/noh/releases/download/$($record.release_tag)/$($release.artifacts[0].file_name)") { throw 'Release asset URL or version mismatch' }
        $setup = Assert-ChildPath "$candidateRoot/packages-B$suffix/$($release.artifacts[0].file_name)" $candidateRoot
        $profileArgs = if ($profile -eq 'complete') { @() } else { @('--profile',$profile) }
        & "$candidateRoot/tools/update-release.exe" verify --format windows-setup @profileArgs --public-keys (Assert-PlainPath $PublicKeys) --envelope "$candidateRoot/envelope-B$suffix.json" --package $setup --package-id NOH --expected-version $record.version_b
        if ($LASTEXITCODE -ne 0) { throw 'Independent Setup authentication failed' }
        $assets += @(@{path=$setup;name=$release.artifacts[0].file_name;profile=$profile;payload=$true},
            @{path="$candidateRoot/envelope-B$suffix.json";name="envelope$suffix.json";profile=$profile;payload=$false})
    }
    if ($profiles.Count -eq 1) { $assets += @{path="$candidateRoot/tools/bootstrap-B.exe";name='NOH-Install.exe';payload=$false} }
    $body = "Windows $($record.version_b) candidate from $ExpectedCommit. CI run $RunId and native installation/update/restart/missing-root repair passed for these exact bytes. This remains a draft; public redistribution and code-signing clearance are not asserted. Initial installation uses NOH-Install.exe setup with the accompanying signed envelope and full Setup. See docs/UPDATE_RELEASE.md at the source commit."
    if ($profiles.Count -gt 1) { $body = "Windows $($record.version_b) signed profile payloads from $ExpectedCommit. CI run $RunId and native installation/update/restart/missing-root repair passed for Minimal, Standard and Complete. Double-click installer qualification is pending; do not distribute this draft. No public redistribution or code-signing clearance is asserted." }
    $plan = @{source_commit=$ExpectedCommit;run_id=$RunId;tag=$record.release_tag;draft=$true;assets=@($assets | ForEach-Object { @{name=$_.name;sha256=(Get-FileHash -LiteralPath $_.path).Hash.ToLowerInvariant()} });notes=$body}
    Write-Json $plan "$candidateRoot/draft-plan.json"
    if (-not $CreateDraft) { Write-Output 'Verified draft plan prepared; no release created'; return }
    $existing = Invoke-RestMethod "$api/releases?per_page=100" -Headers $headers
    if (@($existing | Where-Object { $_.tag_name -eq $record.release_tag }).Count) { throw 'Release tag already exists; refusing replacement' }
    $draft = Invoke-RestMethod -Method Post "$api/releases" -Headers $headers -ContentType 'application/json' -Body (@{tag_name=$record.release_tag;target_commitish=$ExpectedCommit;name="NOH $($record.version_b) Windows candidate";body=$body;draft=$true;prerelease=$false} | ConvertTo-Json)
    # Persist the ID before any upload so an interruption never invites a duplicate.
    Write-Json @{id=$draft.id;url=$draft.html_url;complete=$false;plan=$plan} "$candidateRoot/draft-result.json"
    $downloadSources = @{}
    foreach ($asset in $assets) {
        $url = "https://uploads.github.com/repos/Awalgawe/noh/releases/$($draft.id)/assets?name=$([Uri]::EscapeDataString($asset.name))"
        $uploaded = Invoke-RestMethod -Method Post $url -Headers $headers -ContentType 'application/octet-stream' -InFile $asset.path
        if ($uploaded.state -ne 'uploaded' -or $uploaded.size -ne (Get-Item -LiteralPath $asset.path).Length -or
            $uploaded.digest -ne ('sha256:' + (Get-FileHash -LiteralPath $asset.path).Hash.ToLowerInvariant())) { throw 'Draft asset upload is incomplete or changed' }
        if ($asset.payload) { $downloadSources[$asset.profile] = $uploaded.url }
    }
    $verified = Invoke-RestMethod "$api/releases/$($draft.id)" -Headers $headers
    if (-not $verified.draft -or $verified.assets.Count -ne $assets.Count) { throw 'Final draft verification failed' }
    Write-Json @{id=$draft.id;url=$draft.html_url;complete=$true;plan=$plan} "$candidateRoot/draft-result.json"
    if ($profiles.Count -gt 1) { Write-Json $downloadSources "$candidateRoot/installer-download-sources.json" }
    Write-Output "Private draft prepared: $($draft.html_url)"
} finally {
    foreach ($file in $held) { $file.Dispose() }
    $zip.Dispose()
    $headers=$null; $password=$null; $credential=$null
}
