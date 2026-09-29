// Stub implementations for Windows C++ functions
// Used for cross-compilation from Linux with MinGW
#include <windows.h>
#include <stdint.h>

uint32_t get_current_session(BOOL rdp) { return 0xFFFFFFFF; }
BOOL is_session_locked(DWORD session_id) { return FALSE; }

HANDLE LaunchProcessWin(const uint16_t* cmd, DWORD session_id, BOOL as_user, BOOL show, DWORD* token_pid) {
    *token_pid = 0;
    return NULL;
}

BOOL GetSessionUserTokenWin(LPHANDLE lphUserToken, DWORD dwSessionId, BOOL as_user, DWORD* token_pid) {
    *token_pid = 0;
    return FALSE;
}

BOOL selectInputDesktop() { return FALSE; }
BOOL inputDesktopSelected() { return TRUE; }
BOOL is_windows_server() { return FALSE; }
BOOL is_windows_10_or_greater() { return TRUE; }

int32_t handleMask(uint8_t* out, const uint8_t* mask, int32_t width, int32_t height, int32_t bmWidthBytes, int32_t bmHeight) { return 0; }
void drawOutline(uint8_t* out, const uint8_t* in_, int32_t width, int32_t height, int32_t out_size) {}
int32_t get_di_bits(uint8_t* out, HDC dc, HBITMAP hbmColor, int32_t width, int32_t height) { return 0; }
void blank_screen(BOOL v) {}

int32_t win32_enable_lowlevel_keyboard(HWND hwnd) { return 0; }
void win32_disable_lowlevel_keyboard(HWND hwnd) {}
void win_stop_system_key_propagate(BOOL v) {}
BOOL is_win_down() { return FALSE; }
BOOL is_local_system() { return FALSE; }
void alloc_console_and_redirect() {}
BOOL is_service_running_w(const uint16_t* svc_name) { return FALSE; }

uint32_t get_active_user(uint16_t* path, uint32_t n, BOOL rdp) { return 0; }
uint32_t get_session_user_info(uint16_t* path, uint32_t n, uint32_t session_id) { return 0; }
void get_available_session_ids(wchar_t* buf, int buf_size, BOOL include_rdp) {
    if (buf_size > 0) buf[0] = 0;
}

void AddRecentDocument(const uint16_t* path) {}
void DeleteRustDeskTestCertsW() {}
DWORD PrintXPSRawData(const uint16_t* printer_name, const uint8_t* raw_data, unsigned long data_size) { return 0; }

int64_t get_directory_size_kb(const uint16_t* path) { return 0; }
void handle_custom_client_staging_dir_before_update(const uint16_t* path) {}

// MSVC compatibility functions
void* __memcpy_chk(void* dest, const void* src, size_t len, size_t destlen) {
    return memcpy(dest, src, len);
}
void* __memset_chk(void* dest, int c, size_t len, size_t destlen) {
    return memset(dest, c, len);
}
