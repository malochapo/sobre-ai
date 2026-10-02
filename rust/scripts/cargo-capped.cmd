@powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0cargo-capped.ps1" __sobre_cap_inner__ %*
@exit /b %ERRORLEVEL%
