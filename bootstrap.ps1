# Cupel bootstrap (Windows PowerShell)
# Run from Desktop\hackathons\Cupel:  .\bootstrap.ps1

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot

Write-Host ""
Write-Host "Cupel bootstrap" -ForegroundColor Cyan
Write-Host ""

# --- prerequisites -----------------------------------------------------------

foreach ($cmd in @("git", "cargo", "rustup")) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        Write-Host "missing: $cmd" -ForegroundColor Red
        if ($cmd -ne "git") {
            Write-Host "  install Rust from https://rustup.rs then reopen your terminal"
        }
        exit 1
    }
}

Write-Host "adding wasm32-wasip2 target..."
rustup target add wasm32-wasip2

# --- repositories ------------------------------------------------------------
# zeroclaw-plugins is where your work lives and where the PR comes from.
# Fork it on GitHub first, then set $fork below to your fork's URL.

$fork = "https://github.com/zeroclaw-labs/zeroclaw-plugins.git"

if (-not (Test-Path "$root\zeroclaw-plugins")) {
    Write-Host "cloning zeroclaw-plugins..."
    git clone $fork "$root\zeroclaw-plugins"
} else {
    Write-Host "zeroclaw-plugins already present, skipping"
}

# zeroclaw (the host) is reference only - never edited, but grep it constantly.
if (-not (Test-Path "$root\zeroclaw")) {
    Write-Host "cloning zeroclaw host (reference only)..."
    git clone --depth 1 https://github.com/zeroclaw-labs/zeroclaw.git "$root\zeroclaw"
} else {
    Write-Host "zeroclaw already present, skipping"
}

# --- place the spike ---------------------------------------------------------

$target = "$root\zeroclaw-plugins\plugins\cupel-spike"
if (-not (Test-Path $target)) {
    Write-Host "placing spike..."
    Copy-Item -Recurse "$root\spike\cupel-spike" $target
} else {
    Write-Host "spike already placed, skipping"
}

Write-Host ""
Write-Host "Ready. Now run the spike:" -ForegroundColor Green
Write-Host ""
Write-Host "  cd zeroclaw-plugins\plugins\cupel-spike"
Write-Host "  cargo test"
Write-Host "  cargo clippy --all-targets -- -D warnings"
Write-Host "  cargo clippy --target wasm32-wasip2 -- -D warnings"
Write-Host "  cargo build --target wasm32-wasip2 --release"
Write-Host ""
Write-Host "The fourth command is the one that matters. See spike\cupel-spike\RUN.md"
Write-Host ""
