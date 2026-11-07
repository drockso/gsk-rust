# Build both release binaries from Windows without embedding personal source paths.
$ErrorActionPreference = 'Stop'
$projectBuildRoot = Split-Path -Parent $PSScriptRoot
$userBuildRoot = [Environment]::GetFolderPath('UserProfile')
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
$previousLinker = $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER
Push-Location -LiteralPath $projectBuildRoot
try {
    $releaseFlags = @(
        '-C', 'target-feature=+crt-static',
        "--remap-path-prefix=$userBuildRoot=/build",
        "--remap-path-prefix=$($userBuildRoot.Replace('\', '/'))=/build",
        "--remap-path-prefix=$projectBuildRoot=/gsk-rust",
        "--remap-path-prefix=$($projectBuildRoot.Replace('\', '/'))=/gsk-rust"
    )
    $env:CARGO_ENCODED_RUSTFLAGS = $releaseFlags -join [char]31
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    $releaseSysroot = rustc --print sysroot
    if ($LASTEXITCODE -ne 0) { throw 'Cannot locate Rust toolchain' }
    $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = Join-Path $releaseSysroot 'lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe'
    cargo build --release --locked --target x86_64-unknown-linux-musl
    if ($LASTEXITCODE -ne 0) { throw 'Linux build failed; install its target with rustup target add x86_64-unknown-linux-musl' }
}
finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = $previousLinker
    Pop-Location
}
