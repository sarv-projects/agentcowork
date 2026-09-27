<#
.SYNOPSIS
    Copy a working tree to the fixed Windows v1 validation location, run the
    repository's standard checks, and record an evidence bundle.

.DESCRIPTION
    This script is intentionally fail-closed. The source repository is never
    guessed: -Repo must name the working tree to copy. The copy is made with
    robocopy and excludes credentials, local databases, logs, build caches and
    generated dependency/build output. Git provenance is captured before the
    copy. A local Git index is initialized only in the copy when Git is present
    so Git-aware inventory gates can enumerate the snapshot; it is not a claim
    that the source history was clean.

    Standard checks run by default. Live checks are opt-in with -Live. A live
    check whose tool, credential, host, artifact or evidence prerequisite is
    absent is recorded as BLOCKED, never PASS. No secret values are written to
    logs. Start-Transcript is deliberately not used; each command gets its own
    log and its native exit code is captured immediately.
#>
[CmdletBinding()]
param(
    [string]$Repo,
    [switch]$Live,
    [switch]$AllowUnsignedBuild,
    [switch]$RunInstallDrill,
    [switch]$RunUpgradeDrill,
    [switch]$RunRollbackDrill,
    [switch]$RunSigningInspection,
    [switch]$RunUpdaterInspection,
    [switch]$ReuseCopy
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$Destination = 'C:\Users\sonali\Desktop\tests\AgentCowork'
$EvidenceRoot = 'C:\Users\sonali\Desktop\tests\evidence'
$TimestampBase = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$Timestamp = $TimestampBase
$collision = 1
while (Test-Path -LiteralPath (Join-Path $EvidenceRoot $Timestamp)) {
    $Timestamp = $TimestampBase + '-' + $collision
    $collision++
}
$EvidencePath = Join-Path $EvidenceRoot $Timestamp
$LogsPath = Join-Path $EvidencePath 'logs'
$ProvenancePath = Join-Path $EvidencePath 'provenance'
$ResultsPath = Join-Path $EvidencePath 'results.csv'
$SummaryPath = Join-Path $EvidencePath 'summary.json'
$EvidenceHashPath = Join-Path $EvidencePath 'evidence.sha256'

$script:Results = New-Object System.Collections.ArrayList
$script:SourceCommit = ''
$script:SourceBranch = ''
$script:SourceDirty = $null
$script:FinalExitCode = 1
$script:CopyReady = $false
$script:CoreExe = $null
$script:CoordinatorExe = $null
$script:LiveEnvironment = @{}

function Add-Result {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][ValidateSet('PASS', 'FAIL', 'BLOCKED', 'INFO')][string]$Status,
        [AllowNull()][string]$Command = '',
        [AllowNull()][string]$WorkingDirectory = '',
        [AllowNull()][Nullable[int]]$ExitCode = $null,
        [AllowNull()][string]$Log = '',
        [AllowNull()][string]$Detail = ''
    )

    $record = [pscustomobject][ordered]@{
        id            = $Id
        name          = $Name
        status        = $Status
        command       = $Command
        workingDirectory = $WorkingDirectory
        exitCode      = $ExitCode
        log           = $Log
        detail        = $Detail
        recordedAt    = (Get-Date).ToUniversalTime().ToString('o')
    }
    [void]$script:Results.Add($record)
    Save-Results
}

function Save-Results {
    if ($null -eq $script:Results -or $script:Results.Count -eq 0) {
        Set-Content -LiteralPath $ResultsPath -Value 'id,name,status,command,workingDirectory,exitCode,log,detail,recordedAt' -Encoding UTF8
        return
    }
    $script:Results | Export-Csv -LiteralPath $ResultsPath -NoTypeInformation -Encoding UTF8
}

function Get-Executable {
    param([Parameter(Mandatory = $true)][string]$Name)
    if (Test-Path -LiteralPath $Name -PathType Leaf) {
        return (Resolve-Path -LiteralPath $Name).Path
    }
    $command = Get-Command -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command) {
        return $null
    }
    if ($command.Source) {
        return $command.Source
    }
    if ($command.Path) {
        return $command.Path
    }
    return $command.Name
}

function Invoke-Check {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$File,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [int[]]$AllowedExitCodes = @(0),
        [switch]$BlockedOnExitTwo,
        [switch]$BlockedOnOutputSkip,
        [string[]]$BlockedOutputPatterns = @(),
        [string]$RequiredOutputPattern = '',
        [hashtable]$Environment = @{},
        [string]$Detail = ''
    )

    $safeId = ($Id -replace '[^A-Za-z0-9_.-]', '_')
    $logPath = Join-Path $LogsPath ($safeId + '.log')
    Set-Content -LiteralPath $logPath -Value ("command: {0} {1}`nworkingDirectory: {2}`n" -f $File, ($Arguments -join ' '), $WorkingDirectory) -Encoding UTF8

    $executable = Get-Executable -Name $File
    if ($null -eq $executable) {
        Add-Result -Id $Id -Name $Name -Status 'BLOCKED' -Command ("{0} {1}" -f $File, ($Arguments -join ' ')) -WorkingDirectory $WorkingDirectory -ExitCode 127 -Log $logPath -Detail ("required command is not available: {0}. {1}" -f $File, $Detail)
        return [pscustomobject]@{ id = $Id; exitCode = 127; status = 'BLOCKED'; log = $logPath; output = '' }
    }

    $previousEnvironment = @{}
    try {
        foreach ($entry in $Environment.GetEnumerator()) {
            $previousEnvironment[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key, 'Process')
            [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
        }

        $exitCode = 1
        Push-Location -LiteralPath $WorkingDirectory
        try {
            $global:LASTEXITCODE = 0
            & $executable @Arguments 1> $logPath 2>&1
            if ($null -eq $LASTEXITCODE) {
                $exitCode = 0
            } else {
                $exitCode = [int]$LASTEXITCODE
            }
        } finally {
            Pop-Location
        }
    } catch {
        Add-Content -LiteralPath $logPath -Value ("exception: {0}" -f $_.Exception.Message) -Encoding UTF8
        $exitCode = 1
    } finally {
        foreach ($key in $previousEnvironment.Keys) {
            [Environment]::SetEnvironmentVariable($key, $previousEnvironment[$key], 'Process')
        }
    }

    $status = 'FAIL'
    if ($AllowedExitCodes -contains $exitCode) {
        $status = 'PASS'
    } elseif ($BlockedOnExitTwo -and $exitCode -eq 2) {
        $status = 'BLOCKED'
    }

    $output = ''
    if (Test-Path -LiteralPath $logPath) {
        $output = Get-Content -LiteralPath $logPath -Raw -ErrorAction SilentlyContinue
        if ($null -eq $output) {
            $output = ''
        }
    }
    if ($BlockedOnOutputSkip) {
        foreach ($pattern in $BlockedOutputPatterns) {
            if ($output -match $pattern) {
                $status = 'BLOCKED'
                $Detail = ($Detail + ' output contained a skip/unavailable marker: ' + $pattern).Trim()
                break
            }
        }
    }
    if ($status -eq 'PASS' -and -not [string]::IsNullOrWhiteSpace($RequiredOutputPattern) -and $output -notmatch $RequiredOutputPattern) {
        $status = 'FAIL'
        $Detail = ($Detail + ' required output pattern was absent: ' + $RequiredOutputPattern).Trim()
    }

    Add-Result -Id $Id -Name $Name -Status $status -Command ("{0} {1}" -f $File, ($Arguments -join ' ')) -WorkingDirectory $WorkingDirectory -ExitCode $exitCode -Log $logPath -Detail $Detail
    return [pscustomobject]@{ id = $Id; exitCode = $exitCode; status = $status; log = $logPath; output = $output }
}

