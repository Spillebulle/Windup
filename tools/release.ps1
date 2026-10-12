# Cut a release.
#
#   pwsh tools/release.ps1 0.1.0
#   pwsh tools/release.ps1 0.1.0 -DryRun
#
# What it does, in order, stopping at the first thing that is not right:
#
#   1. checks the working tree is clean and on the default branch
#   2. checks Cargo.toml's version matches the one asked for
#   3. checks CHANGELOG.md has a section for it, and prints the notes
#   4. runs fmt, clippy and the tests, the same gates CI runs
#   5. pushes the branch and waits for CI to pass on that very commit
#   6. writes an annotated tag and pushes it
#
# Step 5 is why this needs the GitHub CLI. The gates in step 4 run on one
# machine and CI builds for more than one, so the tag waits for CI. `-SkipCi`
# opts out; nothing else about the order is optional.
#
# Pushing the tag is the whole of "make a release": the Release workflow builds
# the binaries, packages them and publishes the notes. Nothing here uploads
# anything, so this script cannot half-publish.
#
# The notes rule is stated three times and the three must agree: here,
# tools/release-notes.sh (which the workflow runs) and
# crates/windup-desktop/tests/release.rs.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,

    # Do everything except push and tag. Use it to see the notes and confirm
    # the gates pass before anything leaves the machine.
    [switch]$DryRun,

    # Skip the test run. For when they have just been run and the tree has not
    # moved; not for when they are failing.
    [switch]$SkipTests,

    # Tag without waiting for CI to pass on the pushed commit. The escape hatch
    # for a machine with no `gh`; see the comment at the wait itself for why it
    # is not the default.
    [switch]$SkipCi
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Fail($message) {
    Write-Host "release: $message" -ForegroundColor Red
    exit 1
}

function Step($message) {
    Write-Host "==> $message" -ForegroundColor Cyan
}

if ($Version -notmatch '^\d+\.\d+\.\d+') {
    Fail "'$Version' is not a version. Give it as 0.1.0, without the leading v."
}
$tag = "v$Version"

# --- 1. the tree -------------------------------------------------------------

Step 'checking the working tree'
if (git status --porcelain) {
    Fail 'the working tree has uncommitted changes. A tag points at a commit, so anything not committed is not in the release.'
}

$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') {
    Fail "on branch '$branch', not main. Releases are cut from main."
}

if (git tag -l $tag) {
    Fail "$tag already exists. Bump the version rather than moving a tag somebody may already have fetched."
}

# --- 2. the version ----------------------------------------------------------

Step 'checking the version'
$manifest = Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"' |
    Select-Object -First 1
if (-not $manifest) { Fail 'no version found in Cargo.toml' }
$declared = $manifest.Matches[0].Groups[1].Value
if ($declared -ne $Version) {
    Fail "Cargo.toml says $declared but you asked for $Version. Edit [workspace.package] version, commit it, then run this again."
}

# --- 3. the notes ------------------------------------------------------------

Step 'reading the release notes'
# Same rule as tools/release-notes.sh and tests/release.rs: a section starts at
# `## <version>`, alone or followed by a date, and runs to the next `## `.
$lines = Get-Content 'CHANGELOG.md'
$notes = [System.Collections.Generic.List[string]]::new()
$inside = $false
foreach ($line in $lines) {
    if ($line -like '## *') {
        if ($inside) { break }
        $head = $line.Substring(3).Trim()
        if ($head -eq $Version -or $head.StartsWith("$Version ")) { $inside = $true }
        continue
    }
    if ($inside) { $notes.Add($line) }
}
$text = ($notes -join "`n").Trim()
if (-not $text) {
    Fail "CHANGELOG.md has no notes under '## $Version'. The release notes come from there, so there is nothing to publish."
}
if ($text -notmatch '(?m)^\s*- ') {
    Fail "the '## $Version' section has no bullet points. Release notes are a list of what the release brings."
}

