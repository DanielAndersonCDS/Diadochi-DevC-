@echo off
cargo build --release
if errorlevel 1 (echo BUILD FAILED & pause & exit /b 1)
copy /Y target\release\diadochi-devcpp.exe Diadochi-DevCpp.exe
echo Done: Diadochi-DevCpp.exe
pause