function Save-Text {
    param([Parameter(Mandatory = $true)][string]$Path, [AllowNull()][string]$Text)
    $parent = Split-Path -Parent $Path
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    Set-Content -LiteralPath $Path -Value $Text -Encoding UTF8
}

function Invoke-ToolVersion {
    param([Parameter(Mandatory = $true)][string]$Name, [string[]]$Arguments = @('--version'))
    $path = Join-Path $ProvenancePath ($Name + '.txt')
    $executable = Get-Executable -Name $Name
    if ($null -eq $executable) {
        Save-Text -Path $path -Text 'MISSING'
        return
    }
    try {
        $output = & $executable @Arguments 2>&1
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
        Save-Text -Path $path -Text (($output | Out-String) + "exitCode=$code")
    } catch {
        Save-Text -Path $path -Text ("ERROR: " + $_.Exception.Message)
    }
}

function Redact-PatchText {
    param([AllowNull()][string]$Text)
    if ($null -eq $Text) { return '' }
    $redacted = $Text
    $redacted = [regex]::Replace($redacted, '(?s)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----', '[REDACTED PRIVATE KEY]')
    $redacted = [regex]::Replace($redacted, '(?i)(sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16})', '[REDACTED TOKEN]')
    $redacted = [regex]::Replace($redacted, '(?im)^(\s*(?:TAURI_SIGNING_PRIVATE_KEY|AGENTCOWORK_VAULT_KEY|EVERYAIOS_VAULT_KEY)\s*=\s*).+$', '$1[REDACTED]')
    return $redacted
}

function Save-Provenance {
    $git = Get-Executable -Name 'git'
    if ($null -eq $git) {
        Add-Result -Id 'PROVENANCE-GIT' -Name 'Git provenance' -Status 'BLOCKED' -Detail 'git is required to record commit, branch, status and patch provenance.'
        return
    }

    $commands = @(
        @{ name = 'root.txt'; args = @('rev-parse', '--show-toplevel') },
        @{ name = 'commit.txt'; args = @('rev-parse', 'HEAD') },
        @{ name = 'branch.txt'; args = @('branch', '--show-current') },
        @{ name = 'status.txt'; args = @('status', '--short', '--branch') },
        @{ name = 'diff-stat.txt'; args = @('diff', '--stat', 'HEAD') }
    )
    $failed = $false
    foreach ($entry in $commands) {
        $path = Join-Path $ProvenancePath $entry.name
        $errPath = Join-Path $ProvenancePath ($entry.name + '.err')
        $gitArgs = @('-C', $Repo) + @($entry.args)
        & $git @gitArgs 1> $path 2> $errPath
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
        if ($code -ne 0) {
            $failed = $true
            Add-Content -LiteralPath $path -Value "`nexitCode=$code" -Encoding UTF8
        }
    }

    $commitPath = Join-Path $ProvenancePath 'commit.txt'
    $branchPath = Join-Path $ProvenancePath 'branch.txt'
    $statusPath = Join-Path $ProvenancePath 'status.txt'
    if (Test-Path -LiteralPath $commitPath) { $script:SourceCommit = (Get-Content -LiteralPath $commitPath -Raw).Trim() }
    if (Test-Path -LiteralPath $branchPath) { $script:SourceBranch = (Get-Content -LiteralPath $branchPath -Raw).Trim() }
    if (Test-Path -LiteralPath $statusPath) { $script:SourceDirty = -not ([string]::IsNullOrWhiteSpace((Get-Content -LiteralPath $statusPath -Raw))) }

    $patchPath = Join-Path $ProvenancePath 'working-tree.patch'
    $patchText = ''
    try {
        $patchText = (& $git '-C' $Repo 'diff' '--binary' '--no-ext-diff' 'HEAD' 2>&1 | Out-String)
        $patchCode = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
        if ($patchCode -ne 0) { $failed = $true }
        Save-Text -Path $patchPath -Text (Redact-PatchText -Text $patchText)
    } catch {
        $failed = $true
        Save-Text -Path $patchPath -Text ("patch capture failed: " + $_.Exception.Message)
    }

    $untrackedPath = Join-Path $ProvenancePath 'untracked-files.txt'
    $untrackedHashPath = Join-Path $ProvenancePath 'untracked-file-hashes.csv'
    $untracked = @()
    try {
        $untracked = @(& $git '-C' $Repo 'ls-files' '--others' '--exclude-standard' 2>$null)
        Save-Text -Path $untrackedPath -Text (($untracked | Out-String))
        $hashRows = foreach ($relative in $untracked) {
            $full = Join-Path $Repo $relative
            if (Test-Path -LiteralPath $full -PathType Leaf) {
                $hash = Get-FileHash -LiteralPath $full -Algorithm SHA256
                [pscustomobject]@{ path = $relative; length = (Get-Item -LiteralPath $full).Length; sha256 = $hash.Hash }
            }
        }
        if ($null -eq $hashRows) { $hashRows = @() }
        $hashRows | Export-Csv -LiteralPath $untrackedHashPath -NoTypeInformation -Encoding UTF8
    } catch {
        $failed = $true
        Save-Text -Path $untrackedPath -Text ('untracked capture failed: ' + $_.Exception.Message)
        Save-Text -Path $untrackedHashPath -Text 'unavailable'
    }

    $provenance = [ordered]@{
        source = $Repo
        commit = $script:SourceCommit
        branch = $script:SourceBranch
        dirtyWorkingTree = $script:SourceDirty
        patch = 'working-tree.patch (common secret-shaped values redacted; review before sharing)'
        untracked = 'untracked-files.txt + untracked-file-hashes.csv (names and hashes only)'
        copiedWithoutGitHistory = $true
    }
    Save-Text -Path (Join-Path $ProvenancePath 'provenance.json') -Text (($provenance | ConvertTo-Json -Depth 5) + "`n")
    if ($failed) {
        Add-Result -Id 'PROVENANCE-GIT' -Name 'Git provenance' -Status 'FAIL' -Detail 'One or more Git provenance captures failed; inspect provenance/*.err.'
    } else {
        Add-Result -Id 'PROVENANCE-GIT' -Name 'Git provenance' -Status 'PASS' -Detail ("commit={0}; branch={1}; working tree copied as-is; Git history is not copied." -f $script:SourceCommit, $script:SourceBranch)
    }
}

function Set-WindowsEnvironment {
    param([string]$Triplet = 'x64-windows-static-md')
    $env:CARGO_TERM_COLOR = 'always'
    $env:RUSTUP_TOOLCHAIN = '1.98.0'
    $env:VCPKG_ROOT = 'C:\vcpkg'
    $env:VCPKG_TARGET_TRIPLET = $Triplet
    $env:OPENSSL_DIR = Join-Path 'C:\vcpkg\installed' $Triplet
    $env:OPENSSL_STATIC = '1'
    $env:RUSTFLAGS = '-D warnings -Clink-arg=advapi32.lib -Clink-arg=user32.lib -Clink-arg=crypt32.lib -Clink-arg=gdi32.lib'
    # Both spellings: the current Rust harness gates on the legacy name.
    $env:AGENTCOWORK_LIVE_TEST = if ($Live) { '1' } else { '0' }
    $env:EVERYAIOS_LIVE_TEST = $env:AGENTCOWORK_LIVE_TEST
    if ($null -eq $script:CoreExe) {
        Remove-Item Env:AGENTCOWORK_CLEAN_BOOT_BIN -ErrorAction SilentlyContinue
        Remove-Item Env:EVERYAIOS_CLEAN_BOOT_BIN -ErrorAction SilentlyContinue
        Remove-Item Env:AGENTCOWORK_E2E_CORE_BIN -ErrorAction SilentlyContinue
        Remove-Item Env:EVERYAIOS_E2E_CORE_BIN -ErrorAction SilentlyContinue
    } else {
        $env:AGENTCOWORK_CLEAN_BOOT_BIN = $script:CoreExe
        $env:EVERYAIOS_CLEAN_BOOT_BIN = $script:CoreExe
        $env:AGENTCOWORK_E2E_CORE_BIN = $script:CoreExe
        $env:EVERYAIOS_E2E_CORE_BIN = $script:CoreExe
    }
}

