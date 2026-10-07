[CmdletBinding(PositionalBinding = $false)]
param(
    [switch] $Check,

    [string] $Toolchain,

    [string] $Target,

    [Parameter(Position = 0, ValueFromRemainingArguments = $true)]
    [string[]] $DemosceneArgs
)

$ErrorActionPreference = "Stop"

$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repo

$cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
$cargoPath = if ($cargoCommand) { $cargoCommand.Source } else { $null }
if (-not $cargoPath) {
    $userProfile = [Environment]::GetFolderPath("UserProfile")
    if ($userProfile) {
        $rustupCargo = Join-Path $userProfile ".cargo\bin\cargo.exe"
        if (Test-Path $rustupCargo) {
            $cargoPath = $rustupCargo
        }
    }
}

if (-not $cargoPath) {
    [Console]::Error.WriteLine("error: cargo.exe was not found on Windows. Install the Windows Rust toolchain, then rerun this script from PowerShell or WSL.")
    exit 1
}

if (-not $Toolchain) {
    $Toolchain = $env:DEMOSCENE_WINDOWS_TOOLCHAIN
}
if (-not $Toolchain) {
    $Toolchain = "stable-x86_64-pc-windows-msvc"
}

if (-not $Target) {
    $Target = $env:DEMOSCENE_WINDOWS_TARGET
}
if (-not $Target) {
    $Target = "x86_64-pc-windows-msvc"
}

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $repo "target\windows-native"
}

if ($Check) {
    Write-Host "repo: $repo"
    Write-Host "cargo: $cargoPath"
    Write-Host "toolchain: $Toolchain"
    Write-Host "target: $Target"
    Write-Host "CARGO_TARGET_DIR: $env:CARGO_TARGET_DIR"
    exit 0
}

$cargoArgs = @("+$Toolchain", "run", "-p", "demoscene-app", "--release", "--target", $Target, "--") + $DemosceneArgs
& $cargoPath @cargoArgs
exit $LASTEXITCODE
