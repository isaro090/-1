@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-windows.ps1"
if errorlevel 1 (
  echo.
  echo Build failed. See the message above and docs\BUILDING.md.
  pause
  exit /b 1
)
echo.
echo Build complete.
pause