function Test-CopyExclusions {
    $forbidden = Get-ChildItem -LiteralPath $Destination -Recurse -Force -File -ErrorAction SilentlyContinue | Where-Object {
        $_.FullName -notmatch '\\.git(\\|$)' -and (
            $_.Name -match '^\.env($|\.)' -or
            $_.Extension -in @('.key', '.pem', '.pfx', '.p12', '.sqlite', '.sqlite3', '.db', '.vault', '.log') -or
            $_.Name -match '^(credentials|secrets|service_account|auth)(\.|$)' -or
            $_.FullName -match '\\(node_modules|target|dist|\.cache|\.turbo|coverage)(\\|$)'
        )
    }
    $forbiddenPath = Join-Path $EvidencePath 'copy-exclusion-findings.txt'
    if ($null -eq $forbidden -or @($forbidden).Count -eq 0) {
        Save-Text -Path $forbiddenPath -Text 'No excluded secret/database/log/build-cache names were found in the copied tree.'
        Add-Result -Id 'COPY-EXCLUSIONS' -Name 'Copy exclusion audit' -Status 'PASS' -Detail $forbiddenPath
    } else {
        Save-Text -Path $forbiddenPath -Text (($forbidden | ForEach-Object { $_.FullName }) -join "`n")
        Add-Result -Id 'COPY-EXCLUSIONS' -Name 'Copy exclusion audit' -Status 'FAIL' -Detail ('Excluded files/directories were copied: ' + $forbiddenPath)
    }
}

function Copy-WorkingTree {
    if (Test-Path -LiteralPath $Destination) {
        if (-not $ReuseCopy) {
            Add-Result -Id 'COPY-DESTINATION' -Name 'Fixed copy destination' -Status 'FAIL' -Detail ($Destination + ' already exists; refusing to merge stale files. Use -ReuseCopy only after reviewing that directory.')
            throw 'The fixed destination already exists. Re-run with -ReuseCopy only when an in-place refresh is intentional.'
        }
        Add-Result -Id 'COPY-DESTINATION' -Name 'Fixed copy destination' -Status 'INFO' -Detail ($Destination + ' exists; -ReuseCopy permits an in-place robocopy refresh without deletion.')
    } else {
        New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    }

    $robocopy = Get-Executable -Name 'robocopy.exe'
    if ($null -eq $robocopy) {
        Add-Result -Id 'COPY-ROBOCOPY' -Name 'Working-tree copy' -Status 'BLOCKED' -Detail 'robocopy.exe is required for the fixed Windows copy.'
        throw 'robocopy.exe is unavailable.'
    }

    $excludedDirectories = @(
        '.git', 'node_modules', 'target', 'dist', '.cache', '.turbo', '.vite', 'coverage',
        '.code-intelligence', '.scip', '.codeql', '.venv', 'venv', '__pycache__', '.tauri',
        (Join-Path $Repo 'src-tauri\bin'),
        (Join-Path $Repo 'packages\coordinator\dist')
    )
    $excludedFiles = @(
        '.env', '.env.*', '.git', '*.key', '*.pem', '*.pfx', '*.p12', '*.crt', '*.cer',
        '*.sqlite', '*.sqlite3', '*.db', '*.db-shm', '*.db-wal', '*.vault', '*.log',
        '*.tmp', '*.bak', '*.dmp', '*.sig', 'id_rsa*', 'id_ed25519*', 'id_ecdsa*',
        'credentials*', 'secrets*', 'service_account.json', 'auth.json', '.npmrc', '.pypirc'
    )
    $robocopyArgs = @(
        $Repo, $Destination, '/E', '/XJ', '/R:1', '/W:1', '/COPY:DAT', '/DCOPY:DAT', '/FFT',
        '/NP', '/NFL', '/NDL', '/XD'
    ) + $excludedDirectories + @('/XF') + $excludedFiles
    $copyLog = Join-Path $LogsPath 'copy-robocopy.log'
    & $robocopy @robocopyArgs 1> $copyLog 2>&1
    $copyCode = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
    if ($copyCode -le 7) {
        Add-Result -Id 'COPY-ROBOCOPY' -Name 'Working-tree copy' -Status 'PASS' -ExitCode $copyCode -Log $copyLog -Detail 'robocopy exit codes 0-7 mean copied/extra/skipped files without a fatal error.'
        $script:CopyReady = $true
    } else {
        Add-Result -Id 'COPY-ROBOCOPY' -Name 'Working-tree copy' -Status 'FAIL' -ExitCode $copyCode -Log $copyLog -Detail 'robocopy returned a fatal error code (8 or higher).'
        throw 'robocopy failed.'
    }
    Test-CopyExclusions
}

function Initialize-CopiedGitIndex {
    $git = Get-Executable -Name 'git'
    if ($null -eq $git) {
        Add-Result -Id 'COPY-GIT-INDEX' -Name 'Git-aware snapshot index' -Status 'BLOCKED' -Detail 'git is unavailable; Git-aware inventory gates may be blocked.'
        return
    }
    $log = Join-Path $LogsPath 'copy-git-index.log'
    & $git '-C' $Destination 'init' '--quiet' *> $log
    $initCode = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
    & $git '-C' $Destination 'add' '--all' '--' '.' *>> $log
    $addCode = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
    if ($initCode -eq 0 -and $addCode -eq 0) {
        Add-Result -Id 'COPY-GIT-INDEX' -Name 'Git-aware snapshot index' -Status 'PASS' -ExitCode $addCode -Log $log -Detail 'A local index enumerates the copied snapshot only; source history and source status remain in provenance.'
    } else {
        Add-Result -Id 'COPY-GIT-INDEX' -Name 'Git-aware snapshot index' -Status 'FAIL' -ExitCode $addCode -Log $log -Detail 'Could not initialize the copied snapshot index; Git-aware gates may fail.'
    }
}

