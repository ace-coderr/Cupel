@echo off
setlocal enabledelayedexpansion

rem Cupel bootstrap (Windows cmd)
rem Run from Desktop\hackathons\cupel:  bootstrap.cmd

rem ---------------------------------------------------------------------------
rem EDIT THIS: fork zeroclaw-labs/zeroclaw-plugins on GitHub, then put your
rem fork's URL here so your branch is push-ready from the start.
rem ---------------------------------------------------------------------------
set "FORK=https://github.com/zeroclaw-labs/zeroclaw-plugins.git"

set "ROOT=%~dp0"
cd /d "%ROOT%"

echo.
echo Cupel bootstrap
echo.

rem --- prerequisites ---------------------------------------------------------

where git >nul 2>nul
if errorlevel 1 (
    echo missing: git
    echo   install from https://git-scm.com then reopen this terminal
    exit /b 1
)

where cargo >nul 2>nul
if errorlevel 1 (
    echo missing: cargo
    echo   install Rust from https://rustup.rs then reopen this terminal
    exit /b 1
)

where rustup >nul 2>nul
if errorlevel 1 (
    echo missing: rustup
    echo   install Rust from https://rustup.rs then reopen this terminal
    exit /b 1
)

echo adding wasm32-wasip2 target...
call rustup target add wasm32-wasip2
if errorlevel 1 (
    echo could not add the wasm32-wasip2 target
    exit /b 1
)

rem --- repositories ----------------------------------------------------------

if not exist "%ROOT%zeroclaw-plugins\" (
    echo cloning zeroclaw-plugins...
    call git clone "%FORK%" "%ROOT%zeroclaw-plugins"
    if errorlevel 1 exit /b 1
) else (
    echo zeroclaw-plugins already present, skipping
)

rem The host runtime is reference only - never edited, but grep it constantly.
if not exist "%ROOT%zeroclaw\" (
    echo cloning zeroclaw host ^(reference only^)...
    call git clone --depth 1 https://github.com/zeroclaw-labs/zeroclaw.git "%ROOT%zeroclaw"
    if errorlevel 1 exit /b 1
) else (
    echo zeroclaw already present, skipping
)

rem --- place the spike -------------------------------------------------------

if not exist "%ROOT%zeroclaw-plugins\plugins\cupel-spike\" (
    echo placing spike...
    xcopy /E /I /Q "%ROOT%spike\cupel-spike" "%ROOT%zeroclaw-plugins\plugins\cupel-spike" >nul
    if errorlevel 1 exit /b 1
) else (
    echo spike already placed, skipping
)

echo.
echo Ready. Now run the spike:
echo.
echo   cd zeroclaw-plugins\plugins\cupel-spike
echo   cargo test
echo   cargo clippy --all-targets -- -D warnings
echo   cargo clippy --target wasm32-wasip2 -- -D warnings
echo   cargo build --target wasm32-wasip2 --release
echo.
echo The fourth command is the one that matters. See spike\cupel-spike\RUN.md
echo.

endlocal
