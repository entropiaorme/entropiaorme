@echo off
REM Backward-compatibility shim. Daily-driver dev launch is `just dev`.
REM Scheduled for retirement at dev-orchestration R5.
cd /d "%~dp0"
just dev