function Run-StandardChecks {
    $root = $Destination
    $crates = Join-Path $root 'crates'
    $ui = Join-Path $root 'ui'
    $coordinator = Join-Path $root 'packages\coordinator'
    $tauri = Join-Path $root 'src-tauri'

    [void](Invoke-Check -Id 'STANDARD-PNPM-INSTALL' -Name 'Workspace dependencies' -File 'pnpm' -Arguments @('install') -WorkingDirectory $root)
    [void](Invoke-Check -Id 'STANDARD-UI-NPM-CI' -Name 'UI npm ci' -File 'npm' -Arguments @('ci') -WorkingDirectory $ui)
    [void](Invoke-Check -Id 'STANDARD-UI-TYPECHECK' -Name 'UI typecheck' -File 'npm' -Arguments @('run', 'type-check') -WorkingDirectory $ui)
    [void](Invoke-Check -Id 'STANDARD-UI-BUILD' -Name 'UI build' -File 'npm' -Arguments @('run', 'build') -WorkingDirectory $ui)
    [void](Invoke-Check -Id 'STANDARD-UI-BUN-TEST' -Name 'UI Bun tests' -File 'bun' -Arguments @('test') -WorkingDirectory $ui)

    [void](Invoke-Check -Id 'STANDARD-CORE-BUILD' -Name 'Vendored core build' -File 'pnpm' -Arguments @('--filter', './packages/core-*', 'run', 'build') -WorkingDirectory $root)
    [void](Invoke-Check -Id 'STANDARD-CORE-TYPECHECK' -Name 'Vendored core typecheck' -File 'pnpm' -Arguments @('--filter', './packages/core-*', 'run', 'type-check') -WorkingDirectory $root)
    [void](Invoke-Check -Id 'STANDARD-CORE-TEST' -Name 'Vendored core tests' -File 'pnpm' -Arguments @('--filter', './packages/core-*', 'run', 'test') -WorkingDirectory $root)

    [void](Invoke-Check -Id 'STANDARD-COORDINATOR-TYPECHECK' -Name 'Coordinator typecheck' -File 'pnpm' -Arguments @('--filter', '@agentcowork/coordinator', 'type-check') -WorkingDirectory $root)
    [void](Invoke-Check -Id 'STANDARD-COORDINATOR-TEST' -Name 'Coordinator Bun tests' -File 'pnpm' -Arguments @('--filter', '@agentcowork/coordinator', 'test') -WorkingDirectory $root)
    [void](Invoke-Check -Id 'STANDARD-COORDINATOR-BUILD' -Name 'Coordinator compiled sidecar' -File 'pnpm' -Arguments @('--filter', '@agentcowork/coordinator', 'build') -WorkingDirectory $root)

    [void](Invoke-Check -Id 'STANDARD-RUST-FMT' -Name 'Rust format check' -File 'cargo' -Arguments @('fmt', '--all', '--', '--check') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-RUST-CLIPPY' -Name 'Rust clippy' -File 'cargo' -Arguments @('--all-targets', '--all-features', '--', '-D', 'warnings') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-RUST-TEST' -Name 'Rust tests' -File 'cargo' -Arguments @('test', '--all-features') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-RUST-WORKSPACE-TEST' -Name 'Rust workspace tests' -File 'cargo' -Arguments @('test', '--workspace', '--all-features') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-RUST-CORE-BUILD' -Name 'Debug core binary' -File 'cargo' -Arguments @('build', '-p', 'agentcowork-core') -WorkingDirectory $crates)

    $coreCandidates = @((Join-Path $crates 'target\debug\agentcowork-core.exe'), (Join-Path $crates 'target\debug\agentcowork-core'))
    $script:CoreExe = $coreCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($null -ne $script:CoreExe) {
        $env:AGENTCOWORK_CLEAN_BOOT_BIN = $script:CoreExe
        $env:AGENTCOWORK_E2E_CORE_BIN = $script:CoreExe
    }

    # tauri-build resolves the resource glob before cargo check, so stage the
    # compiled coordinator before the shell checks (mirrors the CI order).
    $coordinatorCandidates = @((Join-Path $coordinator 'dist\coordinator.exe'), (Join-Path $coordinator 'dist\coordinator'))
    $script:CoordinatorExe = $coordinatorCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($null -ne $script:CoordinatorExe) {
        $stageDir = Join-Path $tauri 'bin'
        New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
        $stagePath = Join-Path $stageDir 'coordinator.exe'
        Copy-Item -LiteralPath $script:CoordinatorExe -Destination $stagePath -Force
        Add-Result -Id 'STANDARD-SIDECAR-STAGE' -Name 'Tauri sidecar resource stage' -Status 'PASS' -Detail $stagePath
    } else {
        Add-Result -Id 'STANDARD-SIDECAR-STAGE' -Name 'Tauri sidecar resource stage' -Status 'BLOCKED' -Detail 'Coordinator build did not produce dist\coordinator.exe; Tauri resource-dependent gates cannot be treated as passing.'
    }

    [void](Invoke-Check -Id 'STANDARD-TAURI-CHECK' -Name 'Tauri shell check' -File 'cargo' -Arguments @('check') -WorkingDirectory $tauri)
    [void](Invoke-Check -Id 'STANDARD-TAURI-TEST' -Name 'Tauri shell library tests' -File 'cargo' -Arguments @('test', '--lib') -WorkingDirectory $tauri)

    $nodeGates = @(
        @{ id = 'STANDARD-DOC-SYNC'; file = 'scripts/check-doc-sync.mjs'; name = 'Documentation sync' },
        @{ id = 'STANDARD-VERSIONS'; file = 'scripts/check-versions.mjs'; name = 'Version lockstep' },
        @{ id = 'STANDARD-RELEASE-MATRIX'; file = 'scripts/check-release-matrix.mjs'; name = 'Release matrix' },
        @{ id = 'STANDARD-APP-METADATA'; file = 'scripts/check-app-metadata.mjs'; name = 'App metadata' },
        @{ id = 'STANDARD-NATIVE-DEPS'; file = 'scripts/check-native-deps.mjs'; name = 'Native dependency audit' },
        @{ id = 'STANDARD-SIZE-BUDGET'; file = 'scripts/check-size-budget.mjs'; name = 'Size budget schema' },
        @{ id = 'STANDARD-STORE-SCHEMAS'; file = 'scripts/check-store-schemas.mjs'; name = 'Store schemas' },
        @{ id = 'STANDARD-LICENCES'; file = 'scripts/check-licences.mjs'; name = 'Licence compliance' },
        @{ id = 'STANDARD-UPDATER-KEYS'; file = 'scripts/check-updater-keys.mjs'; name = 'Updater key custody' },
        @{ id = 'STANDARD-UPDATE-PIPELINE'; file = 'scripts/check-update-pipeline.mjs'; name = 'Update pipeline' },
        @{ id = 'STANDARD-DIAGNOSTICS'; file = 'scripts/check-diagnostics-surface.mjs'; name = 'Diagnostics surface' },
        @{ id = 'STANDARD-IPC-PARITY-MD'; args = @('--md'); file = 'scripts/ipc-parity.mjs'; name = 'IPC parity inventory' },
        @{ id = 'STANDARD-IPC-PARITY'; file = 'scripts/ipc-parity.mjs'; name = 'IPC parity JSON' },
        @{ id = 'STANDARD-RELEASE-SURFACE'; args = @('--check'); file = 'scripts/gen-release-surface.mjs'; name = 'Release surface' },
        @{ id = 'STANDARD-PUBLIC-SURFACE'; file = 'scripts/check-public-surface.mjs'; name = 'Public surface' },
        @{ id = 'STANDARD-PROMPT-STEERING'; file = 'scripts/check-prompt-steering.mjs'; name = 'Prompt steering' },
        @{ id = 'STANDARD-DOC-REFS'; file = 'scripts/check-doc-refs.mjs'; name = 'Cross-document references' },
        @{ id = 'STANDARD-VOCABULARY'; file = 'scripts/check-vocabulary.mjs'; name = 'Vocabulary' },
        @{ id = 'STANDARD-ARCH-INVARIANTS'; file = 'scripts/check-arch-invariants.mjs'; name = 'Architecture invariants' },
        # 2026-09-26: STANDARD-CODEBASE-MAP removed — CODEBASE-MAP.md archived (ARCHIVE/v0/repo-cleanup-2026-09-26/).
        @{ id = 'STANDARD-PACKAGED-E2E'; file = 'scripts/verify-packaged-e2e.mjs'; name = 'Packaged verification' }
    )
    foreach ($gate in $nodeGates) {
        $arguments = @($gate.file)
        if ($gate.ContainsKey('args')) { $arguments += @($gate.args) }
        [void](Invoke-Check -Id $gate.id -Name $gate.name -File 'node' -Arguments $arguments -WorkingDirectory $root)
    }

    [void](Invoke-Check -Id 'STANDARD-P50-SEARCH' -Name 'P50 search gate' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-search', '--test', 'p50_search_e2e') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-P50-MCP' -Name 'P50 connector/MCP gate' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-mcp', '--test', 'p50_mcp_e2e') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-P50-CRASH' -Name 'P50 crash/failure composition gate' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-core', '--test', 'p50_gates') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-SECURITY-GATE' -Name 'Security release gate' -File 'node' -Arguments @('scripts/e2e/security-gate.mjs') -WorkingDirectory $root -BlockedOnExitTwo)
    [void](Invoke-Check -Id 'STANDARD-FAILURE-INJECTION' -Name 'Failure-injection gate' -File 'node' -Arguments @('scripts/e2e/failure-injection.mjs') -WorkingDirectory $root -BlockedOnExitTwo -Environment @{ AGENTCOWORK_E2E_CORE_BIN = $script:CoreExe })
    [void](Invoke-Check -Id 'STANDARD-CLEAN-PROFILE' -Name 'Clean-profile boot' -File 'node' -Arguments @('scripts/clean-profile-boot-check.mjs') -WorkingDirectory $root -BlockedOnExitTwo -Environment @{ AGENTCOWORK_CLEAN_BOOT_BIN = $script:CoreExe })
    [void](Invoke-Check -Id 'STANDARD-PERF-P10' -Name 'P10 performance benches' -File 'cargo' -Arguments @('test', '--release', '--all-features', '--test', 'p10_bench', '--', '--test-threads=4') -WorkingDirectory $crates)
    [void](Invoke-Check -Id 'STANDARD-PERF-P45' -Name 'P45 measurement' -File 'node' -Arguments @('scripts/measure-perf-p45.mjs') -WorkingDirectory $root)
    foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
        [void](Invoke-Check -Id ('STANDARD-SIZE-MEASUREMENT-' + $target) -Name ('RSS size budget ' + $target) -File 'node' -Arguments @('scripts/check-size-budget.mjs', '--measurements', 'scripts/p45-live-measurements.json', '--target', $target) -WorkingDirectory $root)
    }

    $sbomDir = Join-Path $EvidencePath 'sbom'
    [void](Invoke-Check -Id 'STANDARD-SBOM' -Name 'SBOM and provenance' -File 'node' -Arguments @('scripts/gen-sbom.mjs', '--out', $sbomDir, '--commit', $script:SourceCommit, '--run', 'windows-v1-validation') -WorkingDirectory $root)
    Run-ReleaseQualification -Root $root
}