Write-Host ''
Write-Host '--- notes that will be published -------------------------------' -ForegroundColor DarkGray
Write-Host $text
Write-Host '----------------------------------------------------------------' -ForegroundColor DarkGray
Write-Host ''

# --- 4. the gates ------------------------------------------------------------

if ($SkipTests) {
    Write-Host 'skipping fmt, clippy and tests as asked' -ForegroundColor Yellow
} else {
    Step 'cargo fmt --all --check'
    cargo fmt --all --check
    if ($LASTEXITCODE -ne 0) { Fail 'formatting is not clean' }

    Step 'cargo clippy --workspace --all-targets'
    $env:RUSTFLAGS = '-D warnings'
    cargo clippy --workspace --all-targets
    if ($LASTEXITCODE -ne 0) { Fail 'clippy is not clean' }

    Step 'cargo test --workspace'
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { Fail 'tests are failing' }
    Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
}

# --- 5. tag and push ---------------------------------------------------------

if ($DryRun) {
    Write-Host ''
    Write-Host "dry run: everything passed. Re-run without -DryRun to push $tag." -ForegroundColor Green
    exit 0
}

Step "pushing $branch"
git push origin $branch
if ($LASTEXITCODE -ne 0) { Fail 'pushing the branch failed' }

# --- 5a. wait for CI on the commit being tagged ------------------------------
#
# The gates above ran on this machine only. CI also builds ARM64 Windows and
# Linux, so the tag waits until CI is green on this very commit. Nothing is
# spent if it is not: the fix is another commit and another run of this.

if ($SkipCi) {
    Write-Host 'not waiting for CI as asked; the tag may land on a red commit' -ForegroundColor Yellow
} else {
    $sha = (git rev-parse HEAD).Trim()
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Fail @"
the GitHub CLI (gh) is not installed, so CI cannot be checked before tagging.
Install it, or pass -SkipCi to tag without waiting for the platforms this
machine does not build.
"@
    }

    Step "waiting for CI on $($sha.Substring(0, 8))"
    $deadline = (Get-Date).AddMinutes(45)
    $run = $null
    while ($true) {
        if ((Get-Date) -gt $deadline) {
            Fail 'CI has not finished within 45 minutes. Nothing has been tagged; look at the run and try again.'
        }
        # `--json` keeps this off the human-readable format, which changes.
        $runs = gh run list --workflow=ci.yml --limit 20 --json headSha,databaseId,status,conclusion,url |
            ConvertFrom-Json
        $run = $runs | Where-Object { $_.headSha -eq $sha } | Select-Object -First 1
        if ($null -eq $run) {
            # GitHub has not created the run yet; that is ordinary for the first
            # few seconds after a push.
            Write-Host '    waiting for the run to appear...' -ForegroundColor DarkGray
        } elseif ($run.status -ne 'completed') {
            Write-Host "    $($run.status)..." -ForegroundColor DarkGray
        } else {
            break
        }
        Start-Sleep -Seconds 20
    }

    if ($run.conclusion -ne 'success') {
        Fail @"
CI on this commit concluded '$($run.conclusion)'. Nothing has been tagged.
  $($run.url)
Fix it, commit, and run this again. The version and the notes are already in
place, so there is nothing to redo but the fix.
"@
    }
    Write-Host '    CI is green' -ForegroundColor Green
}

Step "tagging $tag"
git tag -a $tag -m "$Version"
if ($LASTEXITCODE -ne 0) { Fail 'tagging failed' }

Step "pushing $tag"
git push origin $tag
if ($LASTEXITCODE -ne 0) {
    git tag -d $tag | Out-Null
    Fail 'pushing the tag failed; the local tag has been removed so this can be run again'
}

Write-Host ''
Write-Host "$tag is pushed. The Release workflow is building it:" -ForegroundColor Green
Write-Host '  https://github.com/Spillebulle/Windup/actions/workflows/release.yml'
Write-Host 'It publishes the release itself when the packages are built.'
