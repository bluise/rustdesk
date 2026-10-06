@echo off
chcp 65001 >nul
setlocal EnableExtensions

rem ===========================================================================
rem  用法：把本文件与 rustdesk.exe（无窗口版）放在【同一个目录】
rem        然后双击运行 —— 会自动请求管理员权限
rem  作用：用同目录的 rustdesk.exe 安装开机自启的 Windows 服务
rem  服务名 / 显示名与客户端自带 --install-service 一致，便于识别和替换
rem  可重复运行：会先停止并删除旧服务，再按当前目录重建
rem ===========================================================================

set "SERVICE_NAME=RustDesk"
set "DISPLAY_NAME=RustDesk Service"
set "EXE=%~dp0rustdesk.exe"

rem ---------- 1) 管理员权限：没有就提权重来 ----------
net session >nul 2>&1
if errorlevel 1 (
    echo [*] 需要管理员权限，正在请求提权...
    powershell -NoProfile -Command "Start-Process -Verb RunAs -FilePath '%~f0'"
    exit /b
)

rem ---------- 2) 确认同目录下有 rustdesk.exe ----------
if not exist "%EXE%" (
    echo [x] 同目录下找不到 rustdesk.exe：
    echo     %EXE%
    echo     请把本批处理复制到 rustdesk.exe 所在目录后再运行。
    echo.
    pause
    exit /b 1
)
echo [*] 使用可执行文件：%EXE%

rem ---------- 3) 已有同名服务则先停掉再删除，确保指向当前目录 ----------
sc query "%SERVICE_NAME%" >nul 2>&1
if not errorlevel 1 (
    echo [*] 检测到已存在的服务 %SERVICE_NAME%，先停止并删除...
    sc stop "%SERVICE_NAME%" >nul 2>&1
    ping -n 4 127.0.0.1 >nul
    sc delete "%SERVICE_NAME%" >nul 2>&1
    ping -n 4 127.0.0.1 >nul
)

rem ---------- 4) 结束可能在跑的旧进程，避免文件占用和双实例 ----------
taskkill /F /IM rustdesk.exe >nul 2>&1

rem ---------- 5) 创建服务 ----------
rem  binPath 指向同目录的 exe，并带 --service 进入常驻服务模式
rem  注意 sc 的语法：等号后面必须有一个空格
sc create "%SERVICE_NAME%" binPath= "\"%EXE%\" --service" start= auto DisplayName= "%DISPLAY_NAME%" >nul
if errorlevel 1 (
    echo [x] sc create 失败，错误码 %errorlevel%
    echo     常见原因：未以管理员身份运行、或磁盘/杀软拦截。
    echo.
    pause
    exit /b 1
)

rem ---------- 6) 启动并校验 ----------
sc start "%SERVICE_NAME%" >nul
ping -n 4 127.0.0.1 >nul

echo.
echo ================== 安装结果 ==================
sc query "%SERVICE_NAME%"
echo.
sc qc "%SERVICE_NAME%" | findstr /I "BINARY_PATH_NAME START_TYPE"
echo.
echo [OK] 服务已安装并设为开机自启（START_TYPE 应为 AUTO_START）。
echo      服务名：%SERVICE_NAME%
echo      卸载命令：sc stop %SERVICE_NAME% ^&^& sc delete %SERVICE_NAME%
echo      换目录/换 exe 后，重新运行本批处理即可。
echo.
echo      提醒：dylib_virtual_display.dll 等伴随文件要和 exe 放在同一目录。
echo.
pause
endlocal