function Run-ReleaseQualification {
    param([Parameter(Mandatory = $true)][string]$Root)
    $environment = @{ AGENTCOWORK_LIVE_TEST = if ($Live) { '1' } else { '0' }; EVERYAIOS_LIVE_TEST = if ($Live) { '1' } else { '0' } }
    $qualification = Invoke-Check -Id 'STANDARD-RELEASE-QUALIFICATION' -Name 'P70.E1-E12 qualification harness' -File 'node' -Arguments @('scripts/release-qualify.mjs', '--execute', '--json') -WorkingDirectory $Root -Environment $environment -Detail 'The harness reports PASS, FAIL, RUNNABLE, or BLOCKED; only PASS is treated as a qualification pass.'
    $raw = $qualification.output
    $start = $raw.IndexOf('{')
    $end = $raw.LastIndexOf('}')
    $parsed = $null
    if ($start -ge 0 -and $end -gt $start) {
        try { $parsed = $raw.Substring($start, $end - $start + 1) | ConvertFrom-Json } catch { $parsed = $null }
    }
    if ($null -eq $parsed -or $null -eq $parsed.items) {
        $ids = 1..12 | ForEach-Object { 'P70.E' + $_ }
        foreach ($id in $ids) {
            Add-Result -Id $id -Name 'P70.E qualification item' -Status 'BLOCKED' -Detail 'The qualification JSON was unavailable; inspect the harness log and do not treat the item as passed.'
        }
        return
    }
    foreach ($item in @($parsed.items)) {
        $status = switch ([string]$item.status) {
            'PASS' { 'PASS' }
            'FAIL' { 'FAIL' }
            default { 'BLOCKED' }
        }
        Add-Result -Id ([string]$item.id) -Name ([string]$item.title) -Status $status -ExitCode $qualification.exitCode -Log $qualification.log -Detail ([string]$item.detail)
    }
}

function Add-BlockedLive {
    param([Parameter(Mandatory = $true)][string]$Id, [Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][string]$Reason)
    Add-Result -Id $Id -Name $Name -Status 'BLOCKED' -Detail $Reason
}

function Find-Browser {
    $names = @('chrome.exe', 'msedge.exe', 'brave.exe', 'chromium.exe')
    foreach ($name in $names) {
        $command = Get-Executable -Name $name
        if ($null -ne $command) { return $command }
    }
    $paths = @(
        'C:\Program Files\Google\Chrome\Application\chrome.exe',
        'C:\Program Files (x86)\Google\Chrome\Application\chrome.exe',
        'C:\Program Files\Microsoft\Edge\Application\msedge.exe',
        'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe',
        'C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe'
    )
    foreach ($path in $paths) { if (Test-Path -LiteralPath $path -PathType Leaf) { return $path } }
    return $null
}

