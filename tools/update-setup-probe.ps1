# C1: disposable ordinary installation and missing-current repair by official Setup.
# Local synthetic data only. No download, publication or production updater enablement.
[CmdletBinding()]
param([ValidateSet('C1','C2a','C2s-permissions','C2s-version','C2s-interruption')][string]$Stage = 'C1')
. "$PSScriptRoot/update-qualification/common.ps1"
if (-not $IsWindows) { throw 'PowerShell 7 on Windows is required' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this probe without elevation' }
$base = Assert-ChildPath "$workspace/.mcp-dev/update-setup-probe" $workspace
$case = Assert-ChildPath "$base/case-$([Guid]::NewGuid().ToString('N'))" $base
$id = 'NohSetupProbe' + [Guid]::NewGuid().ToString('N')
$registry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$id"
if (Test-Path -LiteralPath $registry) { throw 'Disposable package identity already exists' }
foreach ($name in @('logs','tools','payload/models','packages','coordination','installation','fault-saved','userdata','temp')) {
    [void][IO.Directory]::CreateDirectory((Assert-ChildPath "$case/$name" $case))
}
$result = [ordered]@{schema_version=1;scope="$Stage; synthetic clients";case=$case;package_id=$id;status='failed';registry_cleaned=$false}
$timer = [Diagnostics.Stopwatch]::StartNew()
$client = $null
$guardian = $null
$originalCaseAcl = $null
$permissionSetup = $null
$permissionConsumersStopped = $false
$renameFault = $null
function Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Invoke-Probe([string]$Attempt, [string]$Setup, [string]$ExpectedError = '', [switch]$WithoutConsent, [string]$Envelope = "$case/envelope.json", [string]$Version = '1.0.0') {
    $arguments = @('--case',$case,'--setup',$Setup,'--keys',"$case/keys.json",'--envelope',$Envelope,'--package-id',$id,'--attempt',$Attempt,'--version',$Version)
    if (-not $WithoutConsent) { $arguments += '--fixture-consent' }
    try {
        Invoke-RecordedProcess "$case/tools/update-setup-probe.exe" $arguments $case "$case/logs/$Attempt" 90
    } catch {
        if (-not $ExpectedError) { throw }
    }
    $report = Get-Content -LiteralPath "$case/$Attempt.json" -Raw | ConvertFrom-Json
    if ($ExpectedError) {
        if ($report.status -ne 'failed' -or $report.setup_started -or $report.error -notmatch $ExpectedError) {
            throw "Unexpected negative result: $Attempt"
        }
    } elseif ($report.status -ne 'passed') { throw "Probe failed: $Attempt" }
}
try {
    $archivePath = Assert-PlainPath "$qualificationBase/vpk.1.2.161.zip"
    $archiveHash = Hash $archivePath
    if ($archiveHash -ne '2b56ce117f803fc70c103cb423bd040e395e40370f6ff6e10818e9ff9c26a323') { throw 'Unexpected Velopack package' }
    # Check the extracted tool closure against the independently pinned release archive.
    $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
    $toolInventory = @()
    try {
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName.EndsWith('/') -or $entry.FullName -notmatch '^(tools/net8.0/any/|vendor/)') { continue }
            $path = Assert-ChildPath "$qualificationBase/vpk/$($entry.FullName)" "$qualificationBase/vpk"
            $stream = $entry.Open()
            try { $expected = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() }
            finally { $stream.Dispose() }
            if ((Hash $path) -ne $expected) { throw "Extracted Velopack file differs: $($entry.FullName)" }
            $toolInventory += @{path=$entry.FullName;sha256=$expected}
        }
    } finally { $archive.Dispose() }
    if ($toolInventory.Count -lt 5) { throw 'Incomplete packager inventory' }
    Write-Json $toolInventory "$case/packager-inventory.json"
    $result.velopack_archive_sha256 = $archiveHash
    $env:CC = Assert-PlainPath "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin/clang.exe"
    $env:AR = Assert-PlainPath "$qualificationBase/llvm-mingw-20260922-ucrt-x86_64/bin/llvm-ar.exe"
    Invoke-RecordedProcess (Get-Command cargo).Source @('build','--locked','--offline','--profile','qa','--features','updates','--example','update-setup-probe','--example','update-release') $workspace "$case/logs/build" 300
    foreach ($tool in @('update-release','update-setup-probe')) {
        Copy-Item -LiteralPath "$workspace/target/qa/examples/$tool.exe" -Destination "$case/tools/$tool.exe"
    }
    $sourcePaths = @('Cargo.toml','Cargo.lock','tools/update_setup_probe.rs','tools/update-setup-probe.ps1','tools/update-setup-probe/fixture.rs','tools/update-setup-probe/RootRenameFault.cs','tools/update-qualification/common.ps1','src/update/windows.rs')
    Write-Json @($sourcePaths | ForEach-Object { @{path=$_;sha256=(Hash "$workspace/$_")} }) "$case/source-inventory.json"
    foreach ($source in $sourcePaths) {
        $destination = Assert-ChildPath "$case/sources/$source" $case
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
        [IO.File]::Copy("$workspace/$source", $destination, $false)
    }
    $result.git_revision = (& git -C $workspace rev-parse HEAD).Trim()
    $result.probe_sha256 = Hash "$case/tools/update-setup-probe.exe"
    $result.verifier_sha256 = Hash "$case/tools/update-release.exe"
    Set-Content -LiteralPath "$case/coordination/identity" -Value $id -Encoding utf8NoBOM
    [IO.File]::WriteAllText("$case/coordination/.noh-update-runtime.lock", '')
    $env:NOH_SETUP_PROBE_ANCHOR = "$case/coordination"
    $env:NOH_SETUP_PROBE_VERSION = '1.0.0'
    Invoke-RecordedProcess (Get-Command rustc).Source @("$PSScriptRoot/update-setup-probe/fixture.rs",'--edition','2021','-C','linker=rust-lld','-C','linker-flavor=ld.lld','-o',"$case/fixture.exe") $workspace "$case/logs/fixture" 60
    Copy-Item -LiteralPath "$case/fixture.exe" -Destination "$case/payload/fixture.exe"
    [IO.File]::WriteAllBytes("$case/payload/models/model.bin", [Text.Encoding]::UTF8.GetBytes('synthetic offline model'))
    [IO.File]::WriteAllText("$case/userdata/project.txt", 'synthetic user project outside installation')
    $dataHash = Hash "$case/userdata/project.txt"
    $dotnet = Assert-PlainPath "$qualificationBase/dotnet8/dotnet.exe"
    $packager = Assert-PlainPath "$qualificationBase/vpk/tools/net8.0/any/vpk.dll"
    Invoke-RecordedProcess $dotnet @($packager,'pack','--packId',$id,'--packTitle','NOH disposable Setup probe','--packVersion','1.0.0','--packDir',"$case/payload",'--mainExe','fixture.exe','--runtime','win-x64','--channel','win-x64-stable','--delta','None','--noPortable','--skipVeloAppCheck','--shortcuts','None','--outputDir',"$case/packages") $case "$case/logs/pack" 120
    $setups = @(Get-ChildItem -LiteralPath "$case/packages" -Filter '*Setup.exe')
    if ($setups.Count -ne 1) { throw 'Expected exactly one Setup artifact' }
    $setup = $setups[0]
    $result.setup_sha256 = Hash $setup.FullName
    $result.fixture_sha256 = Hash "$case/payload/fixture.exe"
    $result.model_sha256 = Hash "$case/payload/models/model.bin"
    $release = @{schema_version=1;package_id=$id;version='1.0.0';channel='stable';notes='Local C1 fixture';artifacts=@(@{target=@{os='windows';arch='x86_64'};kind='full';base_version=$null;file_name=$setup.Name;url="https://github.com/example/noh/releases/download/fixture/$($setup.Name)";size=$setup.Length;sha256=$result.setup_sha256})}
    Write-Json @{format='noh-setup-probe-v1';release=$release} "$case/release.json"
    Invoke-RecordedProcess "$case/tools/update-release.exe" @('keygen','--private-key',"$case/test-key.pk8",'--public-keys',"$case/keys.json",'--key-id','local-setup-probe') $case "$case/logs/keygen" 10
    Invoke-RecordedProcess "$case/tools/update-setup-probe.exe" @('--case',$case,'--setup',$setup.FullName,'--envelope',"$case/envelope.json",'--keys',"$case/keys.json",'--package-id',$id,'--attempt','sign','--fixture-consent','--sign-fixture-key',"$case/test-key.pk8") $case "$case/logs/sign" 10
    Invoke-Probe 'no-consent' $setup.FullName 'Fixture consent missing' -WithoutConsent
    $badEnvelope = Get-Content -LiteralPath "$case/envelope.json" -Raw | ConvertFrom-Json
    $signature = [Convert]::FromBase64String($badEnvelope.signature)
    $signature[0] = $signature[0] -bxor 1
    $badEnvelope.signature = [Convert]::ToBase64String($signature)
    Write-Json $badEnvelope "$case/bad-envelope.json"
    Invoke-Probe 'bad-signature' $setup.FullName 'Invalid fixture signature' -Envelope "$case/bad-envelope.json"
    [void][IO.Directory]::CreateDirectory("$case/tampered")
    $tampered = "$case/tampered/$($setup.Name)"
    Copy-Item -LiteralPath $setup.FullName -Destination $tampered
    $file = [IO.File]::Open($tampered, 'Open', 'ReadWrite', 'None')
    try { $byte=$file.ReadByte(); $file.Position=0; $file.WriteByte($byte -bxor 1) } finally { $file.Dispose() }
    Invoke-Probe 'tampered' $tampered 'size or digest'
    Invoke-Probe 'initial' $setup.FullName
    $info = Get-ItemProperty -LiteralPath $registry
    if ($info.DisplayVersion -ne '1.0.0' -or (Test-Path "$case/installation/.portable")) { throw 'Ordinary installed fixture not established' }
    $result.ordinary_installation = $true
    $psi = [Diagnostics.ProcessStartInfo]::new("$case/installation/current/fixture.exe")
    $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true
    foreach ($argument in @('--hold',"$case/client-ready","$case/client-stop")) { $psi.ArgumentList.Add($argument) }
    $client = [Diagnostics.Process]::Start($psi)
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while (-not (Test-Path "$case/client-ready")) {
        if ($client.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Client did not acquire its activity lease' }
        Start-Sleep -Milliseconds 20
    }
    $before = Hash "$case/installation/current/fixture.exe"
    Invoke-Probe 'active-client' $setup.FullName 'os error 32'
    if ($client.HasExited -or (Hash "$case/installation/current/fixture.exe") -ne $before) { throw 'Active client was disturbed' }
    [IO.File]::WriteAllText("$case/client-stop", 'stop')
    if (-not $client.WaitForExit(5000) -or $client.ExitCode -ne 0) { throw 'Client did not exit cleanly' }
    $client.Dispose(); $client=$null
    # Inject damage by moving only the verified disposable current directory. Retain evidence.
    $oldCurrent = Assert-ChildPath "$case/installation/current" $case
    $savedCurrent = Assert-ChildPath "$case/fault-saved/current" $case
    Move-Item -LiteralPath $oldCurrent -Destination $savedCurrent
    if (Test-Path -LiteralPath $oldCurrent) { throw 'Missing-current fault was not established' }
    $result.missing_current_established = $true
    Invoke-Probe 'repair-missing-current' $setup.FullName
    $result.c1_passed = $true
    if ($Stage -in @('C2s-permissions','C2s-version','C2s-interruption')) {
        $beforeFiles = @{}
        foreach ($name in @('current/fixture.exe','current/models/model.bin','Update.exe')) {
            $beforeFiles[$name] = Hash "$case/installation/$name"
        }
        $psi = [Diagnostics.ProcessStartInfo]::new("$case/tools/update-setup-probe.exe")
        $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true
        foreach ($argument in @('--case',$case,'--setup',$setup.FullName,'--keys',"$case/keys.json",'--envelope',"$case/envelope.json",'--package-id',$id,'--attempt','changed-permissions','--fixture-consent','--pause-before-engine')) { $psi.ArgumentList.Add($argument) }
        $guardian = [Diagnostics.Process]::Start($psi)
        $checkpointPath = "$case/hook-changed-permissions/before-engine.json"
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while (-not (Test-Path -LiteralPath $checkpointPath)) {
            if ($guardian.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Pre-engine rendezvous did not complete' }
            Start-Sleep -Milliseconds 20
        }
        $checkpoint = Get-Content -LiteralPath $checkpointPath -Raw | ConvertFrom-Json
        if ($checkpoint.setup_started -or -not $checkpoint.prelaunch_parent_rename_denied) { throw 'Permission fault was not placed after protection' }
        # Only the disposable parent DACL changes; no inheritance and no user directory ACL.
        # Denying new subdirectories prevents Setup from renaming/recreating installation.
        $originalCaseAcl = Get-Acl -LiteralPath $case
        $changedAcl = Get-Acl -LiteralPath $case
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $rights = [Security.AccessControl.FileSystemRights]::CreateDirectories -bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($identity, $rights, [Security.AccessControl.InheritanceFlags]::None, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Deny)
        $changedAcl.AddAccessRule($rule)
        Set-Acl -LiteralPath $case -AclObject $changedAcl
        Write-Json @{original=$originalCaseAcl.Sddl;changed=(Get-Acl -LiteralPath $case).Sddl;scope=$case;rights=$rights.ToString();identity=$identity.Value} "$case/permission-fault.json"
        try {
            [IO.File]::WriteAllText("$case/hook-changed-permissions/start-engine", 'start')
            $engineRecord = "$case/hook-changed-permissions/engine-started.json"
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            while (-not (Test-Path -LiteralPath $engineRecord)) {
                if ($guardian.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Engine identity not received' }
                Start-Sleep -Milliseconds 10
            }
            $enginePid = (Get-Content -LiteralPath $engineRecord -Raw | ConvertFrom-Json).pid
            $permissionSetup = Get-Process -Id $enginePid -ErrorAction Stop
            $heldSetupHandle = $permissionSetup.SafeHandle
            if ($heldSetupHandle.IsInvalid -or $permissionSetup.Path -ne $setup.FullName) { throw 'Unexpected permission-test consumer' }
            if (-not $guardian.WaitForExit(90000)) { throw 'Setup did not refuse changed permissions before deadline' }
            $refusal = Get-Content -LiteralPath "$case/changed-permissions.json" -Raw | ConvertFrom-Json
            $numericExit = ($refusal.setup_exit -is [long]) -or ($refusal.setup_exit -is [int])
            $emptyJob = ($null -ne $refusal.remaining_consumers_before_cleanup) -and ($refusal.remaining_consumers_before_cleanup -eq 0)
            if ($guardian.ExitCode -eq 0 -or -not $refusal.setup_started -or -not $numericExit -or $refusal.setup_exit -eq 0 -or -not $emptyJob) { throw 'Expected engine refusal and empty job under changed permissions' }
            foreach ($name in $beforeFiles.Keys) {
                if ((Hash "$case/installation/$name") -ne $beforeFiles[$name]) { throw "Permission refusal changed installed bytes: $name" }
            }
            $result.permission_refusal = @{guardian_exit=$guardian.ExitCode;setup_exit=$refusal.setup_exit;installed_bytes_unchanged=$true;fault_after_authentication=$true}
        } finally {
            # The denied-rename fixture has no prerequisites or hook consumers.
            # Stop/wait its guardian and retained Setup handle BEFORE restoring rights.
            if (-not $guardian.HasExited) { $guardian.Kill(); [void]$guardian.WaitForExit(5000) }
            if (-not $guardian.HasExited -or -not $permissionSetup -or -not $permissionSetup.WaitForExit(5000)) {
                throw 'Consumer termination unknown; retaining denied ACL and recorded original SDDL'
            }
            $permissionConsumersStopped = $true
            $permissionSetup.Dispose(); $permissionSetup=$null
            Set-Acl -LiteralPath $case -AclObject $originalCaseAcl
            if ((Get-Acl -LiteralPath $case).Sddl -ne $originalCaseAcl.Sddl) { throw 'Original case ACL not restored exactly' }
            $result.acl_restored_exactly = $true
            $originalCaseAcl = $null
        }
        $guardian.Dispose(); $guardian=$null
        Invoke-Probe 'repair-after-permission-refusal' $setup.FullName
        $result.permission_refusal_recovered = $true
    }
    if ($Stage -in @('C2s-version','C2s-interruption')) {
        [void][IO.Directory]::CreateDirectory("$case/payload-B/models")
        [void][IO.Directory]::CreateDirectory("$case/packages-B")
        $env:NOH_SETUP_PROBE_VERSION = '1.0.1'
        Invoke-RecordedProcess (Get-Command rustc).Source @("$PSScriptRoot/update-setup-probe/fixture.rs",'--edition','2021','-C','linker=rust-lld','-C','linker-flavor=ld.lld','-o',"$case/payload-B/fixture.exe") $workspace "$case/logs/fixture-B" 60
        Copy-Item -LiteralPath "$case/payload/models/model.bin" -Destination "$case/payload-B/models/model.bin"
        Invoke-RecordedProcess $dotnet @($packager,'pack','--packId',$id,'--packTitle','NOH disposable Setup probe','--packVersion','1.0.1','--packDir',"$case/payload-B",'--mainExe','fixture.exe','--runtime','win-x64','--channel','win-x64-stable','--delta','None','--noPortable','--skipVeloAppCheck','--shortcuts','None','--outputDir',"$case/packages-B") $case "$case/logs/pack-B" 120
        $setupsB = @(Get-ChildItem -LiteralPath "$case/packages-B" -Filter '*Setup.exe')
        if ($setupsB.Count -ne 1) { throw 'Expected one B Setup' }
        $setupB = $setupsB[0]
        $release.version = '1.0.1'
        $release.artifacts[0].file_name = $setupB.Name
        $release.artifacts[0].url = "https://github.com/example/noh/releases/download/fixture/$($setupB.Name)"
        $release.artifacts[0].size = $setupB.Length
        $release.artifacts[0].sha256 = Hash $setupB.FullName
        Write-Json @{format='noh-setup-probe-v1';release=$release} "$case/release-B.json"
        Invoke-RecordedProcess "$case/tools/update-setup-probe.exe" @('--case',$case,'--setup',$setupB.FullName,'--envelope',"$case/envelope-B.json",'--keys',"$case/keys.json",'--package-id',$id,'--attempt','sign-B','--fixture-consent','--sign-fixture-key',"$case/test-key.pk8",'--version','1.0.1') $case "$case/logs/sign-B" 10
        if ((Get-ItemProperty -LiteralPath $registry).DisplayVersion -ne '1.0.0') { throw 'Installed A baseline missing' }
        Invoke-Probe 'replace-A-with-B' $setupB.FullName -Version '1.0.1' -Envelope "$case/envelope-B.json"
        if ((Get-ItemProperty -LiteralPath $registry).DisplayVersion -ne '1.0.1') { throw 'B installation identity missing' }
        $result.setup_version_replacement = @{from='1.0.0';to='1.0.1';setup_b_sha256=(Hash $setupB.FullName);fixture_b_sha256=(Hash "$case/payload-B/fixture.exe");restarted_client=(Get-Content "$case/client-replace-A-with-B.json" -Raw | ConvertFrom-Json)}
    }
    if ($Stage -eq 'C2s-interruption') {
        # Explicit fixture reset to A prepares a second, interrupted A-to-B attempt.
        Invoke-Probe 'reset-A-for-fault' $setup.FullName
        Add-Type -Path "$case/sources/tools/update-setup-probe/RootRenameFault.cs"
        $psi = [Diagnostics.ProcessStartInfo]::new("$case/tools/update-setup-probe.exe")
        $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true
        foreach ($argument in @('--case',$case,'--setup',$setupB.FullName,'--keys',"$case/keys.json",'--envelope',"$case/envelope-B.json",'--package-id',$id,'--attempt','post-rename','--fixture-consent','--pause-before-engine','--version','1.0.1')) { $psi.ArgumentList.Add($argument) }
        $guardian = [Diagnostics.Process]::Start($psi)
        $checkpointPath = "$case/hook-post-rename/before-engine.json"
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while (-not (Test-Path -LiteralPath $checkpointPath)) {
            if ($guardian.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Post-rename pre-engine rendezvous failed' }
            Start-Sleep -Milliseconds 10
        }
        $renameFault = [RootRenameFault]::new($case, $guardian, $setupB.FullName, "$case/hook-post-rename/engine-started.json")
        [IO.File]::WriteAllText("$case/hook-post-rename/start-engine", 'start')
        if (-not $renameFault.Wait(10000)) { $result.status='inconclusive'; throw 'Root rename event deadline' }
        $observation = [ordered]@{observed_utc=$renameFault.ObservedUtc;saved_root=$renameFault.SavedRoot;saved_root_existed=$renameFault.SavedRootExisted;saved_fixture_existed=$renameFault.SavedFixtureExisted;new_root_existed=$renameFault.NewRootExisted;new_fixture_existed=$renameFault.NewFixtureExisted;setup_pid=$renameFault.SetupPid;error=$renameFault.Error}
        Write-Json $observation "$case/post-rename-observation.json"
        if ($renameFault.Error) { $result.status='inconclusive'; throw $renameFault.Error }
        if (-not $guardian.WaitForExit(5000) -or -not $renameFault.WaitForSetupExit(5000)) { throw 'Post-rename consumer did not terminate' }
        $savedRoot = Assert-ChildPath $renameFault.SavedRoot $case
        $observation.guardian_exit = $guardian.ExitCode
        $observation.setup_exit = $renameFault.SetupExit
        $observation.saved_fixture_sha256 = Hash "$savedRoot/current/fixture.exe"
        $observation.saved_model_sha256 = Hash "$savedRoot/current/models/model.bin"
        $observation.new_fixture_after_termination = Test-Path "$case/installation/current/fixture.exe"
        $observation.hook_reached = Test-Path "$case/hook-post-rename/ready"
        Write-Json $observation "$case/post-rename-observation.json"
        if ($observation.new_fixture_after_termination -or $observation.hook_reached -or $observation.saved_fixture_sha256 -ne $result.fixture_sha256 -or $observation.saved_model_sha256 -ne $result.model_sha256) {
            $result.status='inconclusive'; throw 'Post-rename state does not establish an early interrupted replacement'
        }
        $guardian.Dispose(); $guardian=$null
        $renameFault.Dispose(); $renameFault=$null
        Invoke-Probe 'repair-after-root-rename' $setupB.FullName -Version '1.0.1' -Envelope "$case/envelope-B.json"
        $result.post_rename_recovered = @{saved_root=$savedRoot;restarted_client=(Get-Content "$case/client-repair-after-root-rename.json" -Raw | ConvertFrom-Json);old_fixture_preserved=((Hash "$savedRoot/current/fixture.exe") -eq $result.fixture_sha256)}
    }
    if ($Stage -eq 'C2a') {
        # Remove both engine entry points, retaining exact old files as fault evidence.
        $engineFiles = @('Update.exe','NOH disposable Setup probe.exe')
        $engineHashes = @{}
        foreach ($name in $engineFiles) {
            $source = Assert-ChildPath "$case/installation/$name" $case
            $destination = Assert-ChildPath "$case/fault-saved/$name" $case
            $engineHashes[$name] = Hash $source
            Move-Item -LiteralPath $source -Destination $destination
            if (Test-Path -LiteralPath $source) { throw "Engine fault not established: $name" }
        }
        Invoke-Probe 'repair-missing-engine' $setup.FullName
        foreach ($name in $engineFiles) {
            if ((Hash "$case/installation/$name") -ne $engineHashes[$name]) { throw "Engine file not restored: $name" }
        }
        $result.missing_engine_repaired = $true
        $psi = [Diagnostics.ProcessStartInfo]::new("$case/tools/update-setup-probe.exe")
        $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true
        foreach ($argument in @('--case',$case,'--setup',$setup.FullName,'--keys',"$case/keys.json",'--envelope',"$case/envelope.json",'--package-id',$id,'--attempt','guardian-loss','--fixture-consent','--wait-for-guardian-loss')) { $psi.ArgumentList.Add($argument) }
        $guardian = [Diagnostics.Process]::Start($psi)
        $checkpointPath = "$case/hook-guardian-loss/checkpoint.json"
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while (-not (Test-Path -LiteralPath $checkpointPath)) {
            if ($guardian.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Guardian-loss rendezvous did not complete' }
            Start-Sleep -Milliseconds 20
        }
        $checkpoint = Get-Content -LiteralPath $checkpointPath -Raw | ConvertFrom-Json
        if (-not $checkpoint.ready_for_guardian_loss -or -not $checkpoint.hook_in_job) { throw 'Invalid guardian-loss checkpoint' }
        $consumers = @()
        try {
            foreach ($observed in $checkpoint.observed_job_processes) {
                $process = Get-Process -Id $observed.pid -ErrorAction Stop
                # Force a retained OS handle before the fault; StartTime alone does not.
                $heldHandle = $process.SafeHandle
                if ($heldHandle.IsInvalid -or $heldHandle.IsClosed) { throw 'Consumer handle is unavailable' }
                if ($process.Path -ne $observed.image) { throw 'Observed process identity changed before fault' }
                $consumers += @{process=$process;handle=$heldHandle;pid=$process.Id;start_ticks=$process.StartTime.ToUniversalTime().Ticks;image=$process.Path}
            }
            $faultTimer = [Diagnostics.Stopwatch]::StartNew()
            $guardian.Kill() # Only the owned guardian; its job, not this runner, kills consumers.
            if (-not $guardian.WaitForExit(5000)) { throw 'Guardian did not terminate' }
            $exits = @()
            foreach ($consumer in $consumers) {
                if (-not $consumer.process.WaitForExit(5000)) { throw "Consumer survived guardian: $($consumer.pid)" }
                if ($null -eq $consumer.process.ExitCode) { throw 'Consumer exit was not obtained from its retained handle' }
                $exits += @{pid=$consumer.pid;image=$consumer.image;start_ticks=$consumer.start_ticks;exit=$consumer.process.ExitCode}
            }
            $result.guardian_loss = @{guardian_pid=$guardian.Id;guardian_exit=$guardian.ExitCode;seconds=$faultTimer.Elapsed.TotalSeconds;consumers=$exits;all_observed_consumers_exited=$true}
        } finally { foreach ($consumer in $consumers) { $consumer.process.Dispose() } }
        $guardian.Dispose(); $guardian=$null
        # Once consumers have exited, their copied file/lease protections must release.
        $writeCheck = [IO.File]::Open($setup.FullName, 'Open', 'Write', 'None')
        $writeCheck.Dispose()
        Invoke-RecordedProcess "$case/fixture.exe" @() $case "$case/logs/client-after-guardian-loss" 5
        Invoke-Probe 'repair-after-guardian-loss' $setup.FullName
        $result.guardian_loss_repaired = $true
    }
    if ((Hash "$case/userdata/project.txt") -ne $dataHash) { throw 'User data sentinel changed' }
    $result.user_data_preserved = $true
    $result.status = 'passed'
} catch {
    $result.error = $_.Exception.Message
    throw
} finally {
    if ($guardian) {
        if (-not $guardian.HasExited) { $guardian.Kill(); [void]$guardian.WaitForExit(5000) }
        $guardian.Dispose()
    }
    if ($renameFault) { [void]$renameFault.WaitForSetupExit(5000); $renameFault.Dispose() }
    if ($permissionSetup) { $permissionSetup.Dispose() }
    if ($originalCaseAcl -and $permissionConsumersStopped) {
        Set-Acl -LiteralPath $case -AclObject $originalCaseAcl
        if ((Get-Acl -LiteralPath $case).Sddl -ne $originalCaseAcl.Sddl) { throw 'Original case ACL not restored exactly' }
    }
    if ($originalCaseAcl -and -not $permissionConsumersStopped) { $result.cleanup_error='Denied disposable ACL retained because consumer termination was not established; original SDDL is recorded' }
    if ($client) {
        [IO.File]::WriteAllText("$case/client-stop", 'stop')
        if (-not $client.WaitForExit(5000)) { $client.Kill($true); [void]$client.WaitForExit(5000) }
        $client.Dispose()
    }
    try {
        if (Test-Path -LiteralPath $registry) {
            $location = (Get-ItemProperty -LiteralPath $registry).InstallLocation
            if ($location.StartsWith('\\?\')) { $location = $location.Substring(4) }
            if ((Assert-PlainPath $location) -ne (Assert-PlainPath "$case/installation")) { throw 'Uninstall key points outside disposable installation' }
            # Only this new, exact, verified HKCU key; never a parent key or existing product.
            Remove-Item -LiteralPath $registry
        }
        $result.registry_cleaned = $true
    } catch { $result.status='failed'; $result.cleanup_error=$_.Exception.Message }
    $result.seconds = $timer.Elapsed.TotalSeconds
    Write-Json $result "$case/result.json"
    Write-Output "$Stage $($result.status): $case/result.json"
}
if ($result.status -ne 'passed') { throw "$Stage did not complete successfully" }
