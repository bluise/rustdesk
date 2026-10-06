@echo off
setlocal EnableExtensions

rem ===========================================================================
rem  Put this file NEXT TO rustdesk.exe (windowless build), then run it.
rem  It registers that very rustdesk.exe as an auto-starting Windows service.
rem  Everything it does is also written to install-service.log next to itself.
rem ===========================================================================

set "SERVICE_NAME=RustDesk"
set "DISPLAY_NAME=RustDesk Service"
set "EXE=%~dp0rustdesk.exe"
set "LOG=%~dp0install-service.log"

echo === install-service === >"%LOG%"
echo script: %~f0 >>"%LOG%"

rem ---------- 1) administrator check ----------
fltmc >nul 2>&1 || net session >nul 2>&1
if errorlevel 1 (
    echo [*] Not elevated. Asking for administrator rights...
    echo not-elevated >>"%LOG%"
    powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath '%~f0'"
    echo.
    echo [!] A second window should open with UAC. If you clicked "No", or nothing
    echo     appeared, right-click this file and choose "Run as administrator".
    echo.
    pause
    exit /b
)
echo elevated >>"%LOG%"
echo [*] Running as administrator.

rem ---------- 2) the exe must sit next to this script ----------
if not exist "%EXE%" (
    echo [x] rustdesk.exe not found next to this script:
    echo     %EXE%
    echo exe-missing >>"%LOG%"
    echo.
    pause
    exit /b 1
)
echo [*] exe      : %EXE%
echo [*] service  : %SERVICE_NAME%
echo exe=%EXE% >>"%LOG%"

rem ---------- 3) drop a service with the same name, so the path follows ----------
sc query "%SERVICE_NAME%" >nul 2>&1
if not errorlevel 1 (
    echo [*] Existing service found, stopping and deleting it first...
    sc stop "%SERVICE_NAME%" >>"%LOG%" 2>&1
    ping -n 4 127.0.0.1 >nul
    sc delete "%SERVICE_NAME%" >>"%LOG%" 2>&1
    ping -n 4 127.0.0.1 >nul
)

rem ---------- 4) stop running instances ----------
taskkill /F /IM rustdesk.exe >>"%LOG%" 2>&1

rem ---------- 5) create the service (output is shown AND logged) ----------
echo.
echo [*] sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto
sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto DisplayName= "%DISPLAY_NAME%" >"%LOG%.tmp" 2>&1
set "RC=%errorlevel%"
type "%LOG%.tmp"
type "%LOG%.tmp" >>"%LOG%"
del "%LOG%.tmp" >nul 2>&1
echo sc-create-exit-code=%RC% >>"%LOG%"

if not "%RC%"=="0" (
    echo.
    echo [x] Service creation failed with exit code %RC%
    echo     Paste the line above into an elevated cmd.exe to see the raw error,
    echo     or send me this log file:  %LOG%
    echo.
    pause
    exit /b 1
)

rem ---------- 6) start and verify ----------
sc start "%SERVICE_NAME%" >>"%LOG%" 2>&1
ping -n 4 127.0.0.1 >nul

echo.
echo ================== result ==================
sc query "%SERVICE_NAME%"
sc query "%SERVICE_NAME%" >>"%LOG%" 2>&1
sc qc "%SERVICE_NAME%" | findstr /I "BINARY_PATH_NAME START_TYPE"
sc qc "%SERVICE_NAME%" | findstr /I "BINARY_PATH_NAME START_TYPE" >>"%LOG%" 2>&1
echo.
echo [OK] Installed and set to auto start (START_TYPE should say AUTO_START).
echo      Log file : %LOG%
echo      Uninstall: sc stop %SERVICE_NAME% ^&^& sc delete %SERVICE_NAME%
echo.
echo      Keep dylib_virtual_display.dll in the same folder as the exe.
echo.
pause
endlocal