function Run-LiveChecks {
    $root = $Destination
    $crates = Join-Path $root 'crates'
    $liveEnv = @{ AGENTCOWORK_LIVE_TEST = '1'; EVERYAIOS_LIVE_TEST = '1' }

    if (-not $Live) {
        Add-BlockedLive 'LIVE-ACP' 'External ACP agent' 'Live mode was not requested; pass -Live only on a disposable validation host.'
        Add-BlockedLive 'LIVE-MODEL-SOAK' 'Live model/edit-ladder/shadow-preflight soak' 'Live mode was not requested; provider credentials and a real repository are required for P70.E6.'
        Add-BlockedLive 'LIVE-VAULT-HYDRATION' 'Live vault hydration' 'Live mode was not requested; a real vault/provider hydration transcript is required for P70.E5.'
        Add-BlockedLive 'LIVE-OFFICE' 'Office/LibreOffice' 'Live mode was not requested; LibreOffice oracle evidence is not a default-suite pass.'
        Add-BlockedLive 'LIVE-CHROME-CDP' 'Chrome/CDP' 'Live mode was not requested; a real browser is required.'
        Add-BlockedLive 'LIVE-CUA' 'CUA/UIA/WGC/ConPTY' 'Live mode was not requested; no result is inferred from compile-time or unit tests.'
        Add-BlockedLive 'LIVE-UI' 'Accessibility and compact UI' 'Live mode was not requested; accessibility and compact-layout acceptance require a real packaged shell.'
        Add-BlockedLive 'LIVE-WINDOWS-X64' 'Windows x64 build' 'Live mode was not requested.'
        Add-BlockedLive 'LIVE-WINDOWS-ARM64' 'Windows ARM64 build' 'Live mode was not requested.'
        Add-BlockedLive 'LIVE-INSTALL' 'Install/uninstall drill' 'Destructive install drill was not explicitly requested.'
        Add-BlockedLive 'LIVE-UPGRADE' 'N-1 to N upgrade drill' 'Destructive upgrade drill was not explicitly requested.'
        Add-BlockedLive 'LIVE-ROLLBACK' 'Rollback drill' 'Destructive rollback drill was not explicitly requested.'
        Add-BlockedLive 'LIVE-SIGNING' 'Authenticode and updater signatures' 'Signature inspection was not explicitly requested.'
        Add-BlockedLive 'LIVE-P70-E12' 'P70.E12 sign-off' 'A fully passing E1-E11 record is required before sign-off; no default result is inferred.'
        return
    }

    [void](Invoke-Check -Id 'LIVE-ACP-SPAWN' -Name 'External ACP handshake' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-acp', '--test', 'live_spawn', '--', '--ignored', '--test-threads=1', '--nocapture') -WorkingDirectory $crates -Environment $liveEnv -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not resolvable|not available|not found)\b') -Detail 'Requires an installed external ACP CLI; an ignored test that prints skip is BLOCKED.')
    [void](Invoke-Check -Id 'LIVE-ACP-REGISTRY' -Name 'Live ACP registry refresh' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-acp', '--test', 'live_registry', '--', '--ignored', '--nocapture') -WorkingDirectory $crates -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not available|not found)\b') -Detail 'Requires network access to the live registry and is not occupancy evidence by itself.')
    Add-BlockedLive 'LIVE-MODEL-SOAK' 'Live model/edit-ladder/shadow-preflight soak' 'No credential value is collected by this runner. Attach a separately executed, redacted real-repository soak record for P70.E6.'
    Add-BlockedLive 'LIVE-VAULT-HYDRATION' 'Live vault hydration' 'No vault secret is collected by this runner. Attach a separately executed redacted first-run/provider hydration record for P70.E5.'

    [void](Invoke-Check -Id 'LIVE-OFFICE-DOCX' -Name 'LibreOffice clean DOCX oracle' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-office', '--lib', 'conformance::tests::live_oracle_opens_clean_docx', '--', '--ignored', '--test-threads=1', '--nocapture') -WorkingDirectory $crates -Environment $liveEnv -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not found|not available)\b') -Detail 'Requires LibreOffice soffice and the live oracle fixture.')
    [void](Invoke-Check -Id 'LIVE-OFFICE-PATCH' -Name 'LibreOffice patched DOCX oracle' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-office', '--lib', 'conformance::tests::live_oracle_opens_clean_after_docx_patch', '--', '--ignored', '--test-threads=1', '--nocapture') -WorkingDirectory $crates -Environment $liveEnv -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not found|not available)\b') -Detail 'Requires LibreOffice soffice and the live oracle fixture.')

    $browser = Find-Browser
    if ($null -eq $browser) {
        Add-BlockedLive 'LIVE-CHROME-CDP' 'Chrome/CDP' 'No supported Chrome/Edge/Brave executable was found.'
    } else {
        [void](Invoke-Check -Id 'LIVE-CHROME-CDP' -Name 'Chrome/CDP acceptance' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-browser', '--test', 'acceptance_cdp', '--', '--ignored', '--test-threads=1', '--nocapture') -WorkingDirectory $crates -Environment $liveEnv -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not available|not found)\b') -Detail ('Browser candidate: ' + $browser))
    }

    [void](Invoke-Check -Id 'LIVE-CUA-INVENTORY' -Name 'Native application inventory' -File 'cargo' -Arguments @('test', '-p', 'agentcowork-desktop', 'live_inventory', '--', '--ignored', '--nocapture') -WorkingDirectory $crates -Environment $liveEnv -BlockedOnOutputSkip -BlockedOutputPatterns @('(?i)\b(skip|skipped|not available|not found)\b') -Detail 'This is an inventory check, not proof of WGC, UIA action delivery, or ConPTY.')
    Add-BlockedLive 'LIVE-CUA-WGC-UIA-CONPTY' 'CUA/UIA/WGC/ConPTY acceptance' 'The current repository has no Windows live harness proving Windows.Graphics.Capture, UIA invoke/set-value, and ConPTY together; compile/unit evidence is insufficient.'

    [void](Invoke-Check -Id 'LIVE-UI-COMPACT-SMOKE' -Name 'Compact UI DOM smoke' -File 'bun' -Arguments @('test', 'src/components/chat/agent-model-picker.dom.test.tsx', 'src/components/panels/computer-use-section.dom.test.tsx', 'src/components/shell/status-bar.dom.test.tsx') -WorkingDirectory (Join-Path $root 'ui') -Environment $liveEnv -Detail 'Targeted DOM evidence only; packaged accessibility behavior still needs a host record.')
    $inspect = Get-Executable -Name 'Inspect.exe'
    $accessibilityInsights = Get-Executable -Name 'AccessibilityInsights.exe'
    if ($null -eq $inspect -and $null -eq $accessibilityInsights) {
        Add-BlockedLive 'LIVE-UI-ACCESSIBILITY' 'Windows accessibility tree' 'Neither Inspect.exe nor AccessibilityInsights.exe is available; keyboard/screen-reader acceptance cannot be claimed.'
    } else {
        Add-BlockedLive 'LIVE-UI-ACCESSIBILITY' 'Windows accessibility tree' 'An accessibility tool is present, but this script does not fabricate a manual accessibility result; record the host inspection in the evidence bundle.'
    }

    $hasAuthenticode = -not [string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE) -and -not [string]::IsNullOrWhiteSpace($env:WINDOWS_CERTIFICATE_PASSWORD)
    $hasUpdater = -not [string]::IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY) -and -not [string]::IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
    if ($hasAuthenticode -and $hasUpdater -or $AllowUnsignedBuild) {
        foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
            $triplet = if ($target -eq 'x86_64-pc-windows-msvc') { 'x64-windows-static-md' } else { 'arm64-windows-static-md' }
            $id = if ($target -eq 'x86_64-pc-windows-msvc') { 'LIVE-WINDOWS-X64' } else { 'LIVE-WINDOWS-ARM64' }
            $env:VCPKG_TARGET_TRIPLET = $triplet
            $env:OPENSSL_DIR = Join-Path 'C:\vcpkg\installed' $triplet
            $env:OPENSSL_STATIC = '1'
            [void](Invoke-Check -Id $id -Name ('Windows build ' + $target) -File 'pnpm' -Arguments @('tauri', 'build', '--target', $target, '--bundles', 'nsis,msi') -WorkingDirectory $root -Detail 'Produces installer artifacts only when the target toolchain, WebView2, and required signing prerequisites are present.')
        }
    } else {
        Add-BlockedLive 'LIVE-WINDOWS-X64' 'Windows x64 build' 'Authenticode/updater signing credentials are absent (or -AllowUnsignedBuild was not supplied); an unsigned release-shaped build is not release evidence.'
        Add-BlockedLive 'LIVE-WINDOWS-ARM64' 'Windows ARM64 build' 'Authenticode/updater signing credentials are absent (or -AllowUnsignedBuild was not supplied); an unsigned release-shaped build is not release evidence.'
    }

    if ($RunInstallDrill) {
        Add-BlockedLive 'LIVE-INSTALL' 'Install/uninstall drill' 'The script intentionally does not launch installers; execute the disposable-VM procedure in windows-v1-runbook.md and attach its transcript.'
    } else {
        Add-BlockedLive 'LIVE-INSTALL' 'Install/uninstall drill' 'Install drill was not requested with -RunInstallDrill.'
    }
    if ($RunUpgradeDrill) {
        Add-BlockedLive 'LIVE-UPGRADE' 'N-1 to N upgrade drill' 'The script intentionally does not launch installers; execute the disposable-VM procedure in windows-v1-runbook.md and attach its transcript.'
    } else {
        Add-BlockedLive 'LIVE-UPGRADE' 'N-1 to N upgrade drill' 'Upgrade drill was not requested with -RunUpgradeDrill.'
    }
    if ($RunRollbackDrill) {
        Add-BlockedLive 'LIVE-ROLLBACK' 'Rollback drill' 'The script intentionally does not launch installers; execute the disposable-VM procedure in windows-v1-runbook.md and attach its transcript.'
    } else {
        Add-BlockedLive 'LIVE-ROLLBACK' 'Rollback drill' 'Rollback drill was not requested with -RunRollbackDrill.'
    }
    if ($RunSigningInspection -and $hasAuthenticode) {
        foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
            $bundle = Join-Path $root ('src-tauri\target\' + $target + '\release\bundle')
            if (Test-Path -LiteralPath $bundle -PathType Container) {
                [void](Invoke-Check -Id ('LIVE-SIGNING-' + $target) -Name ('Authenticode inspection ' + $target) -File 'powershell.exe' -Arguments @('-NoProfile', '-NonInteractive', '-Command', "Get-ChildItem -LiteralPath '$bundle' -Recurse -File -Include *.exe,*.msi | ForEach-Object { `$sig = Get-AuthenticodeSignature -LiteralPath `$_.FullName; [pscustomobject]@{Path=`$_.FullName;Status=`$sig.Status;Signer=if(`$sig.SignerCertificate){`$sig.SignerCertificate.Subject}else{''}} } | Format-Table -AutoSize") -WorkingDirectory $root -RequiredOutputPattern '(?i)\bValid\b' -Detail 'Inspection only; a missing or invalid signature is FAIL, not a release pass.')
            } else {
                Add-BlockedLive ('LIVE-SIGNING-' + $target) ('Authenticode inspection ' + $target) ('No produced bundle exists at ' + $bundle)
            }
        }
    } else {
        Add-BlockedLive 'LIVE-SIGNING' 'Authenticode signatures' 'Signing inspection was not requested or the certificate environment is absent.'
    }
    if ($RunUpdaterInspection) {
        $manifest = Get-ChildItem -LiteralPath (Join-Path $root 'src-tauri') -Recurse -File -Filter 'latest.json' -ErrorAction SilentlyContinue | Select-Object -First 1
        $signature = if ($null -ne $manifest) { Get-Item -LiteralPath ($manifest.FullName + '.sig') -ErrorAction SilentlyContinue } else { $null }
        $minisign = Get-Executable -Name 'minisign'
        if ($null -ne $manifest -and $null -ne $signature -and $null -ne $minisign) {
            $configPath = Join-Path $root 'src-tauri\tauri.conf.json'
            $publicKeyPath = Join-Path $EvidencePath 'updater-public-key.txt'
            $publicKey = $null
            try { $publicKey = (Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json).plugins.updater.pubkey } catch { $publicKey = $null }
            if ($null -eq $publicKey) {
                Add-BlockedLive 'LIVE-UPDATER-SIGNATURE' 'Updater manifest signature' 'The updater public trust anchor is unavailable in src-tauri/tauri.conf.json.'
            } else {
                Save-Text -Path $publicKeyPath -Text ([string]$publicKey)
                [void](Invoke-Check -Id 'LIVE-UPDATER-SIGNATURE' -Name 'Updater manifest signature inspection' -File 'minisign' -Arguments @('-Vm', $manifest.FullName, '-P', $publicKeyPath, '-x', $signature.FullName) -WorkingDirectory $root -Detail 'The public trust anchor is copied into evidence; private signing material is never recorded.')
            }
        } else {
            Add-BlockedLive 'LIVE-UPDATER-SIGNATURE' 'Updater manifest signature' 'latest.json, its .sig, or minisign is unavailable; updater signature is BLOCKED.'
        }
    } else {
        Add-BlockedLive 'LIVE-UPDATER-SIGNATURE' 'Updater manifest signature' 'Updater inspection was not requested with -RunUpdaterInspection.'
    }

    $qualification = Get-ChildItem -LiteralPath (Join-Path $root 'docs\release') -Filter 'qualification-*.json' -File -ErrorAction SilentlyContinue | Select-Object -First 1
    $qualified = $false
    if ($null -ne $qualification) {
        try { $qualified = [bool]((Get-Content -LiteralPath $qualification.FullName -Raw | ConvertFrom-Json).qualified) } catch { $qualified = $false }
    }
    if ($qualified) {
        Add-Result -Id 'LIVE-P70-E12' -Name 'P70.E12 sign-off' -Status 'PASS' -Detail ('Existing qualification record: ' + $qualification.FullName)
    } else {
        Add-BlockedLive 'LIVE-P70-E12' 'P70.E12 sign-off' 'No fully passing E1-E11 qualification record is present; -record is not used by this validator because it must refuse partial evidence.'
    }
}

