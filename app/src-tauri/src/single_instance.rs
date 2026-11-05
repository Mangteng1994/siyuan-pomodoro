//! Windows 单实例：命名互斥量保护进程，命名事件唤起已有窗口。
//! 不依赖 exe 文件名或路径，同一桌面会话内的新版程序共用保护。
use std::ffi::c_void;
use std::ptr::null;
use tauri::AppHandle;

type Handle = *mut c_void;
#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> Handle;
    fn CreateEventW(attributes: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn GetLastError() -> u32;
    fn SetEvent(event: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
}

pub struct Instance { mutex: Handle, event: Handle }
impl Instance {
    pub fn wake_event(&self) -> usize { self.event as usize }
}
impl Drop for Instance {
    fn drop(&mut self) { unsafe { CloseHandle(self.mutex); CloseHandle(self.event); } }
}

pub fn acquire() -> Result<Option<Instance>, String> {
    let event_name: Vec<u16> = "Local\\SiYuanPomodoro.Wake.v1\0".encode_utf16().collect();
    let mutex_name: Vec<u16> = "Local\\SiYuanPomodoro.Instance.v1\0".encode_utf16().collect();
    unsafe {
        // 先创建事件，避免第二个进程在首个进程初始化窗口前发来唤起时丢失信号。
        let event = CreateEventW(null(), 0, 0, event_name.as_ptr());
        if event.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
        let mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        let error = GetLastError();
        if mutex.is_null() {
            CloseHandle(event);
            return Err(std::io::Error::from_raw_os_error(error as i32).to_string());
        }
        let instance = Instance { mutex, event };
        if error == 183 { // ERROR_ALREADY_EXISTS
            if !std::env::args().any(|a| a == "--hidden") && SetEvent(event) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            return Ok(None);
        }
        Ok(Some(instance))
    }
}

pub fn listen(app: AppHandle, event: usize) {
    std::thread::spawn(move || loop {
        let result = unsafe { WaitForSingleObject(event as Handle, u32::MAX) };
        if result != 0 { break; }
        crate::show_main(&app);
    });
}
