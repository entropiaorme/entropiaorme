@echo off
REM Backward-compatibility shim. The canonical dev launch is `just dev`.
cd /d "%~dp0"
just dev