function Run-ArtifactInspection {
    param([Parameter(Mandatory = $true)][string]$Root)
    $signtool = Get-Executable -Name 'signtool.exe'
    $dumpbin = Get-Executable -Name 'dumpbin.exe'
    foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
        $bundle = Join-Path $Root ('src-tauri\target\' + $target + '\release\bundle')
        if (-not (Test-Path -LiteralPath $bundle -PathType Container)) { continue }
        $artifacts = @(Get-ChildItem -LiteralPath $bundle -Recurse -File -Include *.exe,*.msi -ErrorAction SilentlyContinue)
        if ($artifacts.Count -eq 0) {
            Add-BlockedLive ('LIVE-ARTIFACTS-' + $target) ('Artifact inspection ' + $target) 'The bundle contains no .exe or .msi artifact to inspect.'
            continue
        }
        if ($null -eq $signtool) {
            Add-BlockedLive ('LIVE-AUTHENTICODE-' + $target) ('Authenticode inspection ' + $target) 'signtool.exe is unavailable.'
        } else {
            foreach ($artifact in $artifacts) {
                [void](Invoke-Check -Id ('LIVE-SIGNTOOL-' + $target + '-' + $artifact.BaseName) -Name ('signtool verify ' + $artifact.Name) -File 'signtool.exe' -Arguments @('verify', '/pa', '/all', '/v', $artifact.FullName) -WorkingDirectory $Root -Detail 'A valid Authenticode chain and timestamp are required.')
            }
        }
        if ($null -eq $dumpbin) {
            Add-BlockedLive ('LIVE-PE-' + $target) ('PE architecture ' + $target) 'dumpbin.exe is unavailable; PE architecture cannot be inferred from the requested target.'
        } else {
            $binary = $artifacts | Where-Object { $_.Extension -ieq '.exe' } | Select-Object -First 1
            if ($null -eq $binary) {
                Add-BlockedLive ('LIVE-PE-' + $target) ('PE architecture ' + $target) 'No executable was found for PE inspection.'
            } else {
                $pe = Invoke-Check -Id ('LIVE-PE-' + $target) -Name ('PE architecture ' + $target) -File 'dumpbin.exe' -Arguments @('/headers', $binary.FullName) -WorkingDirectory $Root
                if ($pe.status -eq 'PASS' -and $pe.output -notmatch '(?i)(8664|AA64|x64|arm64)') {
                    Add-Result -Id ('LIVE-PE-OUTPUT-' + $target) -Name 'PE architecture output' -Status 'FAIL' -Log $pe.log -Detail 'dumpbin succeeded but no x64/ARM64 machine marker was present.'
                }
            }
        }
        $hashPath = Join-Path $EvidencePath ('artifact-hashes-' + $target + '.csv')
        $hashCommand = "Get-ChildItem -LiteralPath '$bundle' -Recurse -File | Get-FileHash -Algorithm SHA256 | Export-Csv -LiteralPath '$hashPath' -NoTypeInformation -Encoding UTF8"
        [void](Invoke-Check -Id ('LIVE-HASHES-' + $target) -Name ('Artifact SHA-256 hashes ' + $target) -File 'powershell.exe' -Arguments @('-NoProfile', '-NonInteractive', '-Command', $hashCommand) -WorkingDirectory $Root -Detail $hashPath)
    }
}

