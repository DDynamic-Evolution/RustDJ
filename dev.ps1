param(
    [ValidateSet('run', 'build', 'test', 'check', 'fmt', 'clippy')]
    [string]$Task = 'run',
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $localToolchain = Join-Path $PSScriptRoot '.tools\rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
    if (Test-Path (Join-Path $localToolchain 'cargo.exe')) {
        $env:RUSTUP_HOME = Join-Path $PSScriptRoot '.tools\rustup'
        $env:CARGO_HOME = Join-Path $PSScriptRoot '.tools\cargo'
        $env:PATH = "$localToolchain;$env:PATH"
    }
    $cargo = (Get-Command cargo -ErrorAction Stop).Source
    $arguments = @($Task)
    if ($Release -and $Task -in @('run', 'build', 'test', 'check')) { $arguments += '--release' }
    if ($Task -eq 'clippy') { $arguments += @('--all-targets', '--', '-D', 'warnings') }
    & $cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo $Task failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}
