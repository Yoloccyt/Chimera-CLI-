# scripts/toolchain-env.ps1 - repo-root-derived toolchain environment.
#
# WHY one file: nine ops/benchmark scripts each pinned 'D:\Chimera CLI' (and
# 'D:\msys64\...') in their own copy, so one fact existed nine times and had
# already drifted in quoting and in which variables each one set. On a clone at
# another path -- or a CI runner with no D: drive -- they fail at Set-Location
# or at the cargo lookup instead of degrading. Same rule the Python side learned
# in F20: a fact stored twice eventually disagrees with itself.
#
# Usage (dot-source, then call; the function returns the repository root):
#   . (Join-Path $PSScriptRoot 'toolchain-env.ps1')
#   $root = Set-ChimeraToolchainEnv
#
# Overridable by design: CHIMERA_REPO_ROOT relocates the repository,
# MSYS2_MINGW64_BIN relocates the MinGW bin dir. Both are kept optional so the
# documented default layout needs no configuration.
#
# Encoding: ASCII on purpose -- check_ps1_bom_gate requires a BOM on any .ps1
# that carries non-ASCII bytes, and a shared helper should not add that burden.

$ChimeraRepoRoot = if ($env:CHIMERA_REPO_ROOT) {
    $env:CHIMERA_REPO_ROOT
} elseif ($PSScriptRoot) {
    Split-Path -Parent $PSScriptRoot
} else {
    # Dot-sourcing from an interactive -Command context leaves $PSScriptRoot empty.
    # Falling back to the current directory keeps the helper usable there without
    # silently inventing a drive letter; Set-ChimeraToolchainEnv still verifies
    # that whatever root we landed on actually exists.
    (Get-Location).Path
}

function Set-ChimeraToolchainEnv {
    [CmdletBinding()]
    param(
        [string] $Root = $script:ChimeraRepoRoot,
        [string] $MinGWBin = $(if ($env:MSYS2_MINGW64_BIN) {
            $env:MSYS2_MINGW64_BIN
        } else {
            'D:\msys64\mingw64\bin'
        })
    )

    if (-not (Test-Path -LiteralPath $Root)) {
        throw "repository root not found: $Root"
    }

    $toolchain = Join-Path $Root '.toolchain'
    if (Test-Path -LiteralPath $toolchain) {
        # The in-repo toolchain wins: that is the project's documented layout.
        # When it is absent (a CI runner that installed rustup normally) the
        # caller's existing CARGO_HOME/PATH are left alone rather than pointed
        # at a directory that does not exist.
        $env:CARGO_HOME = Join-Path $toolchain 'cargo'
        $env:RUSTUP_HOME = Join-Path $toolchain 'rustup'
        $env:PATH = "$(Join-Path $toolchain 'cargo\bin');$env:PATH"
    }

    if (Test-Path -LiteralPath $MinGWBin) {
        $env:PATH = "$MinGWBin;$env:PATH"
    }

    # Force the temp dir inside the repository: cargo/rustc write a lot there and
    # the host default may sit on another volume (behaviour the old per-script
    # blocks had, kept on purpose rather than "cleaned up").
    $tmp = Join-Path $Root 'tmp'
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    $env:TMP = $tmp
    $env:TEMP = $tmp

    return (Resolve-Path -LiteralPath $Root).Path
}
