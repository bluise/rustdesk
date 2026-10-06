@echo off
setlocal EnableExtensions

rem ===========================================================================
rem  Put this file NEXT TO rustdesk.exe (windowless build), then run it.
rem  It registers that very rustdesk.exe as an auto-starting Windows service.
rem  Everything it does is written to install-service.log next to itself.
rem
rem  Deliberately avoids parenthesised if-blocks: a path such as
rem  "C:\Program Files (x86)\..." carries a ")" that closes such a block while
rem  cmd is still parsing it, which kills the script silently.
rem ===========================================================================

set "SERVICE_NAME=RustDesk"
set "DISPLAY_NAME=RustDesk Service"
set "EXE=%~dp0rustdesk.exe"
set "LOG=%~dp0install-service.log"

>"%LOG%" echo === install-service %DATE% %TIME% ===
>>"%LOG%" echo script=%~f0
>>"%LOG%" echo exe=%EXE%
>>"%LOG%" echo service=%SERVICE_NAME%
>>"%LOG%" echo manual-command=sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto DisplayName= "%DISPLAY_NAME%"

rem ---------- 1) administrator check ----------
fltmc >nul 2>&1
if not errorlevel 1 goto :elevated
net session >nul 2>&1
if not errorlevel 1 goto :elevated

>>"%LOG%" echo result=not-elevated
echo [*] Not elevated. Requesting administrator rights...
powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath '%~f0'"
echo.
echo [!] A second window should open with a UAC prompt. Accept it and continue
echo     in THAT window.
echo [!] If nothing appeared, right-click this file and choose
echo     "Run as administrator".
echo.
pause
goto :eof

rem ---------- 2) the exe must sit next to this script ----------
:elevated
>>"%LOG%" echo elevated=yes
echo [*] Running as administrator.
echo [*] exe     : "%EXE%"
echo [*] service : %SERVICE_NAME%

if exist "%EXE%" goto :have_exe
>>"%LOG%" echo result=exe-missing
echo.
echo [x] rustdesk.exe was not found next to this script:
echo     "%EXE%"
echo     Copy this file into the folder that holds rustdesk.exe and run again.
echo.
pause
goto :eof

rem ---------- 3) drop a service of the same name so the path follows ----------
:have_exe
sc query "%SERVICE_NAME%" >nul 2>&1
if errorlevel 1 goto :create
>>"%LOG%" echo removing-existing-service
echo [*] Existing service found, stopping and deleting it first...
sc stop "%SERVICE_NAME%" >>"%LOG%" 2>&1
ping -n 4 127.0.0.1 >nul
sc delete "%SERVICE_NAME%" >>"%LOG%" 2>&1
ping -n 4 127.0.0.1 >nul

rem ---------- 4) create the service ----------
:create
>>"%LOG%" echo killing-running-instances
taskkill /F /IM rustdesk.exe >>"%LOG%" 2>&1

echo.
echo [*] sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto
sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto DisplayName= "%DISPLAY_NAME%" >"%LOG%.tmp" 2>&1
set "RC=%errorlevel%"
type "%LOG%.tmp"
type "%LOG%.tmp" >>"%LOG%"
del "%LOG%.tmp" >nul 2>&1
>>"%LOG%" echo sc-create-exit-code=%RC%
if "%RC%"=="0" goto :start

>>"%LOG%" echo result=sc-create-failed
echo.
echo [x] Service creation failed with exit code %RC%
echo     The full output is above and in "%LOG%".
echo     You can also run this line yourself in an elevated cmd.exe:
echo.
echo     sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto DisplayName= "%DISPLAY_NAME%"
echo.
pause
goto :eof

rem ---------- 5) start and verify ----------
:start
sc start "%SERVICE_NAME%" >>"%LOG%" 2>&1
ping -n 4 127.0.0.1 >nul

echo.
echo ================== result ==================
sc query "%SERVICE_NAME%"
sc query "%SERVICE_NAME%" >>"%LOG%" 2>&1
sc qc "%SERVICE_NAME%" >>"%LOG%" 2>&1
sc qc "%SERVICE_NAME%" | findstr /I "BINARY_PATH_NAME START_TYPE"
>>"%LOG%" echo === done ===
echo.
echo [OK] Installed, and set to start automatically (START_TYPE reads AUTO_START).
echo      Log file  : "%LOG%"
echo      Uninstall : sc stop %SERVICE_NAME% ^&^& sc delete %SERVICE_NAME%
echo.
echo      Keep dylib_virtual_display.dll in the same folder as the exe.
echo.
pause
goto :eof