function Run-PostBuildGates {
    $root = $Destination
    foreach ($target in @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')) {
        $bundle = Join-Path $root ('src-tauri\target\' + $target + '\release\bundle')
        if (Test-Path -LiteralPath $bundle -PathType Container) {
            [void](Invoke-Check -Id ('POST-HYGIENE-' + $target) -Name ('Artifact hygiene ' + $target) -File 'node' -Arguments @('scripts/check-artifact-hygiene.mjs', '--paths', $bundle) -WorkingDirectory $root)
            [void](Invoke-Check -Id ('POST-SIZE-' + $target) -Name ('Artifact size budget ' + $target) -File 'node' -Arguments @('scripts/check-size-budget.mjs', '--bundle', $bundle, '--target', $target) -WorkingDirectory $root)
            $releaseOut = Join-Path $EvidencePath ('release-surface-' + $target)
            [void](Invoke-Check -Id ('POST-RELEASE-SURFACE-' + $target) -Name ('Release surface and checksums ' + $target) -File 'node' -Arguments @('scripts/gen-release-surface.mjs', '--artifacts', $bundle, '--out', $releaseOut) -WorkingDirectory $root)
        } else {
            Add-BlockedLive ('POST-HYGIENE-' + $target) ('Artifact hygiene ' + $target) ('No produced bundle exists at ' + $bundle + '; run a signed/approved Windows build first.')
            Add-BlockedLive ('POST-SIZE-' + $target) ('Artifact size budget ' + $target) ('No produced bundle exists at ' + $bundle + '; size budget was not evaluated.')
            Add-BlockedLive ('POST-RELEASE-SURFACE-' + $target) ('Release surface and checksums ' + $target) ('No produced bundle exists at ' + $bundle + '; checksums were not generated.')
        }
    }
    if ($Live) {
        Run-ArtifactInspection -Root $root
    } else {
        Add-BlockedLive 'LIVE-AUTHENTICODE' 'Authenticode artifact inspection' 'Artifact signature inspection is live-only; no signature is inferred from a source build.'
        Add-BlockedLive 'LIVE-PE' 'PE architecture inspection' 'PE inspection is live-only; no architecture is inferred from a requested target.'
        Add-BlockedLive 'LIVE-HASHES' 'Artifact hash inspection' 'Artifact hash inspection is live-only; no release hash is inferred from source files.'
    }
}

function Write-EvidenceHash {
    $rows = foreach ($file in (Get-ChildItem -LiteralPath $EvidencePath -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'evidence.sha256' } | Sort-Object FullName)) {
        $relative = $file.FullName.Substring($EvidencePath.Length).TrimStart('\', '/')
        $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
        '{0}  {1}' -f $hash.Hash.ToLowerInvariant(), $relative
    }
    Save-Text -Path $EvidenceHashPath -Text (($rows | Out-String))
}

function Write-Summary {
    $counts = [ordered]@{}
    foreach ($group in ($script:Results | Group-Object status)) { $counts[$group.Name] = $group.Count }
    $blocked = @($script:Results | Where-Object { $_.status -eq 'BLOCKED' }).Count
    $failed = @($script:Results | Where-Object { $_.status -eq 'FAIL' }).Count
    if ($failed -gt 0 -or $blocked -gt 0) { $script:FinalExitCode = 1 } else { $script:FinalExitCode = 0 }
    $summary = [ordered]@{
        generatedAt = (Get-Date).ToUniversalTime().ToString('o')
        sourceRepo = $Repo
        copyDestination = $Destination
        evidencePath = $EvidencePath
        liveRequested = [bool]$Live
        sourceCommit = $script:SourceCommit
        sourceBranch = $script:SourceBranch
        copiedWorkingTreeIncludesUncommittedChanges = $true
        gitHistoryCopied = $false
        counts = $counts
        exitCode = $script:FinalExitCode
        results = @($script:Results)
    }
    Save-Text -Path $SummaryPath -Text (($summary | ConvertTo-Json -Depth 8) + "`n")
    Save-Results
}

try {
    if ([string]::IsNullOrWhiteSpace($Repo)) {
        throw 'No -Repo was supplied. Set -Repo to the source working tree containing package.json, crates\, and src-tauri\; this script never guesses a source path. It copies to C:\Users\sonali\Desktop\tests\AgentCowork.'
    }
    if (-not (Test-Path -LiteralPath $Repo -PathType Container)) {
        throw ('The -Repo path does not exist or is not a directory: ' + $Repo)
    }
    $Repo = (Resolve-Path -LiteralPath $Repo).Path.TrimEnd('\')
    $DestinationFull = [IO.Path]::GetFullPath($Destination).TrimEnd('\')
    if ($Repo -ieq $DestinationFull) {
        throw '-Repo must name the source working tree, not the fixed copy destination. Choose a different source path.'
    }
    if ($Repo.StartsWith($DestinationFull + '\', [StringComparison]::OrdinalIgnoreCase) -or $DestinationFull.StartsWith($Repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The source and fixed copy destination must not contain one another; refusing recursive or ambiguous copying.'
    }
    foreach ($required in @('package.json', 'crates\Cargo.toml', 'src-tauri\Cargo.toml')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Repo $required) -PathType Leaf)) {
            throw ('The supplied -Repo is not the expected AgentCowork working tree; missing ' + $required)
        }
    }

    New-Item -ItemType Directory -Force -Path $EvidencePath, $LogsPath, $ProvenancePath | Out-Null
    Set-WindowsEnvironment
    Save-Provenance
    foreach ($tool in @('git', 'node', 'npm', 'pnpm', 'bun', 'rustc', 'cargo', 'rustup')) {
        Invoke-ToolVersion -Name $tool
    }
    Invoke-ToolVersion -Name 'powershell.exe' -Arguments @('-NoProfile', '-NonInteractive', '-Command', '$PSVersionTable.PSVersion.ToString()')
    Invoke-ToolVersion -Name 'pwsh' -Arguments @('-NoProfile', '-NonInteractive', '-Command', '$PSVersionTable.PSVersion.ToString()')
    Invoke-ToolVersion -Name 'vcpkg' -Arguments @('version')
    Save-Text -Path (Join-Path $ProvenancePath 'host.txt') -Text ("processorArchitecture={0}; os64={1}; windows={2}`n" -f $env:PROCESSOR_ARCHITECTURE, [Environment]::Is64BitOperatingSystem, [Environment]::OSVersion.VersionString)
    Copy-WorkingTree
    Initialize-CopiedGitIndex
    Set-WindowsEnvironment
    Run-StandardChecks
    Run-LiveChecks
    Run-PostBuildGates
} catch {
    $message = $_.Exception.Message
    if (-not (Test-Path -LiteralPath $EvidencePath)) {
        New-Item -ItemType Directory -Force -Path $EvidencePath, $LogsPath, $ProvenancePath | Out-Null
    }
    [Console]::Error.WriteLine($message)
    Add-Result -Id 'RUNNER' -Name 'Validation runner' -Status 'FAIL' -Detail $message
}

Write-Summary
try {
    Write-EvidenceHash
} catch {
    Add-Result -Id 'EVIDENCE-HASH' -Name 'Evidence bundle hash' -Status 'FAIL' -Detail $_.Exception.Message
    Write-Summary
    try { Write-EvidenceHash } catch { }
}
if (@($script:Results | Where-Object { $_.status -eq 'FAIL' -or $_.status -eq 'BLOCKED' }).Count -gt 0) {
    $script:FinalExitCode = 1
} else {
    $script:FinalExitCode = 0
}
Save-Results
exit $script:FinalExitCode
